//! Cuando algo del motor se cae (el evento ProcessFailed de WebView2).
//!
//! - La pagina de una pestana se cerro de golpe (un error del sitio, o se
//!   quedo sin memoria): la pestana pasa a la pagina propia "dejo de
//!   funcionar" (CRASHED_URL), con un boton para volver a cargarla. Sin esto
//!   quedaba en blanco, sin decir nada.
//! - La pagina dejo de responder (un script que no termina): se pregunta, con
//!   un dialogo de Windows, si volver a cargarla o seguir esperando, como el
//!   "La pagina no responde" de Chrome.
//! - Se cayo el proceso principal de WebView2: ningun WebView sirve mas. Michi
//!   guarda las pestanas y se vuelve a abrir solo (`--recover`), con todo como
//!   estaba. Si vuelve a pasar enseguida, avisa y no insiste.
//! - Se cayo la pagina de la interfaz (la barra, un panel, el menu): se vuelve
//!   a cargar sola.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::ProcessFailedEventHandler;
use windows_core::Interface;
use wry::{WebView, WebViewExtWindows};

use crate::state::{own_page_path, Action, Shared, UserEvent, CRASHED_URL};

/// Por que se cayo la pagina (cambia el texto de la pagina de aviso).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Crashed,
    OutOfMemory,
}

impl Reason {
    pub fn id(self) -> &'static str {
        match self {
            Reason::Crashed => "crashed",
            Reason::OutOfMemory => "memory",
        }
    }
}

/// Lo que mostraba una pestana cuando se cayo su pagina.
#[derive(Clone, Debug)]
pub struct Crash {
    pub url: String,
    pub title: String,
    pub reason: Reason,
}

/// Argumento con el que Michi se vuelve a abrir despues de que se cayo WebView2.
pub const RECOVER_ARG: &str = "--recover";

/// Si ya se esta reabriendo Michi (el aviso llega una vez por cada WebView).
static RELAUNCHING: AtomicBool = AtomicBool::new(false);

/// Cuando arranco este proceso (ver relaunch).
fn started() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// Lo llama main al arrancar, para medir desde ahi.
pub fn mark_start() {
    let _ = started();
}

/// Este Michi se abrio solo, despues de que se cayo WebView2.
pub fn is_recovering() -> bool {
    std::env::args().skip(1).any(|a| a == RECOVER_ARG)
}

/// Engancha el aviso en el WebView de una pestana (`tab`) o de la interfaz (None).
pub fn attach(state: &Shared, webview: &WebView, tab: Option<u64>) {
    let s = state.clone();
    let handler = ProcessFailedEventHandler::create(Box::new(move |sender, args| {
        let Some(args) = args else { return Ok(()) };
        if s.borrow().shutting_down {
            return Ok(());
        }
        let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
        unsafe { args.ProcessFailedKind(&mut kind)? };
        match (kind, tab) {
            (COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED, _) => relaunch(&s),
            (COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED, Some(id)) => page_failed(&s, id, reason(&args)),
            (COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE, Some(id)) => ask_reload(&s, id),
            // La barra, un panel o el menu: se vuelven a cargar solos (su
            // pagina es nuestra y no tiene nada que perder).
            (COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED, None) => {
                eprintln!("[recuperacion] se cayo una pagina de la interfaz; se vuelve a cargar");
                if let Some(core) = sender {
                    let _ = unsafe { core.Reload() };
                }
            }
            // Un iframe, la GPU, un proceso auxiliar: el motor los repone solo.
            _ => {}
        }
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { webview.webview().add_ProcessFailed(&handler, &mut token) };
}

fn reason(args: &ICoreWebView2ProcessFailedEventArgs) -> Reason {
    let mut r = COREWEBVIEW2_PROCESS_FAILED_REASON::default();
    let known = args
        .cast::<ICoreWebView2ProcessFailedEventArgs2>()
        .is_ok_and(|a| unsafe { a.Reason(&mut r) }.is_ok());
    if known && r == COREWEBVIEW2_PROCESS_FAILED_REASON_OUT_OF_MEMORY {
        Reason::OutOfMemory
    } else {
        Reason::Crashed
    }
}

/// La pagina de la pestana `id` se cerro de golpe: pasa a la pagina "dejo de
/// funcionar". WebView2 ya puso en su lugar una pagina de error suya, sin
/// estilo y sin boton; la nuestra explica que paso y la vuelve a cargar.
fn page_failed(state: &Shared, id: u64, reason: Reason) {
    let mut st = state.borrow_mut();
    let Some(tab) = st.tabs.iter_mut().find(|t| t.id == id) else { return };
    // Si la que se cae es la propia pagina de aviso, no hay a donde ir.
    if own_page_path(&tab.url) == Some("/crashed") {
        return;
    }
    eprintln!("[recuperacion] se cayo la pagina de la pestana {id} ({:?}): {}", reason, tab.url);
    tab.crashed = Some(Crash { url: tab.real_url().to_string(), title: tab.title.clone(), reason });
    tab.loading = false;
    tab.playing = Default::default();
    st.request(Action::LoadUrl(id, CRASHED_URL.into()));
}

/// "Volver a cargar" en la pagina de aviso de esa pestana.
pub fn reload_crashed(state: &Shared, id: u64) {
    let mut st = state.borrow_mut();
    let url = st.tabs.iter().find(|t| t.id == id).and_then(|t| t.crashed.as_ref()).map(|c| c.url.clone());
    if let Some(url) = url {
        st.request(Action::LoadUrl(id, url));
    }
}

/// Lo que muestra la pagina de aviso de esa pestana: (url, titulo, motivo).
pub fn crash_info(state: &Shared, id: u64) -> Option<(String, String, &'static str)> {
    let st = state.borrow();
    let c = st.tabs.iter().find(|t| t.id == id)?.crashed.as_ref()?;
    Some((c.url.clone(), c.title.clone(), c.reason.id()))
}

/// Pestanas con la pregunta "no responde" abierta (una sola por pestana: el
/// motor repite el aviso mientras la pagina siga colgada).
fn asking() -> &'static Mutex<HashSet<u64>> {
    static ASKING: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();
    ASKING.get_or_init(Default::default)
}

/// La pagina de la pestana `id` no responde: se pregunta si volver a
/// cargarla. El dialogo va en otro hilo (modal en este frenaria todo Michi, y
/// la pagina puede volver a responder sola mientras tanto).
fn ask_reload(state: &Shared, id: u64) {
    let (title, proxy) = {
        let st = state.borrow();
        let Some(tab) = st.tabs.iter().find(|t| t.id == id) else { return };
        let title = if tab.title.is_empty() { tab.url.clone() } else { tab.title.clone() };
        (title, st.proxy.clone())
    };
    if !asking().lock().map(|mut a| a.insert(id)).unwrap_or(false) {
        return;
    }
    eprintln!("[recuperacion] la pagina de la pestana {id} no responde");
    std::thread::spawn(move || {
        let reload = rfd::MessageDialog::new()
            .set_parent(&crate::win::Owner)
            .set_level(rfd::MessageLevel::Warning)
            .set_title("La página no responde")
            .set_description(format!(
                "\"{title}\" dejó de responder.\n\nPuedes esperar a ver si vuelve sola, o volver a cargarla \
                 (lo que no se haya guardado en ella se pierde).\n\n¿Volver a cargarla?"
            ))
            .set_buttons(rfd::MessageButtons::YesNo)
            .show()
            == rfd::MessageDialogResult::Yes;
        if let Ok(mut a) = asking().lock() {
            a.remove(&id);
        }
        if let (true, Some(proxy)) = (reload, proxy) {
            let _ = proxy.send_event(UserEvent::ReloadHung(id));
        }
    });
}

/// El usuario eligio volver a cargar la pagina colgada.
pub fn reload_hung(state: &Shared, id: u64) {
    let st = state.borrow();
    if let Some(tab) = st.tabs.iter().find(|t| t.id == id) {
        let _ = tab.webview.reload();
    }
}

/// Se cayo el proceso principal de WebView2: se guardan las pestanas y
/// Michi se vuelve a abrir solo, en la ventana nueva con las mismas pestanas.
/// Si este mismo Michi ya venia de eso hace menos de un minuto, algo anda mal
/// con WebView2: se avisa en vez de entrar en un bucle de reaperturas.
fn relaunch(state: &Shared) {
    if RELAUNCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    eprintln!("[recuperacion] se cayo el proceso principal de WebView2");
    state.borrow().save_session();
    if let Some(w) = state.borrow().window.clone() {
        w.set_visible(false);
    }
    let looping = is_recovering() && started().elapsed() < Duration::from_secs(60);
    let exe = std::env::current_exe();
    match (looping, exe) {
        (false, Ok(exe)) => {
            let _ = std::process::Command::new(exe).arg(RECOVER_ARG).spawn();
        }
        _ => {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("Michi")
                .set_description(
                    "El motor de Michi (Microsoft Edge WebView2) dejó de funcionar dos veces seguidas, así que \
                     Michi se cerró. Tus pestañas quedaron guardadas.\n\nSi vuelve a pasar, reinicia la PC o \
                     reinstala WebView2 desde la página de Microsoft.",
                )
                .show();
        }
    }
    std::process::exit(0);
}
