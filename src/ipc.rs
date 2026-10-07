//! Comandos que nuestras paginas envian a Rust via
//! `window.ipc.postMessage(JSON.stringify({...}))`.
//!
//! Dos niveles de confianza:
//! - `dispatch`: la barra superior, los paneles laterales y el menu (nuestro
//!   HTML, en WebViews que nunca navegan a otro lado: ver
//!   security::chrome_builder). Todo permitido.
//! - `dispatch_from_content`: las PESTANAS. Ahi puede haber cualquier sitio,
//!   asi que solo se atiende a una pagina propia (pestana nueva, ajustes,
//!   aviso de HTTPS; ver state::is_own_page), y a cada una solo los comandos
//!   que necesita.
//!
//! En los dos casos se mira de donde viene cada mensaje: `source` es la URL
//! del documento que lo envio, que pone WebView2 y una pagina no puede
//! falsear. No alcanza con mirar en que pagina "esta" la pestana: mientras
//! un sitio navega hacia una pagina propia, el sitio sigue vivo un instante
//! y sus mensajes llegaban cuando la pestana ya figuraba en la pagina propia
//! (podia borrar los datos o quitar extensiones).

use serde::Deserialize;
use serde_json::Value;

use crate::native::ClearKinds;
use crate::state::{is_own_page, own_page_path, Action, Shared, Side, UserEvent, HOME_LOAD_URL, HOME_URL};
use crate::storage::{self, Settings};
use crate::{
    actions, downloads, extensions, import, menu, native, panels, perf, permissions, protocol, security, suggest, sync,
    tabs,
};

#[derive(Deserialize, Debug)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Navigate { url: String },
    /// Lo que se escribio en la barra de direcciones (URL o busqueda; ver
    /// storage::resolve_input).
    NavigateInput { input: String },
    Back,
    Forward,
    Reload,
    NewTab,
    /// Pestana nueva de incognito (ver Tab::incognito).
    NewIncognitoTab,
    CloseTab { id: u64 },
    SwitchTab { id: u64 },
    DragWindow,
    Minimize,
    ToggleMaximize,
    Close,
    ExpandLeft { value: bool },
    ExpandRight { value: bool },
    OpenDownloadsFolder,
    /// Muestra un archivo descargado en el Explorador (seleccionado).
    ShowDownload { id: u64 },
    /// "pause" | "resume" | "cancel" (ver downloads::control).
    DownloadControl { id: u64, action: String },
    ClearHistory,
    ToggleBookmark,
    RemoveBookmark { url: String },
    /// Acciones del menu "..." y atajos equivalentes.
    Menu { action: String },
    ToggleMenu,
    CloseMenu,
    /// "Agregar a Michi" en una pagina de Chrome Web Store / Edge Add-ons.
    InstallFromStore,
    /// Clic en el icono de una extension en la barra (ver popup.rs).
    ExtPopup { id: String, right: f64 },
    /// Escudo de la barra: abrir la ultima ventana emergente bloqueada.
    OpenBlockedPopup,
    /// Sugerencias de la barra (ver suggest.rs): cambio lo escrito (o se entro
    /// a la barra: texto vacio); `x`/`w`, donde esta la barra de direcciones.
    SuggestInput { text: String, deleting: bool, x: f64, w: f64 },
    /// Flecha abajo (+1) / arriba (-1).
    SuggestKey { delta: i32 },
    /// Enter: la fila elegida, o lo escrito.
    SuggestEnter { input: String },
    /// Clic en una fila del desplegable.
    SuggestPick { index: usize },
    SuggestClose,
    /// Una pagina avisa que ya cargo su JS y esta lista para recibir el primer
    /// estado. Evita la carrera entre el primer evaluate_script de Rust y el
    /// registro de window.onXxx en la pagina.
    Ready,

    // --- Paginas propias en pestanas (ver dispatch_from_content) ---
    /// Cambia el fondo animado compartido.
    SetTheme { id: String },
    /// Busqueda desde la pestana nueva (con el buscador de ajustes).
    Search { q: String },
    SetSetting { key: String, value: Value },
    ResetSettings,
    PickDownloadsDir,
    ClearData { history: bool, cookies: bool, cache: bool, downloads: bool },
    ExtInstallFile,
    ExtInstallFolder,
    ExtEnable { id: String, enabled: bool },
    ExtRemove { id: String },
    /// Abre la pagina de popup u opciones de una extension en una pestana.
    ExtOpen { id: String, page: String },
    /// Abre una URL en una pestana nueva (ej. "Ir a Chrome Web Store").
    OpenUrl { url: String },
    /// Bloquear (o volver a preguntar) un tipo de permiso para todos los sitios.
    SetPermissionDefault { kind: String, blocked: bool },
    /// Olvidar lo que se le permitio o bloqueo a un sitio (ver permissions.rs).
    ResetSitePermission { origin: String, kind: String },
    /// Pagina de aviso de HTTPS: abrir el sitio igual, o volver.
    InsecureProceed,
    InsecureBack,
    /// Buscar navegadores instalados con datos para importar (ver import.rs).
    ImportScan,
    /// Importar lo marcado del perfil `key` (uno de los que devolvio ImportScan).
    ImportRun {
        key: String,
        bookmarks: bool,
        history: bool,
        cookies: bool,
        tabs: bool,
    },
}

pub fn dispatch(state: &Shared, source: &str, raw: &str) {
    if own_page_path(source).is_none() {
        eprintln!("[seguridad] IPC ignorado: viene de {source}");
        return;
    }
    let cmd: Command = match serde_json::from_str(raw) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("[ipc] comando invalido `{raw}`: {err}");
            return;
        }
    };

    match cmd {
        Command::Navigate { url } => {
            // Marcadores e historial: nunca codigo (javascript:) ni paginas armadas (data:).
            if security::is_safe_to_open(&url) {
                with_active(state, |w| { let _ = w.load_url(&url); });
            }
        }
        Command::NavigateInput { input } => navigate_input(state, &input),
        Command::Back => with_active(state, |w| { let _ = w.go_back(); }),
        Command::Forward => with_active(state, |w| { let _ = w.go_forward(); }),
        Command::Reload => with_active(state, |w| { let _ = w.reload(); }),
        Command::NewTab => state.borrow_mut().request_tab(HOME_URL),
        Command::NewIncognitoTab => state.borrow_mut().request_tab_in(HOME_URL, true),
        Command::CloseTab { id } => tabs::close_tab(state, id),
        Command::SwitchTab { id } => {
            {
                let mut st = state.borrow_mut();
                if let Some(i) = st.tabs.iter().position(|t| t.id == id) {
                    st.active = i;
                }
            }
            tabs::activated(state);
        }
        // La ventana se clona y el prestamo de `state` se suelta ANTES de
        // tocarla: maximizar/minimizar disparan el Resized de winit en el
        // acto (dentro de esta llamada), y arrastrar corre un bucle modal de
        // Windows que sigue despachando eventos (IPC, titulos de pagina...)
        // mientras dura. Todos esos piden prestado `state`; con el prestamo
        // todavia tomado, el navegador se cerraba de golpe (RefCell ya
        // prestado) al maximizar o al arrastrar la ventana.
        Command::DragWindow => {
            if let Some(w) = window(state) {
                let _ = w.drag_window();
            }
        }
        Command::Minimize => {
            if let Some(w) = window(state) {
                w.set_minimized(true);
            }
        }
        Command::ToggleMaximize => {
            if let Some(w) = window(state) {
                w.set_maximized(!w.is_maximized());
            }
        }
        Command::Close => security::quit(state),
        Command::ExpandLeft { value } => panels::set_expanded(state, Side::Left, value),
        Command::ExpandRight { value } => panels::set_expanded(state, Side::Right, value),
        Command::OpenDownloadsFolder => {
            let dir = state.borrow().settings.downloads_dir();
            let _ = std::process::Command::new("explorer").arg(dir).spawn();
        }
        Command::ShowDownload { id } => {
            let path = state.borrow().downloads.iter().find(|d| d.id == id).and_then(|d| d.path.clone());
            // Mostrarlo en su carpeta, no abrirlo: abrir de un clic un .exe
            // recien bajado no es algo que un panel lateral deba hacer.
            if let Some(path) = path.filter(|p| p.exists()) {
                let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
            }
        }
        Command::DownloadControl { id, action } => downloads::control(state, id, &action),
        Command::ClearHistory => {
            {
                let mut st = state.borrow_mut();
                st.history.clear();
                st.save_history();
            }
            suggest::forget_favicons(state);
            sync::push_history(state);
            sync::push_own_pages(state);
        }
        Command::ToggleBookmark => actions::toggle_bookmark(state),
        Command::RemoveBookmark { url } => {
            {
                let mut st = state.borrow_mut();
                st.bookmarks.retain(|b| b.url != url);
                st.save_bookmarks();
            }
            sync::push_bookmarks(state);
            sync::push_active_tab(state);
        }
        Command::Menu { action } => menu_action(state, &action),
        Command::ToggleMenu => state.borrow_mut().request(Action::ToggleMenu),
        Command::CloseMenu => menu::close(state),
        Command::ExtPopup { id, right } => {
            menu::close(state);
            state.borrow_mut().request(Action::TogglePopup(id, right));
        }
        Command::OpenBlockedPopup => security::open_blocked_popup(state),
        Command::SuggestInput { text, deleting, x, w } => suggest::on_input(state, text, deleting, x, w),
        Command::SuggestKey { delta } => suggest::on_key(state, delta),
        Command::SuggestEnter { input } => suggest::on_enter(state, input),
        Command::SuggestPick { index } => suggest::on_pick(state, index),
        Command::SuggestClose => suggest::close(state),
        Command::InstallFromStore => {
            let target = state.borrow().active_tab().and_then(|t| extensions::store_id(&t.url));
            if let Some((store, id)) = target {
                extensions::install_from_store(state, store, id);
            }
        }
        Command::Ready => {
            // No sabemos cual de las paginas se acaba de anunciar, asi que
            // simplemente reenviamos todo; cada push_* es un no-op si la
            // pagina correspondiente todavia no existe.
            sync::push_tabs(state);
            sync::push_active_tab(state);
            sync::push_downloads(state);
            sync::push_history(state);
            sync::push_bookmarks(state);
            sync::push_maximized(state);
            // Antes que el fondo: asi se crea ya como imagen fija, no como video.
            sync::push_perf(state);
            sync::push_wallpaper(state);
            sync::push_menu(state);
            extensions::refresh(state);
            sync::push_expanded(state, Side::Left);
            sync::push_expanded(state, Side::Right);
            suggest::push(state);
        }
        other => handle_page_command(state, other),
    }
}

/// La pagina de la pestana avisa que empezo o termino de reproducir un video.
fn set_video_playing(state: &Shared, tab_id: u64, playing: bool) {
    {
        let mut st = state.borrow_mut();
        match st.tabs.iter_mut().find(|t| t.id == tab_id) {
            Some(tab) => tab.playing.video = playing,
            None => return,
        }
    }
    sync::push_media(state);
}

/// Dispatcher restringido para IPC que llega desde una pestana de
/// CONTENIDO. Solo se honra si el mensaje viene de una pagina propia
/// (app://) y la pestana sigue en una, y nunca los comandos de
/// ventana/pestanas/menu de `dispatch` — asi un sitio de terceros no puede
/// invocar nada, y ni siquiera nuestras paginas pueden hacer mas de lo que
/// necesitan (la pestana nueva no puede borrar datos ni tocar extensiones).
pub fn dispatch_from_content(state: &Shared, tab_id: u64, source: &str, raw: &str) {
    // Lo unico que se acepta de CUALQUIER sitio (ver ui/content_init.js): si
    // tiene un video reproduciendose. No ejecuta nada: solo congela el fondo
    // animado de la interfaz mientras tanto (ver sync::push_media).
    if let Some(flag) = raw.strip_prefix("michi:media:") {
        set_video_playing(state, tab_id, flag == "1");
        return;
    }
    let Some(page) = own_page_path(source) else { return };
    let trusted = state
        .borrow()
        .tabs
        .iter()
        .find(|t| t.id == tab_id)
        .map(|t| is_own_page(&t.url))
        .unwrap_or(false);
    if !trusted {
        return;
    }

    let Ok(cmd) = serde_json::from_str::<Command>(raw) else { return };
    match (page, cmd) {
        (_, Command::Ready) => {
            sync::push_perf(state);
            sync::push_wallpaper(state);
            sync::push_own_pages(state);
            sync::push_incognito(state, tab_id);
            match page {
                "/settings" => {
                    extensions::refresh(state);
                    sync::push_extension_status(state);
                    permissions::push_site_permissions(state);
                }
                "/insecure" => sync::push_insecure(state, tab_id),
                _ => {}
            }
        }
        ("/", cmd @ (Command::SetTheme { .. } | Command::Search { .. })) => handle_page_command(state, cmd),
        ("/settings", Command::NewIncognitoTab) => state.borrow_mut().request_tab_in(HOME_URL, true),
        (
            "/settings",
            cmd @ (Command::SetTheme { .. }
            | Command::SetSetting { .. }
            | Command::ResetSettings
            | Command::PickDownloadsDir
            | Command::ClearData { .. }
            | Command::ExtInstallFile
            | Command::ExtInstallFolder
            | Command::ExtEnable { .. }
            | Command::ExtRemove { .. }
            | Command::ExtOpen { .. }
            | Command::OpenUrl { .. }
            | Command::SetPermissionDefault { .. }
            | Command::ResetSitePermission { .. }
            | Command::ImportScan
            | Command::ImportRun { .. }),
        ) => handle_page_command(state, cmd),
        ("/insecure", Command::InsecureProceed) => security::proceed_insecure(state, tab_id),
        ("/insecure", Command::InsecureBack) => {
            let st = state.borrow();
            if let Some(tab) = st.tabs.iter().find(|t| t.id == tab_id) {
                let _ = if tab.webview.can_go_back().unwrap_or(false) {
                    tab.webview.go_back()
                } else {
                    tab.webview.load_url(HOME_LOAD_URL)
                };
            }
        }
        _ => {}
    }
}

fn handle_page_command(state: &Shared, cmd: Command) {
    match cmd {
        Command::SetTheme { id } => set_setting(state, "theme", Value::String(id)),
        Command::Search { q } => navigate_input(state, &q),
        Command::SetSetting { key, value } => set_setting(state, &key, value),
        Command::ResetSettings => {
            {
                let mut st = state.borrow_mut();
                st.settings = Settings::default();
                st.save_settings();
            }
            security::apply_smartscreen(state);
            security::apply_tracking_prevention(state);
            perf::on_mode_changed(state);
            sync::push_own_pages(state);
            sync::push_wallpaper(state);
            sync::push_toast(state, "Ajustes restablecidos");
        }
        Command::SetPermissionDefault { kind, blocked } => {
            if !permissions::is_kind(&kind) {
                return;
            }
            {
                let mut st = state.borrow_mut();
                let list = &mut st.settings.blocked_permissions;
                list.retain(|k| *k != kind);
                if blocked {
                    list.push(kind);
                }
                st.save_settings();
            }
            sync::push_own_pages(state);
        }
        Command::ResetSitePermission { origin, kind } => permissions::reset_site_permission(state, &origin, &kind),
        Command::PickDownloadsDir => {
            let proxy = state.borrow().proxy.clone();
            // El dialogo va en otro hilo: modal en este bloquearia el bucle de
            // eventos (y con el, los callbacks de WebView2) mientras esta abierto.
            std::thread::spawn(move || {
                if let (Some(dir), Some(proxy)) =
                    (rfd::FileDialog::new().set_parent(&crate::win::Owner).set_title("Carpeta de descargas").pick_folder(), proxy)
                {
                    let _ = proxy.send_event(UserEvent::DownloadsDirPicked(dir));
                }
            });
        }
        Command::ClearData { history, cookies, cache, downloads } => {
            {
                let mut st = state.borrow_mut();
                if history {
                    st.history.clear();
                    st.closed_tabs.clear();
                    st.save_history();
                }
                if downloads {
                    st.downloads.retain(|d| d.status == "downloading" || d.status == "paused");
                }
            }
            if history {
                suggest::forget_favicons(state);
            }
            sync::push_history(state);
            sync::push_downloads(state);
            let s = state.clone();
            let st = state.borrow();
            if let Some(tb) = &st.topbar {
                native::clear_browsing_data(tb, ClearKinds { cookies, cache }, move |ok| {
                    sync::push_own_pages(&s);
                    sync::push_toast(&s, if ok { "Datos borrados" } else { "No se pudieron borrar algunos datos" });
                });
            }
        }
        Command::ExtInstallFile => extensions::install_from_file(state),
        Command::ExtInstallFolder => extensions::install_from_folder(state),
        Command::ExtEnable { id, enabled } => extensions::set_enabled(state, id, enabled),
        Command::ExtRemove { id } => extensions::remove(state, id),
        Command::ExtOpen { id, page } => {
            if let Some(url) = extensions::page_url(&id, &page) {
                state.borrow_mut().request_tab(&url);
            }
        }
        Command::OpenUrl { url } => {
            if url.starts_with("https://") {
                state.borrow_mut().request_tab(&url);
            }
        }
        Command::ImportScan => {
            let sources = import::scan();
            state.borrow_mut().import_sources = sources.clone();
            sync::push_import_sources(state, &sources);
        }
        Command::ImportRun { key, bookmarks, history, cookies, tabs } => {
            import::run(state, key, import::Wants { bookmarks, history, cookies, tabs });
        }
        _ => {}
    }
}

fn set_setting(state: &Shared, key: &str, value: Value) {
    {
        let mut st = state.borrow_mut();
        let s = &mut st.settings;
        match (key, &value) {
            ("theme", Value::String(v)) if protocol::is_theme(v) => s.theme = v.clone(),
            ("searchEngine", Value::String(v)) if storage::SEARCH_ENGINES.iter().any(|e| e.0 == v) => {
                s.search_engine = v.clone()
            }
            ("startup", Value::String(v)) if v == "newtab" || v == "restore" => s.startup = v.clone(),
            ("defaultZoom", Value::Number(n)) => {
                if let Some(z) = n.as_f64().filter(|z| (0.25..=5.0).contains(z)) {
                    s.default_zoom = z;
                }
            }
            ("blockPopups", Value::Bool(v)) => s.block_popups = *v,
            ("smartscreen", Value::Bool(v)) => s.smartscreen = *v,
            ("httpsMode", Value::String(v)) if ["upgrade", "strict", "off"].contains(&v.as_str()) => {
                s.https_mode = v.clone()
            }
            ("trackingPrevention", Value::String(v)) if ["off", "basic", "balanced", "strict"].contains(&v.as_str()) => {
                s.tracking_prevention = v.clone()
            }
            ("clearOnExit", Value::Bool(v)) => s.clear_on_exit = *v,
            ("searchSuggestions", Value::Bool(v)) => s.search_suggestions = *v,
            ("performance", Value::String(v)) if perf::IDS.contains(&v.as_str()) => s.performance = v.clone(),
            ("downloadsDir", Value::Null) => s.downloads_dir = None,
            _ => {
                eprintln!("[ipc] ajuste invalido {key} = {value}");
                return;
            }
        }
        st.save_settings();
    }
    match key {
        "theme" => sync::push_wallpaper(state),
        "smartscreen" => security::apply_smartscreen(state),
        "trackingPrevention" => security::apply_tracking_prevention(state),
        "performance" => perf::on_mode_changed(state),
        // Lo que no tiene que quedar en el disco, desde ya.
        "clearOnExit" if value == Value::Bool(true) => storage::remove(storage::SESSION_FILE),
        _ => {}
    }
    sync::push_own_pages(state);
}

/// Carpeta elegida en el dialogo de descargas (llega como UserEvent).
pub fn downloads_dir_picked(state: &Shared, dir: std::path::PathBuf) {
    {
        let mut st = state.borrow_mut();
        st.settings.downloads_dir = Some(dir);
        st.save_settings();
    }
    sync::push_own_pages(state);
}

fn menu_action(state: &Shared, action: &str) {
    let mut st = state.borrow_mut();
    match action {
        "new_tab" => {
            st.request(Action::CloseMenu);
            st.request_tab(HOME_URL);
        }
        "new_incognito_tab" => {
            st.request(Action::CloseMenu);
            st.request_tab_in(HOME_URL, true);
        }
        "reopen_tab" => {
            st.request(Action::CloseMenu);
            st.request(Action::ReopenClosedTab);
        }
        "zoom_in" => st.request(Action::Zoom(1)),
        "zoom_out" => st.request(Action::Zoom(-1)),
        "zoom_reset" => st.request(Action::Zoom(0)),
        "bookmark" => {
            drop(st);
            actions::toggle_bookmark(state);
        }
        "library" => st.request(Action::ShowLibrary),
        "print" => st.request(Action::Print),
        "devtools" => st.request(Action::DevTools),
        "settings" => st.request(Action::OpenSettings(String::new())),
        "extensions" => st.request(Action::OpenSettings("extensions".into())),
        "about" => st.request(Action::OpenSettings("about".into())),
        "quit" => {
            drop(st);
            security::quit(state);
        }
        _ => {}
    }
}

pub fn navigate_input(state: &Shared, input: &str) {
    let engine = state.borrow().settings.search_engine.clone();
    if let Some(url) = storage::resolve_input(input, &engine) {
        with_active(state, |w| { let _ = w.load_url(&url); });
    }
}

fn window(state: &Shared) -> Option<std::rc::Rc<winit::window::Window>> {
    state.borrow().window.clone()
}

fn with_active(state: &Shared, f: impl FnOnce(&wry::WebView)) {
    let st = state.borrow();
    if let Some(tab) = st.tabs.get(st.active) {
        f(&tab.webview);
    }
}
