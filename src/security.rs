//! Medidas de seguridad del navegador.
//!
//! Lo que ya resuelve el motor (Chromium, via WebView2) no se toca: procesos
//! en sandbox, aislamiento de sitios, politica de mismo origen, HSTS, bloqueo
//! de contenido mixto, validacion de certificados, actualizaciones del motor
//! (WebView2 Evergreen se actualiza solo con Windows Update). Aca esta lo que
//! depende de la aplicacion:
//!
//! - Proceso: DLL solo desde ubicaciones seguras (`harden_process`).
//! - Todos los WebView: los mismos argumentos del motor, con SmartScreen
//!   activo (wry lo apaga por defecto) y sin "host objects" (`base_builder`,
//!   `harden`).
//! - Interfaz (barra, paneles, menu): no pueden salir de sus paginas propias
//!   (`chrome_builder`); su IPC solo se acepta desde esas paginas (ipc.rs).
//! - Pestanas (`attach_tab`): HTTPS primero, errores de certificado,
//!   ventanas emergentes, aplicaciones externas peligrosas, barra de estado
//!   con el destino real de los enlaces, window.close().
//! - Paginas propias: CSP con nonce, sin iframes ajenos (protocol.rs).
//!
//! Permisos de sitios: permissions.rs. Descargas peligrosas: downloads.rs.
//! Firma de extensiones: crx.rs y extensions.rs.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde::Serialize;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    ContentLoadingEventHandler, LaunchingExternalUriSchemeEventHandler, NavigationCompletedEventHandler,
    NavigationStartingEventHandler, NewWindowRequestedEventHandler, ServerCertificateErrorDetectedEventHandler,
    WindowCloseRequestedEventHandler,
};
use windows_core::{Interface, BOOL};
use wry::{WebView, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows};

use crate::native::{self, read_pwstr, ClearKinds};
use crate::state::{is_own_page, own_page_path, Action, AppState, Shared, INSECURE_URL};
use crate::{protocol, storage, sync};

/// Argumentos del motor. Tienen que ser LOS MISMOS en todos los WebView:
/// comparten proceso y WebView2 se niega a crear uno con otras opciones. Si
/// no le pasamos ninguno, wry usa "--disable-features=msWebOOUI,msPdfOOUI,
/// msSmartScreenProtection", que apaga SmartScreen (el filtro de sitios de
/// phishing y malware, y de descargas peligrosas). Dejamos solo lo otro: el
/// mini menu que aparece al seleccionar texto.
pub const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI";

/// Cuanto esperar a que un sitio responda por HTTPS antes de volver a HTTP:
/// sin esto, un sitio que no atiende el puerto 443 (los paquetes se pierden,
/// no se rechazan) dejaria la pestana cargando hasta que venza la conexion.
const UPGRADE_TIMEOUT: Duration = Duration::from_secs(10);
/// Cuanto esperar a que termine "borrar datos al cerrar" antes de salir igual.
const QUIT_TIMEOUT: Duration = Duration::from_secs(4);

/// Esquemas de aplicaciones externas que una pagina nunca puede abrir, ni
/// siquiera preguntando: los que se usaron para ejecutar codigo o meter
/// malware en el equipo.
const BLOCKED_SCHEMES: &[&str] = &[
    // Follina (CVE-2022-30190): ejecucion de codigo via la herramienta de diagnostico.
    "ms-msdt",
    // Abren el buscador de Windows sobre una carpeta remota: entrega de malware.
    "search-ms",
    "search",
    // Lanzador de Office: ejecucion remota de codigo (2021).
    "ms-officecmd",
    // Instalar paquetes .appx/.msix desde la web: Microsoft lo apago por el abuso.
    "ms-appinstaller",
    // Centro de ayuda y archivos de ayuda .chm: viejas vias de ejecucion de codigo.
    "hcp",
    "its",
    "ms-its",
    "mk",
    // Java Web Start: ejecuta aplicaciones descargadas.
    "jnlp",
    "jnlps",
];

// ---------------------------------------------------------------------------
// Estado por pestana
// ---------------------------------------------------------------------------

/// Estado de seguridad de una pestana (ver attach_tab).
#[derive(Default)]
pub struct TabSecurity {
    /// Intento en curso de abrir por HTTPS una direccion http:// (ver https_first).
    pub upgrade: Option<Upgrade>,
    /// Direccion http:// que espera la decision del usuario en la pagina de
    /// aviso (modo estricto, ver INSECURE_URL).
    pub insecure_target: Option<String>,
    /// URL de la navegacion principal en curso: un error de certificado es
    /// "de la pagina" si es de su mismo origen (ver on_certificate_error).
    pub nav_target: String,
    /// Lo que se le bloqueo a la pagina actual (se vacia al cambiar de pagina).
    pub blocked: Vec<Blocked>,
    /// La pagina actual es una de error del motor (no se pudo abrir el
    /// sitio): no va al historial (ver tabs::open_tab). Pasa seguido con
    /// HTTPS primero: el intento por HTTPS de un sitio que no lo tiene.
    pub error_page: bool,
}

pub struct Upgrade {
    pub host: String,
    pub http_url: String,
    /// Id de la navegacion por HTTPS y cuando arranco, una vez que arranco.
    pub nav_id: Option<u64>,
    pub started: Option<Instant>,
    /// El intento termino en un error de certificado (ver on_certificate_error).
    pub failed: bool,
}

/// Algo que la pagina actual intento y no se le dejo hacer. La barra
/// superior lo muestra con un escudo (y una ventana emergente se puede abrir
/// igual desde ahi).
#[derive(Clone, Serialize)]
#[serde(tag = "kind", content = "target", rename_all = "lowercase")]
pub enum Blocked {
    /// Ventana emergente abierta sin un clic del usuario (su URL).
    Popup(String),
    /// Aplicacion externa (su esquema, ej. "ms-msdt:").
    App(String),
}

/// Lo bloqueado que se recuerda por pagina (los mas nuevos).
const MAX_BLOCKED: usize = 10;

// ---------------------------------------------------------------------------
// Proceso y WebViews
// ---------------------------------------------------------------------------

/// Endurece el proceso del navegador. Lo primero que corre en main().
#[cfg(windows)]
pub fn harden_process() {
    use windows_sys::Win32::System::LibraryLoader::{
        SetDefaultDllDirectories, SetDllDirectoryW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
    };
    use windows_sys::Win32::System::SystemServices::PROCESS_MITIGATION_IMAGE_LOAD_POLICY;
    use windows_sys::Win32::System::Threading::{ProcessImageLoadPolicy, SetProcessMitigationPolicy};

    unsafe {
        // Las DLL se buscan solo en la carpeta del programa y en System32. Por
        // defecto Windows tambien mira la carpeta actual y el PATH: un .dll
        // con el nombre justo dejado ahi (por ejemplo en Descargas, si el
        // navegador se abre desde esa carpeta) se cargaria adentro del
        // navegador ("DLL planting").
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
        let empty = [0u16];
        SetDllDirectoryW(empty.as_ptr());

        // Nunca cargar codigo desde una carpeta de red (UNC/WebDAV) ni
        // archivos marcados de integridad baja (los que escribe un proceso
        // en sandbox).
        let mut policy = PROCESS_MITIGATION_IMAGE_LOAD_POLICY::default();
        policy.Anonymous.Flags = 0b11; // NoRemoteImages | NoLowMandatoryLabelImages
        SetProcessMitigationPolicy(
            ProcessImageLoadPolicy,
            &policy as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&policy),
        );
    }
}

#[cfg(not(windows))]
pub fn harden_process() {}

/// Punto de partida de TODOS los WebView (ver BROWSER_ARGS).
pub fn base_builder<'a>() -> WebViewBuilder<'a> {
    WebViewBuilder::new()
        // Todos los WebView comparten carpeta de datos, y WebView2 exige que
        // todos tengan el mismo valor aca (ver extensions.rs).
        .with_browser_extensions_enabled(true)
        .with_additional_browser_args(BROWSER_ARGS)
}

/// WebView que muestra nuestras paginas del protocolo app:// (pestanas e
/// interfaz).
pub fn app_builder<'a>() -> WebViewBuilder<'a> {
    base_builder().with_asynchronous_custom_protocol("app".into(), |_webview_id, request, responder| {
        protocol::handle(request, responder)
    })
}

/// Barra superior, paneles y menu: nunca salen de su pagina propia. Ni
/// soltando encima un enlace o un archivo arrastrado desde una pestana, ni
/// por nada que se ejecute adentro: una pagina ajena ahi tendria todos los
/// comandos de ipc::dispatch (cerrar pestanas, instalar extensiones...).
pub fn chrome_builder<'a>() -> WebViewBuilder<'a> {
    app_builder().with_navigation_handler(|url| {
        let ok = own_page_path(&url).is_some();
        if !ok {
            eprintln!("[seguridad] la interfaz no puede navegar a {url}");
        }
        ok
    })
}

/// Lo comun a todos los WebView, recien creados.
pub fn harden(state: &Shared, webview: &WebView) {
    if let Ok(settings) = unsafe { webview.webview().Settings() } {
        // Ninguna pagina usa "host objects" (objetos nativos accesibles desde
        // JS): sin ellos hay menos superficie expuesta a las paginas.
        let _ = unsafe { settings.SetAreHostObjectsAllowed(false) };
    }
    set_smartscreen(webview, state.borrow().settings.smartscreen);
}

/// SmartScreen queda activo si CUALQUIER WebView del perfil lo pide, asi que
/// se aplica a todos (interfaz incluida), no solo a las pestanas.
fn set_smartscreen(webview: &WebView, on: bool) {
    let settings8 = unsafe { webview.webview().Settings() }.and_then(|s| s.cast::<ICoreWebView2Settings8>());
    if let Ok(s8) = settings8 {
        let _ = unsafe { s8.SetIsReputationCheckingRequired(on) };
    }
}

/// Si Windows tiene apagado "SmartScreen para Microsoft Edge" (Seguridad de
/// Windows > Control de aplicaciones y navegador > Proteccion basada en la
/// reputacion). WebView2 obedece a esa opcion del sistema: con ella apagada
/// SmartScreen no actua, diga lo que diga el ajuste del navegador, y la
/// pagina de ajustes lo avisa.
#[cfg(windows)]
pub fn smartscreen_off_in_windows() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let key: Vec<u16> = "Software\\Microsoft\\Edge\\SmartScreenEnabled\0".encode_utf16().collect();
    let mut value = 1u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            std::ptr::null(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut value as *mut u32 as *mut core::ffi::c_void,
            &mut size,
        )
    };
    // Sin la clave, Windows lo deja encendido.
    status == 0 && value == 0
}

#[cfg(not(windows))]
pub fn smartscreen_off_in_windows() -> bool {
    false
}

/// Ajustes > SmartScreen: lo aplica a todos los WebView abiertos.
pub fn apply_smartscreen(state: &Shared) {
    let st = state.borrow();
    let on = st.settings.smartscreen;
    let chrome = [&st.topbar, &st.left_panel, &st.right_panel, &st.menu].into_iter().flatten();
    let tabs = st.tabs.iter().map(|t| &t.webview);
    let popup = st.ext_popup.iter().map(|p| &p.webview);
    for webview in chrome.chain(tabs).chain(popup) {
        set_smartscreen(webview, on);
    }
}

/// Ajustes > Prevencion de seguimiento (es del perfil: vale para todos).
pub fn apply_tracking_prevention(state: &Shared) {
    let st = state.borrow();
    let Some(topbar) = &st.topbar else { return };
    let level = match st.settings.tracking_prevention.as_str() {
        "off" => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_NONE,
        "basic" => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_BASIC,
        "strict" => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_STRICT,
        _ => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_BALANCED,
    };
    if let Some(profile3) = native::profile(topbar).and_then(|p| p.cast::<ICoreWebView2Profile3>().ok()) {
        let _ = unsafe { profile3.SetPreferredTrackingPreventionLevel(level) };
    }
}

// ---------------------------------------------------------------------------
// Pestanas
// ---------------------------------------------------------------------------

/// Engancha las medidas de seguridad de una pestana recien creada (antes de
/// que llegue el primer evento de su navegacion inicial).
pub fn attach_tab(state: &Shared, webview: &WebView, id: u64) {
    harden(state, webview);
    let core = webview.webview();
    if let Ok(settings) = unsafe { core.Settings() } {
        // Barra de estado (abajo a la izquierda) con la direccion real de un
        // enlace al pasar el cursor: wry la apaga en todos los WebView, pero
        // es la forma de ver a donde lleva un enlace antes de hacer clic.
        let _ = unsafe { settings.SetIsStatusBarEnabled(true) };
        // Una pagina no puede declarar zonas que arrastren o redimensionen la
        // ventana del navegador (CSS app-region).
        if let Ok(settings9) = settings.cast::<ICoreWebView2Settings9>() {
            let _ = unsafe { settings9.SetIsNonClientRegionSupportEnabled(false) };
        }
    }
    on_navigation_starting(state, &core, id);
    on_content_loading(state, &core, id);
    on_navigation_completed(state, &core, id);
    on_certificate_error(state, &core, id);
    on_new_window(state, &core, id);
    on_external_scheme(state, &core, id);
    on_window_close(state, &core, id);
}

/// HTTPS primero (como "Usar siempre conexiones seguras" de Chrome): toda
/// navegacion nueva a http:// de un sitio publico se cancela y se intenta
/// por https://. Si el sitio no responde por HTTPS (error de conexion, de
/// certificado, demora, o redirige de vuelta a HTTP), en modo "upgrade" se
/// abre por HTTP y ese host queda permitido por la sesion; en modo "strict"
/// se muestra antes la pagina de aviso (INSECURE_URL).
fn on_navigation_starting(state: &Shared, core: &ICoreWebView2, id: u64) {
    let s = state.clone();
    let handler = NavigationStartingEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let uri = unsafe { read_pwstr(|p| args.Uri(p)) };
        let mut nav_id = 0u64;
        let mut redirected = BOOL(0);
        unsafe {
            args.NavigationId(&mut nav_id)?;
            args.IsRedirected(&mut redirected)?;
        }
        // Atras/adelante/recargar no se desvian: esa entrada ya se abrio antes.
        let new_document = args
            .cast::<ICoreWebView2NavigationStartingEventArgs3>()
            .ok()
            .map(|a| {
                let mut kind = COREWEBVIEW2_NAVIGATION_KIND_NEW_DOCUMENT;
                let _ = unsafe { a.NavigationKind(&mut kind) };
                kind == COREWEBVIEW2_NAVIGATION_KIND_NEW_DOCUMENT
            })
            .unwrap_or(true);
        if https_first(&s, id, &uri, nav_id, redirected.as_bool(), new_document) {
            unsafe { args.SetCancel(true)? };
        }
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { core.add_NavigationStarting(&handler, &mut token) };
}

/// Decide una navegacion que arranca (ver on_navigation_starting). Devuelve
/// si hay que cancelarla.
fn https_first(state: &Shared, id: u64, uri: &str, nav_id: u64, redirected: bool, new_document: bool) -> bool {
    enum Step {
        Allow,
        /// Cancelar y probar por HTTPS.
        Upgrade(String, String),
        /// Nuestro intento por HTTPS redirige a HTTP: no hay HTTPS de verdad.
        RedirectedBack(Upgrade),
    }

    let mut st = state.borrow_mut();
    let st = &mut *st;
    let mode = st.settings.https_mode.clone();
    let step = {
        let Some(tab) = st.tabs.iter_mut().find(|t| t.id == id) else { return false };
        let sec = &mut tab.sec;
        sec.nav_target = uri.to_string();
        let mut step = Step::Allow;
        if let Some(up) = &mut sec.upgrade {
            // Por host y no por URL exacta: el motor puede normalizar la URL.
            let ours = uri.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://"))
                && host_of(uri).as_deref() == Some(up.host.as_str());
            if up.nav_id.is_none() && ours {
                // Arranca nuestro propio intento por HTTPS.
                up.nav_id = Some(nav_id);
                up.started = Some(Instant::now());
                return false;
            }
            if up.nav_id == Some(nav_id) && redirected && https_upgrade(uri).is_some_and(|(host, _)| host == up.host) {
                step = Step::RedirectedBack(sec.upgrade.take().unwrap());
            } else if up.nav_id != Some(nav_id) {
                // Otra navegacion reemplazo al intento.
                sec.upgrade = None;
            }
        }
        if matches!(step, Step::Allow) && mode != "off" && new_document {
            if let Some((host, https_url)) = https_upgrade(uri) {
                if !st.http_allowed.contains(&host) {
                    step = Step::Upgrade(host, https_url);
                }
            }
        }
        step
    };

    match step {
        Step::Allow => false,
        Step::Upgrade(host, https_url) => {
            if let Some(tab) = st.tabs.iter_mut().find(|t| t.id == id) {
                tab.sec.upgrade = Some(Upgrade {
                    host,
                    http_url: uri.to_string(),
                    nav_id: None,
                    started: None,
                    failed: false,
                });
            }
            st.request(Action::LoadUrl(id, https_url));
            true
        }
        Step::RedirectedBack(up) => {
            eprintln!("[seguridad] {} redirige de HTTPS a HTTP", up.host);
            // La navegacion a la direccion http:// ya esta en curso (es la
            // redireccion): en modo normal alcanza con dejarla seguir.
            fall_back(st, id, mode == "strict", up.host, uri.to_string(), true)
        }
    }
}

/// `host` no tiene HTTPS. Modo normal: se abre `http_url` y el host queda
/// permitido por la sesion. Modo estricto: pagina de aviso, donde el usuario
/// decide. `in_flight`: la navegacion a `http_url` ya esta en curso. Devuelve
/// si hay que cancelar esa navegacion en curso.
fn fall_back(st: &mut AppState, id: u64, strict: bool, host: String, http_url: String, in_flight: bool) -> bool {
    if strict {
        if let Some(tab) = st.tabs.iter_mut().find(|t| t.id == id) {
            tab.sec.insecure_target = Some(http_url);
        }
        st.request(Action::LoadUrl(id, INSECURE_URL.into()));
        return true;
    }
    st.http_allowed.insert(host);
    if !in_flight {
        st.request(Action::LoadUrl(id, http_url));
    }
    false
}

/// Pagina de aviso > "Continuar al sitio": el usuario acepta abrirlo por HTTP.
pub fn proceed_insecure(state: &Shared, id: u64) {
    let mut st = state.borrow_mut();
    let target = st.tabs.iter().find(|t| t.id == id).and_then(|t| t.sec.insecure_target.clone());
    let Some(url) = target else { return };
    if let Some(host) = host_of(&url) {
        st.http_allowed.insert(host);
    }
    st.request(Action::LoadUrl(id, url));
}

/// Los datos de la pagina de aviso de esa pestana.
pub fn insecure_info(state: &Shared, id: u64) -> Option<(String, String)> {
    let st = state.borrow();
    let url = st.tabs.iter().find(|t| t.id == id)?.sec.insecure_target.clone()?;
    let host = host_of(&url).unwrap_or_default();
    Some((url, host))
}

/// La pagina de una navegacion empezo a cargar: lo bloqueado en la anterior
/// ya no aplica, y si era nuestro intento por HTTPS, el sitio respondio.
fn on_content_loading(state: &Shared, core: &ICoreWebView2, id: u64) {
    let s = state.clone();
    let handler = ContentLoadingEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let mut nav_id = 0u64;
        let mut error_page = BOOL(0);
        unsafe {
            args.NavigationId(&mut nav_id)?;
            args.IsErrorPage(&mut error_page)?;
        }
        let had_blocked = {
            let mut st = s.borrow_mut();
            let Some(tab) = st.tabs.iter_mut().find(|t| t.id == id) else { return Ok(()) };
            let sec = &mut tab.sec;
            sec.error_page = error_page.as_bool();
            if !error_page.as_bool() && sec.upgrade.as_ref().is_some_and(|up| up.nav_id == Some(nav_id)) {
                sec.upgrade = None;
            }
            let had = !sec.blocked.is_empty();
            sec.blocked.clear();
            had
        };
        if had_blocked {
            sync::push_active_tab(&s);
        }
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { core.add_ContentLoading(&handler, &mut token) };
}

/// Fin de nuestro intento por HTTPS: si fallo por algo que indica que el
/// sitio no tiene HTTPS, se vuelve a HTTP (o al aviso).
fn on_navigation_completed(state: &Shared, core: &ICoreWebView2, id: u64) {
    let s = state.clone();
    let handler = NavigationCompletedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let mut ok = BOOL(0);
        let mut status = COREWEBVIEW2_WEB_ERROR_STATUS_UNKNOWN;
        let mut nav_id = 0u64;
        unsafe {
            args.IsSuccess(&mut ok)?;
            args.WebErrorStatus(&mut status)?;
            args.NavigationId(&mut nav_id)?;
        }
        let mut st = s.borrow_mut();
        let st = &mut *st;
        let Some(tab) = st.tabs.iter_mut().find(|t| t.id == id) else { return Ok(()) };
        if !tab.sec.upgrade.as_ref().is_some_and(|up| up.nav_id == Some(nav_id)) {
            return Ok(());
        }
        let up = tab.sec.upgrade.take().unwrap();
        if !ok.as_bool() && (up.failed || https_unavailable(status)) {
            eprintln!("[seguridad] {} no responde por HTTPS", up.host);
            let strict = st.settings.https_mode == "strict";
            fall_back(st, id, strict, up.host, up.http_url, false);
        }
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { core.add_NavigationCompleted(&handler, &mut token) };
}

/// Intentos por HTTPS que no respondieron en UPGRADE_TIMEOUT: se cortan y
/// se vuelve a HTTP (o al aviso). Se llama en cada vuelta del bucle de
/// eventos; devuelve cuando hay que volver a mirar.
pub fn tick(state: &Shared) -> Option<Instant> {
    let now = Instant::now();
    let mut next: Option<Instant> = None;
    let mut expired = Vec::new();
    {
        let mut st = state.borrow_mut();
        for tab in st.tabs.iter_mut() {
            let Some(started) = tab.sec.upgrade.as_ref().and_then(|up| up.started) else { continue };
            let deadline = started + UPGRADE_TIMEOUT;
            if now >= deadline {
                let up = tab.sec.upgrade.take().unwrap();
                expired.push((tab.id, up, tab.webview.webview()));
            } else {
                next = Some(next.map_or(deadline, |n| n.min(deadline)));
            }
        }
    }
    for (id, up, core) in expired {
        eprintln!("[seguridad] {} no respondio por HTTPS a tiempo", up.host);
        // Corta la carga; su NavigationCompleted ya no encuentra el intento.
        let _ = unsafe { core.Stop() };
        let mut st = state.borrow_mut();
        let strict = st.settings.https_mode == "strict";
        fall_back(&mut st, id, strict, up.host, up.http_url, false);
    }
    next
}

/// Errores de certificado. Si es nuestro intento por HTTPS, el sitio no
/// tiene HTTPS de verdad: se cancela sin aviso y se vuelve a HTTP. Si es la
/// pagina que se esta abriendo, WebView2 muestra su aviso (y no deja seguir
/// si el sitio usa HSTS) y la barra lo marca como no seguro.
fn on_certificate_error(state: &Shared, core: &ICoreWebView2, id: u64) {
    let Ok(core14) = core.cast::<ICoreWebView2_14>() else { return };
    let s = state.clone();
    let handler = ServerCertificateErrorDetectedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let uri = unsafe { read_pwstr(|p| args.RequestUri(p)) };
        let host = host_of(&uri);
        let cancel = {
            let mut st = s.borrow_mut();
            let st = &mut *st;
            let Some(tab) = st.tabs.iter_mut().find(|t| t.id == id) else { return Ok(()) };
            match &mut tab.sec.upgrade {
                Some(up) if up.nav_id.is_some() && host.as_deref() == Some(up.host.as_str()) => {
                    up.failed = true;
                    true
                }
                _ => {
                    if origin_of(&uri).is_some() && origin_of(&uri) == origin_of(&tab.sec.nav_target) {
                        if let Some(host) = host {
                            st.cert_errors.insert(host);
                        }
                    }
                    false
                }
            }
        };
        if cancel {
            unsafe { args.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL)? };
        }
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { core14.add_ServerCertificateErrorDetected(&handler, &mut token) };
}

/// window.open y enlaces target=_blank. Nunca una ventana del sistema: una
/// pestana nueva, sin relacion con la pagina que la abrio (window.opener es
/// null, asi que esa pagina no puede redirigir la pestana nueva). Si la abre
/// un clic del usuario, se abre siempre; si la pagina la abre por su cuenta,
/// solo si no estan bloqueadas las ventanas emergentes (si lo estan, queda
/// anotada para abrirla desde el escudo de la barra).
fn on_new_window(state: &Shared, core: &ICoreWebView2, id: u64) {
    let s = state.clone();
    let handler = NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let uri = unsafe { read_pwstr(|p| args.Uri(p)) };
        let mut user = BOOL(0);
        unsafe {
            args.IsUserInitiated(&mut user)?;
            // window.open() devuelve null: nada de ventanas del sistema.
            args.SetHandled(true)?;
        }
        let opener = s.borrow().tabs.iter().find(|t| t.id == id).map(|t| t.url.clone()).unwrap_or_default();
        if !popup_target_allowed(&uri, &opener) {
            eprintln!("[seguridad] una pagina no puede abrir {uri} en una pestana");
            return Ok(());
        }
        let blocked = {
            let mut st = s.borrow_mut();
            if user.as_bool() || !st.settings.block_popups {
                st.request_tab(&uri);
                false
            } else {
                eprintln!("[seguridad] ventana emergente bloqueada: {uri}");
                record_blocked(&mut st, id, Blocked::Popup(uri));
                true
            }
        };
        if blocked {
            sync::push_active_tab(&s);
        }
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { core.add_NewWindowRequested(&handler, &mut token) };
}

/// Enlaces a aplicaciones externas (mailto:, zoommtg:, ms-settings:...).
/// WebView2 ya pregunta antes de abrir una; ademas, se bloquean siempre los
/// esquemas peligrosos (BLOCKED_SCHEMES) y los que una pagina intenta abrir
/// sin un clic del usuario.
fn on_external_scheme(state: &Shared, core: &ICoreWebView2, id: u64) {
    let Ok(core18) = core.cast::<ICoreWebView2_18>() else { return };
    let s = state.clone();
    let handler = LaunchingExternalUriSchemeEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let uri = unsafe { read_pwstr(|p| args.Uri(p)) };
        let mut user = BOOL(0);
        unsafe { args.IsUserInitiated(&mut user)? };
        if is_blocked_scheme(&uri) || !user.as_bool() {
            unsafe { args.SetCancel(true)? };
            let scheme = format!("{}:", uri.split(':').next().unwrap_or_default());
            eprintln!("[seguridad] bloqueado: una pagina intento abrir {scheme}");
            record_blocked(&mut s.borrow_mut(), id, Blocked::App(scheme));
            sync::push_active_tab(&s);
        }
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { core18.add_LaunchingExternalUriScheme(&handler, &mut token) };
}

/// window.close() desde la pagina: wry destruye en el acto la ventana del
/// WebView, y la pestana quedaba en el estado como un hueco vacio. Se cierra
/// la pestana de verdad (diferido: no se puede destruir el WebView dentro de
/// su propio callback).
fn on_window_close(state: &Shared, core: &ICoreWebView2, id: u64) {
    let s = state.clone();
    let handler = WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
        s.borrow_mut().request(Action::CloseTab(id));
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { core.add_WindowCloseRequested(&handler, &mut token) };
}

fn record_blocked(st: &mut AppState, id: u64, item: Blocked) {
    if let Some(tab) = st.tabs.iter_mut().find(|t| t.id == id) {
        tab.sec.blocked.push(item);
        let excess = tab.sec.blocked.len().saturating_sub(MAX_BLOCKED);
        tab.sec.blocked.drain(..excess);
    }
}

/// Escudo de la barra > abrir la ultima ventana emergente bloqueada de la
/// pestana activa.
pub fn open_blocked_popup(state: &Shared) {
    {
        let mut st = state.borrow_mut();
        let active = st.active;
        let Some(tab) = st.tabs.get_mut(active) else { return };
        let Some(i) = tab.sec.blocked.iter().rposition(|b| matches!(b, Blocked::Popup(_))) else { return };
        let Blocked::Popup(url) = tab.sec.blocked.remove(i) else { return };
        st.request_tab(&url);
    }
    sync::push_active_tab(state);
}

// ---------------------------------------------------------------------------
// Salir
// ---------------------------------------------------------------------------

/// Cerrar el navegador. Con "Borrar datos al cerrar" primero se borra todo
/// (lo de WebView2 es asincrono: se sale cuando termina, o a los
/// QUIT_TIMEOUT si no termina).
pub fn quit(state: &Shared) {
    if !state.borrow().settings.clear_on_exit {
        std::process::exit(0);
    }
    let window = {
        let mut st = state.borrow_mut();
        if st.quit_deadline.is_some() {
            return;
        }
        st.quit_deadline = Some(Instant::now() + QUIT_TIMEOUT);
        st.history.clear();
        st.closed_tabs.clear();
        st.downloads.clear();
        st.save_history();
        st.window.clone()
    };
    storage::remove(storage::SESSION_FILE);
    crate::suggest::forget_favicons(state);
    // Sin prestamos vivos: ocultar la ventana puede disparar eventos de winit.
    if let Some(window) = window {
        window.set_visible(false);
    }
    let st = state.borrow();
    match &st.topbar {
        Some(topbar) => native::clear_browsing_data(topbar, ClearKinds { cookies: true, cache: true }, |_| {
            std::process::exit(0)
        }),
        None => std::process::exit(0),
    }
    st.wake();
}

/// Si ya vencio la espera de quit(): sale. Si no, cuando volver a mirar.
pub fn quit_deadline(state: &Shared) -> Option<Instant> {
    let deadline = state.borrow().quit_deadline?;
    if Instant::now() >= deadline {
        std::process::exit(0);
    }
    Some(deadline)
}

/// Al arrancar con "Borrar datos al cerrar": si la ultima vez el navegador
/// no llego a borrar (se corto la luz, se colgo), se borra ahora.
pub fn clear_leftovers(state: &Shared) {
    let st = state.borrow();
    if !st.settings.clear_on_exit {
        return;
    }
    if let Some(topbar) = &st.topbar {
        native::clear_browsing_data(topbar, ClearKinds { cookies: true, cache: true }, |_| {});
    }
}

// ---------------------------------------------------------------------------
// Reglas (funciones puras)
// ---------------------------------------------------------------------------

/// Una direccion http:// que conviene intentar primero por HTTPS: (host, la
/// misma direccion con https://). Solo sitios publicos en el puerto por
/// defecto: con otro puerto, HTTPS casi nunca esta en ese mismo puerto.
pub fn https_upgrade(url: &str) -> Option<(String, String)> {
    if !url.get(..7)?.eq_ignore_ascii_case("http://") {
        return None;
    }
    let rest = &url[7..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((user, hostport)) => (format!("{user}@"), hostport),
        None => (String::new(), authority),
    };
    let (host, port) = split_port(hostport);
    if port.is_some_and(|p| p != "80") || !is_public_host(host) {
        return None;
    }
    let host = host.to_ascii_lowercase();
    let https = format!("https://{userinfo}{host}{tail}");
    Some((host, https))
}

/// Si un host es un dominio publico de Internet (los unicos que se pasan a
/// HTTPS): no localhost, ni una IP, ni un nombre de una sola palabra de la
/// red local, ni un dominio reservado para redes internas o pruebas.
pub fn is_public_host(host: &str) -> bool {
    const INTERNAL: &[&str] = &[
        "localhost", "local", "lan", "internal", "intranet", "corp", "home", "home.arpa", "localdomain", "test",
        "example", "invalid",
    ];
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    if h.is_empty() || h.starts_with('[') || !h.contains('.') || h.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return false;
    }
    !INTERNAL.iter().any(|tld| h == *tld || h.ends_with(&format!(".{tld}")))
}

/// Errores con los que un intento por HTTPS cuenta como "el sitio no tiene
/// HTTPS". No los de "no hay Internet" o "el dominio no existe" (por HTTP
/// tampoco andaria) ni una navegacion cancelada.
pub fn https_unavailable(status: COREWEBVIEW2_WEB_ERROR_STATUS) -> bool {
    [
        COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_COMMON_NAME_IS_INCORRECT,
        COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_EXPIRED,
        COREWEBVIEW2_WEB_ERROR_STATUS_CLIENT_CERTIFICATE_CONTAINS_ERRORS,
        COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_REVOKED,
        COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_IS_INVALID,
        COREWEBVIEW2_WEB_ERROR_STATUS_SERVER_UNREACHABLE,
        COREWEBVIEW2_WEB_ERROR_STATUS_TIMEOUT,
        COREWEBVIEW2_WEB_ERROR_STATUS_ERROR_HTTP_INVALID_SERVER_RESPONSE,
        COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED,
        COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_RESET,
        COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT,
        COREWEBVIEW2_WEB_ERROR_STATUS_REDIRECT_FAILED,
        COREWEBVIEW2_WEB_ERROR_STATUS_UNEXPECTED_ERROR,
        COREWEBVIEW2_WEB_ERROR_STATUS_UNKNOWN,
    ]
    .contains(&status)
}

/// Si una pagina puede abrir `target` en una pestana nueva. Solo direcciones
/// web normales: nunca nuestras paginas internas, archivos locales, data: o
/// javascript:. Chromium no deja que una pagina navegue a esas por su cuenta,
/// y la pestana nueva la abrimos nosotros (para el motor, la abrio el
/// usuario): sin este filtro una pagina se las saltearia. Una pagina de
/// extension si puede abrir otras de la MISMA extension.
pub fn popup_target_allowed(target: &str, opener: &str) -> bool {
    if is_own_page(target) {
        return false;
    }
    let lower = target.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") || lower == "about:blank" || lower.starts_with("blob:") {
        return true;
    }
    lower.starts_with("chrome-extension://") && origin_of(target).is_some() && origin_of(target) == origin_of(opener)
}

/// Una URL que se puede abrir desde un marcador o el historial: nunca
/// codigo (javascript:, vbscript:) ni una pagina armada dentro de la propia
/// URL (data:, la forma clasica de disfrazar una pagina de phishing).
pub fn is_safe_to_open(url: &str) -> bool {
    let scheme = url.trim_start().split(':').next().unwrap_or_default().to_ascii_lowercase();
    !matches!(scheme.as_str(), "javascript" | "vbscript" | "data")
}

pub fn is_blocked_scheme(uri: &str) -> bool {
    let scheme = uri.split(':').next().unwrap_or_default().to_ascii_lowercase();
    BLOCKED_SCHEMES.contains(&scheme.as_str())
}

/// Origen desde el que una pagina puede pedir permisos (camara, ubicacion...):
/// HTTPS, archivos locales, extensiones y el propio equipo (localhost).
/// Chromium ya exige un contexto seguro para casi todos; esto cierra la
/// puerta a cualquier otro.
pub fn is_secure_origin(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("file:") || lower.starts_with("chrome-extension://") {
        return true;
    }
    lower.starts_with("http://")
        && host_of(url).is_some_and(|h| h == "localhost" || h.ends_with(".localhost") || h == "127.0.0.1" || h == "[::1]")
}

/// Lo que muestra la barra de direcciones a la izquierda de la URL.
pub fn indicator(url: &str, cert_errors: &HashSet<String>) -> &'static str {
    if url.is_empty() || url == "about:blank" || is_own_page(url) {
        return "internal";
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") {
        let bad_cert = host_of(url).is_some_and(|h| cert_errors.contains(&h));
        return if bad_cert { "dangerous" } else { "secure" };
    }
    if lower.starts_with("http://") {
        return "insecure";
    }
    if lower.starts_with("file:") {
        return "file";
    }
    if lower.starts_with("chrome-extension://") {
        return "extension";
    }
    "other"
}

/// Host (en minusculas, sin usuario ni puerto) de una URL con "://".
pub fn host_of(url: &str) -> Option<String> {
    let (host, _) = split_port(hostport_of(url)?);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// "esquema://host:puerto" de una URL, para comparar origenes.
pub fn origin_of(url: &str) -> Option<String> {
    let scheme = url.split_once("://")?.0;
    let hostport = hostport_of(url)?;
    (!hostport.is_empty()).then(|| format!("{}://{}", scheme, hostport).to_ascii_lowercase())
}

fn hostport_of(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    Some(authority.rsplit_once('@').map_or(authority, |(_, hostport)| hostport))
}

/// "host:puerto" -> (host, puerto). Respeta las IPv6 entre corchetes.
fn split_port(hostport: &str) -> (&str, Option<&str>) {
    if hostport.starts_with('[') {
        return match hostport.find(']') {
            Some(i) => (&hostport[..=i], hostport[i + 1..].strip_prefix(':')),
            None => (hostport, None),
        };
    }
    match hostport.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (hostport, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrades_only_public_sites_on_the_default_port() {
        assert_eq!(
            https_upgrade("http://Example.com/a?b#c"),
            Some(("example.com".into(), "https://example.com/a?b#c".into()))
        );
        assert_eq!(https_upgrade("http://neverssl.com"), Some(("neverssl.com".into(), "https://neverssl.com".into())));
        assert_eq!(https_upgrade("http://ana@example.com:80/"), Some(("example.com".into(), "https://ana@example.com/".into())));
        assert_eq!(https_upgrade("https://example.com/"), None);
        assert_eq!(https_upgrade("http://example.com:8080/"), None);
        assert_eq!(https_upgrade("http://localhost:3000/"), None);
        assert_eq!(https_upgrade("http://app.localhost/settings"), None);
        assert_eq!(https_upgrade("http://192.168.0.1/"), None);
        assert_eq!(https_upgrade("http://[::1]/"), None);
        assert_eq!(https_upgrade("http://router/"), None);
        assert_eq!(https_upgrade("http://nas.local/"), None);
        assert_eq!(https_upgrade("http://printer.home.arpa/"), None);
        assert_eq!(https_upgrade("ftp://example.com/"), None);
    }

    #[test]
    fn popups_cannot_reach_internal_or_local_pages() {
        let web = "https://example.com/";
        assert!(popup_target_allowed("https://other.com/", web));
        assert!(popup_target_allowed("about:blank", web));
        assert!(!popup_target_allowed("http://app.localhost/settings", web));
        assert!(!popup_target_allowed("file:///C:/Windows/win.ini", web));
        assert!(!popup_target_allowed("data:text/html,<h1>banco</h1>", web));
        assert!(!popup_target_allowed("javascript:alert(1)", web));
        let ext = "chrome-extension://abcdefghijklmnopabcdefghijklmnop/opciones.html";
        assert!(!popup_target_allowed(ext, web));
        assert!(popup_target_allowed(ext, "chrome-extension://abcdefghijklmnopabcdefghijklmnop/popup.html"));
        assert!(!popup_target_allowed(ext, "chrome-extension://ppppppppppppppppppppppppppppppp/x.html"));
    }

    #[test]
    fn bookmarks_never_run_code() {
        assert!(is_safe_to_open("https://example.com/"));
        assert!(is_safe_to_open("file:///C:/notas.html"));
        assert!(!is_safe_to_open("javascript:alert(1)"));
        assert!(!is_safe_to_open(" JavaScript:alert(1)"));
        assert!(!is_safe_to_open("data:text/html,<h1>banco</h1>"));
    }

    #[test]
    fn dangerous_schemes_are_blocked() {
        assert!(is_blocked_scheme("ms-msdt:/id PCWDiagnostic"));
        assert!(is_blocked_scheme("SEARCH-MS:query=x&crumb=location:\\\\evil\\share"));
        assert!(!is_blocked_scheme("mailto:ana@example.com"));
        assert!(!is_blocked_scheme("zoommtg://zoom.us/join"));
    }

    #[test]
    fn permissions_only_for_secure_origins() {
        assert!(is_secure_origin("https://meet.example.com/"));
        assert!(is_secure_origin("http://localhost:8080/"));
        assert!(is_secure_origin("file:///C:/demo.html"));
        assert!(!is_secure_origin("http://example.com/"));
        assert!(!is_secure_origin("http://localhost.evil.com/"));
    }

    #[test]
    fn address_bar_indicator() {
        let mut bad = HashSet::new();
        bad.insert("expired.badssl.com".to_string());
        assert_eq!(indicator("https://example.com/", &bad), "secure");
        assert_eq!(indicator("https://expired.badssl.com/", &bad), "dangerous");
        assert_eq!(indicator("http://example.com/", &bad), "insecure");
        assert_eq!(indicator("http://app.localhost/", &bad), "internal");
        assert_eq!(indicator("https://evil.com/?app.localhost", &bad), "secure");
        assert_eq!(indicator("file:///C:/a.html", &bad), "file");
    }

    #[test]
    fn hosts_and_origins() {
        assert_eq!(host_of("https://user:pw@Example.com:8443/x").as_deref(), Some("example.com"));
        assert_eq!(host_of("http://[::1]:80/").as_deref(), Some("[::1]"));
        assert_eq!(origin_of("https://Example.com/a"), origin_of("https://example.com/b"));
        assert_ne!(origin_of("https://example.com/"), origin_of("http://example.com/"));
        assert_eq!(host_of("about:blank"), None);
    }
}
