//! Empuja el estado de Rust hacia las paginas de interfaz (barra superior,
//! panel de pestanas, panel de descargas/historial/marcadores, menu "...") y
//! nuestras paginas propias abiertas en pestanas (nueva pestana, ajustes),
//! llamando a funciones JS globales que cada HTML define (window.onTabs,
//! window.onActiveTab, etc).

use serde_json::{json, Value};

use crate::state::{
    is_own_page, own_page_path, Shared, Side, EDGE_IDLE, MIN_WINDOW_H, MIN_WINDOW_W, PANEL_W, TOPBAR_H,
};
use crate::{extensions, menu, native, perf, permissions, protocol, security, storage, tabs};

/// Lo que se puede dejar para la proxima vuelta del bucle (ver mark).
pub const TABS: u8 = 1;
pub const ACTIVE_TAB: u8 = 2;
pub const HISTORY: u8 = 4;
pub const MEDIA: u8 = 8;

/// Anota que hay que reenviar `what` a la interfaz, y sale todo junto en la
/// proxima vuelta del bucle de eventos (ver flush). Cada cambio de titulo o
/// de carga de cada pestana mandaba la lista entera de pestanas, la barra y
/// el historial en el momento: con muchas pestanas cargando a la vez (o
/// una pagina que cambia el titulo seguido) eran decenas de envios por
/// segundo que la interfaz redibujaba enteros.
pub fn mark(state: &Shared, what: u8) {
    let st = state.borrow();
    let d = &st.dirty;
    let was = d.tabs.get() || d.active_tab.get() || d.history.get() || d.media.get();
    for (bit, flag) in [(TABS, &d.tabs), (ACTIVE_TAB, &d.active_tab), (HISTORY, &d.history), (MEDIA, &d.media)] {
        if what & bit != 0 {
            flag.set(true);
        }
    }
    // Un solo aviso hasta que se envie.
    if !was {
        st.wake();
    }
}

/// Envia lo anotado con mark. Lo llama el bucle de eventos en cada vuelta.
pub fn flush(state: &Shared) {
    let (tabs, active, history, media) = {
        let st = state.borrow();
        let d = &st.dirty;
        (d.tabs.replace(false), d.active_tab.replace(false), d.history.replace(false), d.media.replace(false))
    };
    if media {
        push_media(state);
    }
    if history {
        push_history(state);
    }
    if active {
        push_active_tab(state);
    }
    if tabs {
        push_tabs(state);
    }
}

pub fn push_tabs(state: &Shared) {
    let st = state.borrow();
    let data: Vec<_> = st
        .tabs
        .iter()
        .enumerate()
        .map(|(i, t)| {
            json!({
                "id": t.id,
                "title": t.title,
                "url": t.url,
                "active": i == st.active,
                // La URL corta, no el data URL (ver protocol::favicon_url).
                "favicon": t.favicon.as_deref().and_then(protocol::favicon_url),
                "incognito": t.incognito,
                "pinned": t.pinned,
                "muted": t.muted,
                // Esta sonando (aunque este silenciada: el icono lo dice).
                "audible": t.playing.audio,
                "crashed": own_page_path(&t.url) == Some("/crashed"),
                "group": t.group,
            })
        })
        .collect();
    let groups: Vec<_> = st
        .groups
        .iter()
        .map(|g| {
            let hex = crate::groups::COLORS.iter().find(|(c, _)| *c == g.color).map_or("#6c9eff", |c| c.1);
            json!({ "id": g.id, "name": g.name, "color": g.color, "hex": hex, "collapsed": g.collapsed })
        })
        .collect();
    let colors: Vec<_> = crate::groups::COLORS.iter().map(|(id, hex)| json!({ "id": id, "hex": hex })).collect();

    if let Some(rp) = &st.right_panel {
        let payload = json!({ "tabs": data, "groups": groups, "colors": colors });
        let _ = rp.evaluate_script(&format!("window.onTabs && window.onTabs({payload})"));
    }
}

pub fn push_active_tab(state: &Shared) {
    let st = state.borrow();
    let Some(tab) = st.tabs.get(st.active) else { return };

    let can_back = tab.webview.as_ref().and_then(|w| w.can_go_back().ok()).unwrap_or(false);
    let can_fwd = tab.webview.as_ref().and_then(|w| w.can_go_forward().ok()).unwrap_or(false);

    // En la pagina de aviso de HTTPS la barra muestra el sitio pendiente (y
    // que no es seguro), no la direccion interna del aviso; en la de "dejo
    // de funcionar", el sitio que se cayo.
    let url = tab.real_url();
    let security = match own_page_path(&tab.url) {
        Some("/insecure") if url != tab.url => "insecure",
        _ => security::indicator(url, &st.cert_errors),
    };

    let data = json!({
        "title": tab.title,
        "url": url,
        // Pestana de incognito: la barra se tine y lo avisa.
        "incognito": tab.incognito,
        // Lo que muestra la barra a la izquierda de la direccion; "internal"
        // = nuestras paginas (la barra queda vacia, como en la pestana nueva).
        "security": security,
        // Lo que se le bloqueo a esta pagina (escudo de la barra).
        "blocked": tab.sec.blocked,
        "loading": tab.loading,
        "canGoBack": can_back,
        "canGoForward": can_fwd,
        "canBookmark": tabs::is_bookmarkable(&tab.url),
        "bookmarked": st.bookmarks.iter().any(|b| b.url == tab.url),
        // Pagina de una extension en Chrome Web Store / Edge Add-ons: la barra
        // ofrece "Agregar a Michi" (ver extensions::store_id).
        "storeExtension": extensions::store_id(&tab.url).is_some(),
        // La llave de la barra (cuentas guardadas para este sitio) y "Guardar
        // la contrasena?" (ver passwords.rs).
        "passwordCount": st.passwords.count_for(&tab.url),
        "passwordOffer": crate::passwords::offer_view(&tab.password_offer),
        "extensionStatus": st.extension_status,
    });

    if let Some(tb) = &st.topbar {
        let _ = tb.evaluate_script(&format!("window.onActiveTab && window.onActiveTab({data})"));
    }
}

pub fn push_maximized(state: &Shared) {
    let st = state.borrow();
    let Some(window) = st.window.as_ref() else { return };
    let is_max = window.is_maximized();
    if let Some(tb) = &st.topbar {
        let _ = tb.evaluate_script(&format!("window.onMaximized && window.onMaximized({is_max})"));
    }
}

/// Reposiciona el fondo animado compartido en la barra superior, ambos
/// paneles, el menu y cualquier pestana que este en nuestra propia pagina
/// (app://). Cada superficie recibe un fondo del tamano de TODA la ventana,
/// corrido para que el fragmento visible en su propio viewport coincida con
/// lo que le tocaria si todas fueran una sola imagen — asi se ve como un
/// unico fondo continuo detras de toda la interfaz en vez de videos sueltos.
///
/// Los paneles laterales usan siempre el recorte que les toca ABIERTOS: se
/// deslizan (ver panels.rs) y su fondo viaja con ellos, difuminado por el
/// vidrio; cerrados solo asoman 10px, donde la diferencia no se nota.
pub fn push_wallpaper(state: &Shared) {
    let st = state.borrow();
    let Some(window) = st.window.as_ref() else { return };
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());

    // Winit a veces reporta, de forma transitoria durante el arranque (varias
    // veces seguidas, alternando con el tamano real), un inner_size absurdo
    // — bien por debajo de min_inner_size — probablemente por un reflow de
    // DWM al combinar with_blur(true) con una ventana transparente. La
    // ventana NUNCA puede ser mas chica que MIN_WINDOW_W/H de verdad (el SO
    // lo impide), asi que un valor menor a eso no es un tamano real: si lo
    // propagaramos, el fondo animado de las 4 superficies quedaria pegado a
    // un recorte minusculo del video hasta el proximo evento que lo pise.
    // Mejor ignorar el evento entero y dejar el ultimo tamano valido.
    if size.width < MIN_WINDOW_W || size.height < MIN_WINDOW_H {
        return;
    }

    let theme_json = serde_json::to_string(&st.settings.theme).unwrap_or_else(|_| "null".into());

    let call = |off_x: f64, off_y: f64| {
        format!(
            "window.__applyWallpaper && window.__applyWallpaper({theme_json}, {}, {}, {}, {})",
            size.width, size.height, off_x, off_y
        )
    };

    if let Some(tb) = &st.topbar {
        let _ = tb.evaluate_script(&call(0.0, 0.0));
    }
    if let Some(lp) = &st.left_panel {
        let _ = lp.evaluate_script(&call(0.0, TOPBAR_H));
    }
    if let Some(rp) = &st.right_panel {
        let _ = rp.evaluate_script(&call(size.width - PANEL_W, TOPBAR_H));
    }
    if let (Some(m), true) = (&st.menu, st.menu_open) {
        let (x, y) = menu::wallpaper_offset(size.width);
        let _ = m.evaluate_script(&call(x, y));
    }
    if let (Some(s), true) = (&st.suggest.view, st.suggest.open) {
        let (x, y) = st.suggest.pos;
        let _ = s.evaluate_script(&call(x, y));
    }
    for wv in own_page_views(&st) {
        let _ = wv.evaluate_script(&call(EDGE_IDLE, TOPBAR_H));
    }
}

/// Congela (o reanuda) el fondo animado de la interfaz segun la pestana al
/// frente: mientras reproduce un video o suena algo, los videos de fondo de
/// la barra, los paneles y el menu quedan en pausa. Medido con TikTok: sin
/// eso el proceso de GPU del motor gasta el doble (otro decodificador y otra
/// composicion por cada fondo) y los cuadros del video se atrasan. Barato de
/// llamar seguido: solo avisa cuando el estado cambia.
pub fn push_media(state: &Shared) {
    let frozen = {
        let mut st = state.borrow_mut();
        // Reproduciendo algo al frente, o (con un modo de rendimiento) Michi
        // fuera de primer plano.
        let frozen = st.tabs.get(st.active).is_some_and(|t| t.playing.any()) || perf::freeze_wallpaper(&st);
        if frozen == st.wallpaper_frozen {
            return;
        }
        st.wallpaper_frozen = frozen;
        frozen
    };
    send_frozen(state, frozen);
}

/// Lo mismo, sin mirar si cambio: para una pagina de la interfaz que recien
/// se anuncia (puede haber nacido despues de que se congelo el fondo, ver
/// main::build_deferred_ui).
pub fn push_media_now(state: &Shared) {
    let frozen = state.borrow().wallpaper_frozen;
    send_frozen(state, frozen);
}

fn send_frozen(state: &Shared, frozen: bool) {
    let st = state.borrow();
    let js = format!("window.__setWallpaperFrozen && window.__setWallpaperFrozen({frozen})");
    for view in [&st.topbar, &st.left_panel, &st.right_panel, &st.menu, &st.suggest.view].into_iter().flatten() {
        let _ = view.evaluate_script(&js);
    }
}

/// Le dice a cada pagina propia (la interfaz y las pestanas que muestran una
/// pagina nuestra) en que modo de rendimiento esta: el fondo fijo y las
/// animaciones los resuelve ella (ver ui/wallpaper.js, window.__setPerf).
pub fn push_perf(state: &Shared) {
    let st = state.borrow();
    let js = format!("window.__setPerf && window.__setPerf('{}')", perf::mode(&st).id());
    for view in [&st.topbar, &st.left_panel, &st.right_panel, &st.menu, &st.suggest.view].into_iter().flatten() {
        let _ = view.evaluate_script(&js);
    }
    for wv in own_page_views(&st) {
        let _ = wv.evaluate_script(&js);
    }
}

/// Los WebView de las pestanas que muestran una pagina propia (las que
/// todavia no se abrieron no tienen: ver Tab::webview), con la de repuesto:
/// asi ya tiene el fondo y los ajustes cuando se la usa (ver tabs::adopt_spare).
fn own_page_views(st: &crate::state::AppState) -> impl Iterator<Item = &wry::WebView> {
    st.tabs.iter().chain(&st.spare).filter(|t| is_own_page(&t.url)).filter_map(|t| t.webview.as_ref())
}

/// La pagina propia de esa pestana (la pestana nueva) se entera de que es de
/// incognito, para decirlo.
pub fn push_incognito(state: &Shared, tab_id: u64) {
    let st = state.borrow();
    if let Some(wv) = st.tabs.iter().find(|t| t.id == tab_id && t.incognito && is_own_page(&t.url)).and_then(|t| t.webview.as_ref()) {
        let _ = wv.evaluate_script("window.onIncognito && window.onIncognito(true)");
    }
}

/// Le confirma a la pagina de un panel lateral si esta abierto (ver
/// window.onExpanded en ui/sidebar_*.html y panels::set_expanded).
pub fn push_expanded(state: &Shared, side: Side) {
    let st = state.borrow();
    let expanded = st.panel(side).expanded;
    let panel = match side {
        Side::Left => &st.left_panel,
        Side::Right => &st.right_panel,
    };
    if let Some(p) = panel {
        let _ = p.evaluate_script(&format!("window.onExpanded && window.onExpanded({expanded})"));
    }
}

pub fn push_downloads(state: &Shared) {
    let st = state.borrow();
    let data: Vec<_> = st
        .downloads
        .iter()
        .map(|d| {
            json!({
                "id": d.id,
                "fileName": d.file_name,
                "status": d.status,
                "received": d.received,
                "total": d.total,
                // Tipo de archivo peligroso esperando que el usuario decida
                // (ver downloads::on_security_check).
                "dangerous": d.dangerous,
                "insecureSource": d.insecure_source,
            })
        })
        .collect();

    if let Some(lp) = &st.left_panel {
        if let Ok(payload) = serde_json::to_string(&data) {
            let _ = lp.evaluate_script(&format!("window.onDownloads && window.onDownloads({payload})"));
        }
    }
}

/// El historial del panel izquierdo (ver library.rs: buscador, por dia).
pub fn push_history(state: &Shared) {
    crate::library::push_history(state);
}

/// Los marcadores del panel izquierdo, con sus carpetas (ver library.rs).
pub fn push_bookmarks(state: &Shared) {
    crate::library::push_bookmarks(state);
}

/// Estado que muestra el menu "..." (zoom de la pestana activa, marcador).
pub fn push_menu(state: &Shared) {
    let st = state.borrow();
    let Some(m) = &st.menu else { return };
    let (zoom, can_bookmark, bookmarked) = match st.active_tab() {
        Some(t) => (
            t.webview.as_ref().map_or(1.0, native::zoom),
            tabs::is_bookmarkable(&t.url),
            st.bookmarks.iter().any(|b| b.url == t.url),
        ),
        None => (1.0, false, false),
    };
    let data = json!({ "zoom": zoom, "canBookmark": can_bookmark, "bookmarked": bookmarked });
    let _ = m.evaluate_script(&format!("window.onMenuState && window.onMenuState({data})"));
}

/// Ejecuta `js` en cada pestana que este en una pagina propia (nueva
/// pestana, ajustes).
fn for_own_pages(state: &Shared, js: &str) {
    let st = state.borrow();
    for wv in own_page_views(&st) {
        let _ = wv.evaluate_script(js);
    }
}

/// Version de WebView2: no cambia mientras Michi esta abierto, y preguntarla
/// carga la DLL del cargador cada vez.
fn engine_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| wry::webview_version().unwrap_or_default())
}

/// Ajustes + catalogos (buscadores, fondos) para la pestana nueva y la
/// pagina de ajustes.
pub fn push_own_pages(state: &Shared) {
    let data = {
        let st = state.borrow();
        let engines: Vec<_> = storage::SEARCH_ENGINES
            .iter()
            .map(|(id, label, _)| json!({ "id": id, "label": label }))
            .collect();
        let themes: Vec<_> = protocol::THEMES
            .iter()
            .map(|(id, label)| json!({ "id": id, "label": label }))
            .collect();
        let permission_kinds: Vec<_> = permissions::KINDS
            .iter()
            .map(|(id, label, _)| json!({ "id": id, "label": label }))
            .collect();
        json!({
            "settings": st.settings,
            "downloadsDir": st.settings.downloads_dir(),
            "engines": engines,
            "themes": themes,
            "permissionKinds": permission_kinds,
            // Los dos se calculan una vez (esto corre en cada cambio de pestana).
            "smartscreenOffInWindows": security::smartscreen_off_cached(),
            "engineVersion": engine_version(),
            "appVersion": env!("CARGO_PKG_VERSION"),
            "dataDir": storage::data_dir(),
            "historyCount": st.history.len(),
            "bookmarkCount": st.bookmarks.len(),
            "siteZoomCount": st.site_zoom.len(),
            "update": crate::update::view(&st),
        })
    };
    for_own_pages(state, &format!("window.onSettings && window.onSettings({data})"));
}

pub fn push_extensions(state: &Shared, list: &Value, supported: bool) {
    for_own_pages(
        state,
        &format!("window.onExtensions && window.onExtensions({list}, {supported})"),
    );
}

/// Iconos de extensiones en la barra superior (ver popup.rs).
pub fn push_toolbar(state: &Shared, items: &Value) {
    let st = state.borrow();
    if let Some(tb) = &st.topbar {
        let _ = tb.evaluate_script(&format!("window.onExtensionToolbar && window.onExtensionToolbar({items})"));
    }
}

pub fn push_extension_status(state: &Shared) {
    let msg = serde_json::to_string(&state.borrow().extension_status).unwrap_or_default();
    for_own_pages(state, &format!("window.onExtensionStatus && window.onExtensionStatus({msg})"));
    push_active_tab(state);
}

/// Solo a las pestanas que muestran Ajustes (no a la pestana nueva).
fn for_settings(state: &Shared, js: &str) {
    let st = state.borrow();
    for tab in st.tabs.iter().filter(|t| own_page_path(&t.url) == Some("/settings")) {
        if let Some(wv) = &tab.webview {
            let _ = wv.evaluate_script(js);
        }
    }
}

/// Ajustes > Contrasenas: la lista (sin las contrasenas).
pub fn push_passwords(state: &Shared, data: &Value) {
    for_settings(state, &format!("window.onPasswords && window.onPasswords({data})"));
}

/// Ajustes > Contrasenas: una contrasena, despues de Windows Hello.
pub fn push_password_revealed(state: &Shared, data: &Value) {
    for_settings(state, &format!("window.onPasswordRevealed && window.onPasswordRevealed({data})"));
}

/// Resultado de una accion de la pagina de ajustes ("Datos borrados", etc).
pub fn push_toast(state: &Shared, message: &str) {
    let msg = serde_json::to_string(message).unwrap_or_default();
    for_own_pages(state, &format!("window.onToast && window.onToast({msg})"));
}

/// Permisos guardados de cada sitio (ajustes, ver permissions.rs).
pub fn push_site_permissions(state: &Shared, list: &Value) {
    for_own_pages(state, &format!("window.onSitePermissions && window.onSitePermissions({list})"));
}

/// Navegadores detectados para importar (ajustes, ver import.rs).
pub fn push_import_sources(state: &Shared, sources: &[crate::import::Source]) {
    let payload = serde_json::to_string(sources).unwrap_or_else(|_| "[]".into());
    for_own_pages(state, &format!("window.onImportSources && window.onImportSources({payload})"));
}

/// Lo que se cayo, para la pagina "dejo de funcionar" de esa pestana.
pub fn push_crashed(state: &Shared, tab_id: u64) {
    let Some((url, title, reason)) = crate::recovery::crash_info(state, tab_id) else { return };
    let data = json!({ "url": url, "host": security::host_of(&url).unwrap_or_default(), "title": title, "reason": reason });
    let st = state.borrow();
    if let Some(wv) = st.tabs.iter().find(|t| t.id == tab_id && is_own_page(&t.url)).and_then(|t| t.webview.as_ref()) {
        let _ = wv.evaluate_script(&format!("window.onCrashed && window.onCrashed({data})"));
    }
}

/// Sitio pendiente de la pagina de aviso de HTTPS de esa pestana.
pub fn push_insecure(state: &Shared, tab_id: u64) {
    let Some((url, host, form)) = security::insecure_info(state, tab_id) else { return };
    let data = json!({ "url": url, "host": host, "form": form });
    let st = state.borrow();
    if let Some(wv) = st.tabs.iter().find(|t| t.id == tab_id && is_own_page(&t.url)).and_then(|t| t.webview.as_ref()) {
        let _ = wv.evaluate_script(&format!("window.onInsecure && window.onInsecure({data})"));
    }
}
