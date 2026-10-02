//! Empuja el estado de Rust hacia las paginas de interfaz (barra superior,
//! panel de pestanas, panel de descargas/historial/marcadores, menu "...") y
//! nuestras paginas propias abiertas en pestanas (nueva pestana, ajustes),
//! llamando a funciones JS globales que cada HTML define (window.onTabs,
//! window.onActiveTab, etc).

use serde_json::{json, Value};

use crate::state::{
    is_own_page, own_page_path, Shared, Side, EDGE_IDLE, MIN_WINDOW_H, MIN_WINDOW_W, PANEL_W, TOPBAR_H,
};
use crate::{extensions, menu, native, permissions, protocol, security, storage, tabs};

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
                "favicon": t.favicon,
            })
        })
        .collect();

    if let Some(rp) = &st.right_panel {
        if let Ok(payload) = serde_json::to_string(&data) {
            let _ = rp.evaluate_script(&format!("window.onTabs && window.onTabs({payload})"));
        }
    }
}

pub fn push_active_tab(state: &Shared) {
    let st = state.borrow();
    let Some(tab) = st.tabs.get(st.active) else { return };

    let can_back = tab.webview.can_go_back().unwrap_or(false);
    let can_fwd = tab.webview.can_go_forward().unwrap_or(false);

    // En la pagina de aviso de HTTPS la barra muestra el sitio pendiente (y
    // que no es seguro), no la direccion interna del aviso.
    let pending = tab.sec.insecure_target.as_ref().filter(|_| own_page_path(&tab.url) == Some("/insecure"));
    let (url, security) = match pending {
        Some(target) => (target.as_str(), "insecure"),
        None => (tab.url.as_str(), security::indicator(&tab.url, &st.cert_errors)),
    };

    let data = json!({
        "title": tab.title,
        "url": url,
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
    for tab in &st.tabs {
        if is_own_page(&tab.url) {
            let _ = tab.webview.evaluate_script(&call(EDGE_IDLE, TOPBAR_H));
        }
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
        let frozen = st.tabs.get(st.active).is_some_and(|t| t.playing.any());
        if frozen == st.wallpaper_frozen {
            return;
        }
        st.wallpaper_frozen = frozen;
        frozen
    };
    let st = state.borrow();
    let js = format!("window.__setWallpaperFrozen && window.__setWallpaperFrozen({frozen})");
    for view in [&st.topbar, &st.left_panel, &st.right_panel, &st.menu, &st.suggest.view].into_iter().flatten() {
        let _ = view.evaluate_script(&js);
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

pub fn push_history(state: &Shared) {
    let st = state.borrow();
    // El panel muestra las ultimas; no hace falta mandarle las 1000.
    let start = st.history.len().saturating_sub(100);
    let data: Vec<_> = st.history[start..]
        .iter()
        .map(|h| json!({ "title": h.title, "url": h.url }))
        .collect();

    if let Some(lp) = &st.left_panel {
        if let Ok(payload) = serde_json::to_string(&data) {
            let _ = lp.evaluate_script(&format!("window.onHistory && window.onHistory({payload})"));
        }
    }
}

pub fn push_bookmarks(state: &Shared) {
    let st = state.borrow();
    if let Some(lp) = &st.left_panel {
        if let Ok(payload) = serde_json::to_string(&st.bookmarks) {
            let _ = lp.evaluate_script(&format!("window.onBookmarks && window.onBookmarks({payload})"));
        }
    }
}

/// Estado que muestra el menu "..." (zoom de la pestana activa, marcador).
pub fn push_menu(state: &Shared) {
    let st = state.borrow();
    let Some(m) = &st.menu else { return };
    let (zoom, can_bookmark, bookmarked) = match st.active_tab() {
        Some(t) => (
            native::zoom(&t.webview),
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
    for tab in st.tabs.iter().filter(|t| is_own_page(&t.url)) {
        let _ = tab.webview.evaluate_script(js);
    }
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
            "smartscreenOffInWindows": security::smartscreen_off_in_windows(),
            "engineVersion": wry::webview_version().unwrap_or_default(),
            "appVersion": env!("CARGO_PKG_VERSION"),
            "dataDir": storage::data_dir(),
            "historyCount": st.history.len(),
            "bookmarkCount": st.bookmarks.len(),
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

/// Sitio pendiente de la pagina de aviso de HTTPS de esa pestana.
pub fn push_insecure(state: &Shared, tab_id: u64) {
    let Some((url, host)) = security::insecure_info(state, tab_id) else { return };
    let data = json!({ "url": url, "host": host });
    let st = state.borrow();
    if let Some(tab) = st.tabs.iter().find(|t| t.id == tab_id && is_own_page(&t.url)) {
        let _ = tab.webview.evaluate_script(&format!("window.onInsecure && window.onInsecure({data})"));
    }
}
