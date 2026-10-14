//! Control de Michi desde otra aplicacion del usuario (May, la asistente de
//! voz): listar, abrir, cambiar y cerrar pestanas, navegar, traer la ventana
//! al frente y mandarle comandos del protocolo DevTools (CDP) a una pestana
//! puntual, que es lo que permite leer la pagina, hacer clic, escribir o
//! sacar una captura.
//!
//! Por un pipe con nombre (nunca desde la red), un pedido por conexion:
//! el cliente escribe una linea JSON {"cmd": ...} y recibe una linea JSON
//! {"ok": true, ...} o {"ok": false, "error": "..."}. Los comandos CDP van
//! por ICoreWebView2::CallDevToolsProtocolMethod de la pestana pedida: no se
//! abre ningun puerto de depuracion y cada comando cae en la pestana justa.
//!
//! Para encontrarlo, el principal deja `control.json` en la carpeta de datos
//! con el nombre del pipe, su pid, la ruta del .exe y el usuario dueno del
//! pipe.
//!
//! Seguridad: cualquier programa de este usuario puede hablarle al pipe (no
//! hay forma de saber que es May), y con CDP se puede leer y usar cualquier
//! pagina, sesiones iniciadas incluidas. Por eso:
//! - Viene apagado. La primera vez que algo se conecta, Michi le pregunta al
//!   usuario si lo permite; despues se cambia en Ajustes > Privacidad y
//!   seguridad ("Permitir que May controle Michi").
//! - Nunca las paginas propias (ajustes: contrasenas, datos, extensiones):
//!   ni navegar a ellas ni mandarles CDP. Solo direcciones web y archivos
//!   locales (sin carpetas de red). La pestana nueva si se puede abrir.
//! - Las pestanas de incognito del usuario no existen para el pipe: solo las
//!   de incognito que abrio el mismo (Tab::assistant).
//! - Mientras se usa, la barra lo muestra (window.onAssistant).
//! - El pipe es solo de este usuario de Windows (ver pipes.rs).

use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
use windows_core::HSTRING;
use winit::event_loop::EventLoopProxy;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{FlushFileBuffers, ReadFile, WriteFile, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{
    DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use wry::WebViewExtWindows;

use crate::state::{is_own_page, own_page_path, Shared, Tab, UserEvent, HOME_LOAD_URL, HOME_URL};
use crate::{storage, tabs, win};

/// Archivo (en la carpeta de datos) por el que los clientes encuentran el pipe.
pub const DISCOVERY_FILE: &str = "control.json";
/// Lo maximo que se acepta en un pedido (un script o un CDP grande entra).
const MAX_REQUEST: usize = 4 * 1024 * 1024;
/// Cuanto se espera la respuesta del bucle de eventos (una captura o una
/// pagina pesada pueden tardar).
const REPLY_TIMEOUT: Duration = Duration::from_secs(60);
/// Cuanto sigue el aviso de la barra despues del ultimo pedido.
const INDICATOR_FOR: Duration = Duration::from_secs(8);

/// Un pedido que llego por el pipe, para resolverlo en el hilo de la interfaz.
#[derive(Debug, Clone)]
pub struct Request {
    pub body: Value,
    pub reply: Sender<Value>,
}

fn pipe_name() -> String {
    crate::pipes::name(&format!("Michi-control-{:016x}", crate::single_instance::install_id()))
}

/// Arranca el pipe (en su propio hilo) y deja control.json. Solo el principal.
pub fn listen(proxy: EventLoopProxy<UserEvent>) {
    let name = pipe_name();
    write_discovery(&name);
    std::thread::spawn(move || {
        let mut listener = crate::pipes::Listener::new(
            &name,
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            MAX_REQUEST as u32,
            MAX_REQUEST as u32,
        );
        loop {
            let pipe = listener.accept();
            // Cada conexion en su hilo: un CDP lento no frena a los demas.
            let proxy = proxy.clone();
            let pipe = pipe as usize;
            std::thread::spawn(move || serve(pipe as HANDLE, &proxy));
        }
    });
}

fn write_discovery(pipe: &str) {
    let info = json!({
        "pipe": pipe,
        "pid": std::process::id(),
        "exe": std::env::current_exe().ok(),
        "version": env!("CARGO_PKG_VERSION"),
        // Un cliente puede comprobar que el pipe sea de este usuario (como
        // hace single_instance::forward) antes de mandar nada.
        "owner": crate::pipes::user_sid(),
    });
    let dir = storage::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(DISCOVERY_FILE), info.to_string());
}

fn serve(pipe: HANDLE, proxy: &EventLoopProxy<UserEvent>) {
    let response = match read_line(pipe).and_then(|raw| serde_json::from_slice::<Value>(&raw).ok()) {
        None => error("pedido inválido"),
        Some(body) => {
            let (tx, rx) = mpsc::channel();
            if proxy.send_event(UserEvent::Control(Request { body, reply: tx })).is_err() {
                error("Michi se esta cerrando")
            } else {
                rx.recv_timeout(REPLY_TIMEOUT).unwrap_or_else(|_| error("Michi no respondió a tiempo"))
            }
        }
    };
    let mut out = response.to_string();
    out.push('\n');
    let mut written = 0u32;
    unsafe {
        WriteFile(pipe, out.as_ptr(), out.len() as u32, &mut written, std::ptr::null_mut());
        FlushFileBuffers(pipe);
        DisconnectNamedPipe(pipe);
        CloseHandle(pipe);
    }
}

/// Lee hasta el primer salto de linea (o hasta que el otro lado cierre).
fn read_line(pipe: HANDLE) -> Option<Vec<u8>> {
    let mut data = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let mut read = 0u32;
        let ok = unsafe { ReadFile(pipe, buf.as_mut_ptr(), buf.len() as u32, &mut read, std::ptr::null_mut()) } != 0;
        data.extend_from_slice(&buf[..read as usize]);
        if let Some(end) = data.iter().position(|b| *b == b'\n') {
            data.truncate(end);
            return Some(data);
        }
        if !ok || read == 0 {
            return (!data.is_empty()).then_some(data);
        }
        if data.len() > MAX_REQUEST {
            return None;
        }
    }
}

fn error(message: &str) -> Value {
    json!({ "ok": false, "error": message })
}

fn ok(mut extra: Value) -> Value {
    if let Some(map) = extra.as_object_mut() {
        map.insert("ok".into(), Value::Bool(true));
    }
    extra
}

/// En el hilo de la interfaz (UserEvent::Control), fuera de cualquier
/// callback de WebView2: aca si se pueden crear y cerrar pestanas.
pub fn handle(state: &Shared, req: Request) {
    if state.borrow().window.is_none() {
        let _ = req.reply.send(error("Michi todavía está arrancando"));
        return;
    }
    let (allowed, asked, asking) = {
        let st = state.borrow();
        (st.settings.allow_assistant, st.settings.assistant_asked, st.assistant_asking)
    };
    if !allowed {
        let message = if asking {
            "Michi está esperando que el usuario permita el control desde May."
        } else if asked {
            "Michi tiene apagado el control desde May (Ajustes > Privacidad y seguridad)."
        } else {
            return ask_consent(state, req);
        };
        let _ = req.reply.send(error(message));
        return;
    }
    show_indicator(state);

    let body = &req.body;
    let cmd = body.get("cmd").and_then(Value::as_str).unwrap_or("");
    let tab = body.get("tab").and_then(Value::as_u64);
    let response = match cmd {
        "hello" => ok(json!({ "version": env!("CARGO_PKG_VERSION"), "pid": std::process::id() })),
        "tabs" => ok(tabs_json(state)),
        "focus" => {
            win::force_to_front();
            ok(json!({}))
        }
        "open" => {
            let incognito = body.get("incognito").and_then(Value::as_bool).unwrap_or(false);
            match resolve(state, body) {
                None => error("dirección inválida"),
                Some(url) => {
                    let before = state.borrow().tabs.iter().map(|t| t.id).max();
                    tabs::open_tab(state, &url, incognito);
                    let id = {
                        let mut st = state.borrow_mut();
                        // La pestana recien creada (la de id mas alto) es suya.
                        let new = st.tabs.iter_mut().filter(|t| Some(t.id) > before).max_by_key(|t| t.id);
                        new.map(|t| {
                            t.assistant = true;
                            t.id
                        })
                    };
                    if body.get("focus").and_then(Value::as_bool).unwrap_or(true) {
                        win::force_to_front();
                    }
                    ok(json!({ "id": id, "url": url }))
                }
            }
        }
        "navigate" => {
            match (resolve(state, body), core_of(state, tab)) {
                (None, _) => error("dirección inválida"),
                (_, None) => error("no existe esa pestaña"),
                (Some(url), Some((id, core))) => {
                    // En una pestana que ya existe, la propia va con su forma http.
                    let url = if url == HOME_URL { HOME_LOAD_URL.to_string() } else { url };
                    let _ = unsafe { core.Navigate(&HSTRING::from(url.as_str())) };
                    ok(json!({ "id": id, "url": url }))
                }
            }
        }
        "activate" => match find(state, tab) {
            None => error("no existe esa pestaña"),
            Some(i) => {
                let changed = {
                    let mut st = state.borrow_mut();
                    let changed = st.active != i;
                    st.active = i;
                    changed
                };
                if changed {
                    tabs::activated(state);
                }
                if body.get("focus").and_then(Value::as_bool).unwrap_or(true) {
                    win::force_to_front();
                }
                ok(json!({}))
            }
        },
        "close" => match find(state, tab).map(|i| state.borrow().tabs[i].id) {
            None => error("no existe esa pestaña"),
            Some(id) => {
                tabs::close_tab(state, id);
                ok(json!({}))
            }
        },
        "back" | "forward" | "reload" | "stop" => match core_of(state, tab) {
            None => error("no existe esa pestaña"),
            Some((_, core)) => {
                let _ = unsafe {
                    match cmd {
                        "back" => core.GoBack(),
                        "forward" => core.GoForward(),
                        "reload" => core.Reload(),
                        _ => core.Stop(),
                    }
                };
                ok(json!({}))
            }
        },
        "cdp" => {
            let method = body.get("method").and_then(Value::as_str).unwrap_or("").to_string();
            let params = body.get("params").cloned().unwrap_or_else(|| json!({}));
            match core_of(state, tab) {
                None => error("no existe esa pestaña"),
                Some(_) if method.is_empty() => error("falta el método"),
                Some((id, _)) if !cdp_allowed(state, id) => {
                    error("Michi no deja controlar sus páginas internas (ajustes, avisos) desde otro programa.")
                }
                Some(_) if !navigation_allowed(&method, &params) => error("dirección inválida"),
                Some((_, core)) => {
                    // La respuesta llega despues, en el callback.
                    call_cdp(&core, &method, &params, req.reply);
                    return;
                }
            }
        }
        _ => error("comando desconocido"),
    };
    let _ = req.reply.send(response);
}

/// La primera vez que algo se conecta: le pregunta al usuario (en otro hilo,
/// el cuadro es modal). El pedido espera la respuesta (ver on_consent).
fn ask_consent(state: &Shared, req: Request) {
    let proxy = {
        let mut st = state.borrow_mut();
        st.assistant_asking = true;
        st.proxy.clone()
    };
    let Some(proxy) = proxy else { return };
    win::force_to_front();
    std::thread::spawn(move || {
        let yes = rfd::MessageDialog::new()
            .set_parent(&crate::win::Owner)
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Control desde otro programa")
            .set_description(
                "Un programa de este equipo (May, la asistente de voz, u otro que diga serlo) quiere controlar \
                 Michi: abrir y cerrar pestañas, y leer y usar las páginas que abras, con tus sesiones iniciadas.\n\n\
                 ¿Lo permites? Se puede cambiar cuando quieras en Ajustes > Privacidad y seguridad.",
            )
            .set_buttons(rfd::MessageButtons::YesNo)
            .show()
            == rfd::MessageDialogResult::Yes;
        let _ = proxy.send_event(UserEvent::AssistantConsent(yes, req));
    });
}

/// La respuesta del usuario: queda guardada (no se vuelve a preguntar) y se
/// atiende el pedido que esperaba.
pub fn on_consent(state: &Shared, allowed: bool, req: Request) {
    {
        let mut st = state.borrow_mut();
        st.assistant_asking = false;
        st.settings.allow_assistant = allowed;
        st.settings.assistant_asked = true;
        st.save_settings();
    }
    crate::sync::push_own_pages(state);
    handle(state, req);
}

/// La barra muestra que May esta usando Michi, hasta INDICATOR_FOR despues
/// del ultimo pedido.
fn show_indicator(state: &Shared) {
    let was_off = {
        let mut st = state.borrow_mut();
        st.assistant_until.replace(Instant::now() + INDICATOR_FOR).is_none()
    };
    if was_off {
        push_indicator(state, true);
    }
    state.borrow().wake();
}

fn push_indicator(state: &Shared, on: bool) {
    let st = state.borrow();
    if let Some(tb) = &st.topbar {
        let _ = tb.evaluate_script(&format!("window.onAssistant && window.onAssistant({on})"));
    }
}

/// Desde el bucle de eventos: apaga el aviso de la barra a su hora.
pub fn tick(state: &Shared) -> Option<Instant> {
    let until = state.borrow().assistant_until?;
    if Instant::now() < until {
        return Some(until);
    }
    state.borrow_mut().assistant_until = None;
    push_indicator(state, false);
    None
}

/// "search": lo que se busca con el buscador elegido. "url": texto escrito
/// o URL, como la barra de direcciones (dominios con https://, lo demas al
/// buscador). La pestana nueva propia se pide con "" o "about:newtab".
fn resolve(state: &Shared, body: &Value) -> Option<String> {
    let engine = state.borrow().settings.search_engine.clone();
    if let Some(query) = body.get("search").and_then(Value::as_str).map(str::trim).filter(|q| !q.is_empty()) {
        return Some(storage::search_url(&engine, query));
    }
    let input = body.get("url").and_then(Value::as_str).unwrap_or("").trim();
    if input.is_empty() || input == "about:newtab" {
        return Some(HOME_URL.to_string());
    }
    let url = storage::resolve_input(input, &engine)?;
    is_allowed_target(&url).then_some(url)
}

/// Adonde se puede llevar una pestana desde el pipe: la web y archivos de
/// este equipo. Nunca codigo (javascript:, data:), nuestras paginas internas
/// ni una carpeta de red (file://servidor/..., que le manda la credencial de
/// Windows a ese servidor).
fn is_allowed_target(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("file:///")) && !is_own_page(url)
}

/// Page.navigate por CDP: con el mismo filtro que "navigate".
fn navigation_allowed(method: &str, params: &Value) -> bool {
    if method != "Page.navigate" {
        return true;
    }
    params.get("url").and_then(Value::as_str).is_some_and(is_allowed_target)
}

/// Si se le puede mandar CDP a esa pestana: no si muestra (o esta yendo a)
/// una pagina propia que no sea la pestana nueva. Lo de "esta yendo" importa:
/// un `location = ...` por CDP seguido enseguida de otro comando caeria en la
/// pagina nueva.
fn cdp_allowed(state: &Shared, id: u64) -> bool {
    let st = state.borrow();
    let Some(t) = st.tabs.iter().find(|t| t.id == id) else { return false };
    let internal = |url: &str| own_page_path(url).is_some_and(|p| p != "/");
    !internal(&t.url) && !internal(&t.sec.nav_target)
}

/// Las pestanas que el pipe puede ver: todas menos las de incognito que abrio
/// el usuario.
fn visible(t: &Tab) -> bool {
    !t.incognito || t.assistant
}

fn tabs_json(state: &Shared) -> Value {
    let st = state.borrow();
    let list: Vec<Value> = st
        .tabs
        .iter()
        .enumerate()
        .filter(|(_, t)| visible(t))
        .map(|(i, t)| {
            json!({
                "id": t.id,
                "title": t.title,
                "url": t.url,
                "active": i == st.active,
                "incognito": t.incognito,
                "loading": t.loading,
                "playing": t.playing.any(),
                "own": is_own_page(&t.url),
            })
        })
        .collect();
    let active = st.active_tab().filter(|t| visible(t)).map(|t| t.id);
    json!({ "tabs": list, "active": active })
}

/// Indice de la pestana (None = la del frente), si el pipe la puede ver.
fn find(state: &Shared, tab: Option<u64>) -> Option<usize> {
    let st = state.borrow();
    let i = match tab {
        None => (!st.tabs.is_empty()).then_some(st.active)?,
        Some(id) => st.tabs.iter().position(|t| t.id == id)?,
    };
    visible(&st.tabs[i]).then_some(i)
}

/// El puntero COM de la pestana: el prestamo de `state` se suelta antes de
/// usarlo (navegar dispara eventos que lo vuelven a pedir).
fn core_of(state: &Shared, tab: Option<u64>) -> Option<(u64, webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2)> {
    let i = find(state, tab)?;
    let st = state.borrow();
    let t = &st.tabs[i];
    Some((t.id, t.webview.webview()))
}

fn call_cdp(
    core: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    method: &str,
    params: &Value,
    reply: Sender<Value>,
) {
    let sender = reply.clone();
    let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, text| {
        // Con error, WebView2 devuelve igual el JSON del error de CDP.
        let parsed = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
        let response = match result {
            Ok(()) => json!({ "ok": true, "result": parsed }),
            Err(e) => {
                let message = parsed
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| e.message().to_string());
                json!({ "ok": false, "error": message })
            }
        };
        let _ = sender.send(response);
        Ok(())
    }));
    let started = unsafe {
        core.CallDevToolsProtocolMethod(&HSTRING::from(method), &HSTRING::from(params.to_string()), &handler)
    };
    if let Err(e) = started {
        let _ = reply.send(error(&e.message()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pipe_only_reaches_the_web_and_local_files() {
        assert!(is_allowed_target("https://example.com/"));
        assert!(is_allowed_target("http://localhost:3000/"));
        assert!(is_allowed_target("file:///C:/Users/ana/notas.html"));
        assert!(!is_allowed_target("file://servidor/carpeta/x.html"));
        assert!(!is_allowed_target("http://app.localhost/settings"));
        assert!(!is_allowed_target("app://localhost/settings"));
        assert!(!is_allowed_target("javascript:alert(1)"));
        assert!(!is_allowed_target("data:text/html,hola"));
        assert!(!is_allowed_target("chrome-extension://abc/opciones.html"));

        assert!(navigation_allowed("Runtime.evaluate", &json!({ "expression": "1" })));
        assert!(navigation_allowed("Page.navigate", &json!({ "url": "https://example.com/" })));
        assert!(!navigation_allowed("Page.navigate", &json!({ "url": "http://app.localhost/settings" })));
        assert!(!navigation_allowed("Page.navigate", &json!({})));
    }
}
