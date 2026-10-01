//! Calcula y aplica los bounds (posicion+tamano) de cada WebView hijo cada vez
//! que cambia el tamano de la ventana, la pestana activa o (solo los paneles)
//! en cada frame de la animacion de un panel lateral (ver panels.rs).
//!
//! El contenido (pestana activa) tiene SIEMPRE el mismo tamano: el hueco
//! entre las dos franjas de EDGE_IDLE de los bordes. Los paneles laterales,
//! al abrirse, se deslizan POR ENCIMA de el en vez de empujarlo, asi la
//! pagina no se re-acomoda (ni pega un salto) cada vez que se abre uno.
//!
//! Para que eso funcione, los paneles tienen que quedar siempre por encima
//! de las pestanas en el orden de apilamiento (z-order). wry no lo expone,
//! pero cada WebView vive en su propia ventana hija (WRY_WEBVIEW), y cada
//! una nueva se crea arriba de todo — una pestana abierta despues de los
//! paneles los tapaba. Por eso relayout vuelve a subir la barra y los
//! paneles al tope (ver raise_chrome) cada vez que se llama, lo que incluye
//! abrir, cerrar o cambiar de pestana.

use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{Rect, WebView};

use crate::state::{Shared, Side, EDGE_IDLE, PANEL_W, TOPBAR_H};

pub fn relayout(state: &Shared) {
    let st = state.borrow();
    let Some(window) = st.window.as_ref() else { return };
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());

    let content_h = (size.height - TOPBAR_H).max(0.0);
    let content_w = (size.width - 2.0 * EDGE_IDLE).max(0.0);

    if let Some(tb) = &st.topbar {
        let _ = tb.set_bounds(rect(0.0, 0.0, size.width, TOPBAR_H));
    }

    if let Some(active_tab) = st.tabs.get(st.active) {
        let _ = active_tab.webview.set_bounds(rect(EDGE_IDLE, TOPBAR_H, content_w, content_h));
        let _ = active_tab.webview.set_visible(true);
    }

    // Ocultas de verdad (no solo 0x0): asi la pagina pasa a
    // document.hidden, Chromium la frena, y el fondo animado de una pestana
    // nueva en segundo plano deja de decodificar video (ver ui/wallpaper.js).
    for (i, t) in st.tabs.iter().enumerate() {
        if i != st.active {
            let _ = t.webview.set_visible(false);
            let _ = t.webview.set_bounds(rect(0.0, 0.0, 0.0, 0.0));
        }
    }

    drop(st);
    position_panels(state);
    crate::menu::relayout(state);
    raise_chrome(state);
}

/// Ubica los dos paneles laterales segun cuanto esta abierto cada uno. Solo
/// los mueve: su tamano es siempre PANEL_W x (alto - TOPBAR_H), asi que en
/// cada frame de la animacion la pagina del panel no tiene que re-pintarse.
pub fn position_panels(state: &Shared) {
    let st = state.borrow();
    let Some(window) = st.window.as_ref() else { return };
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
    let panel_h = (size.height - TOPBAR_H).max(0.0);
    // Cuanto del panel queda fuera de la ventana cuando esta cerrado.
    let hidden = PANEL_W - EDGE_IDLE;

    if let Some(lp) = &st.left_panel {
        let x = -hidden * (1.0 - st.panel(Side::Left).progress);
        let _ = lp.set_bounds(rect(x, TOPBAR_H, PANEL_W, panel_h));
    }
    if let Some(rp) = &st.right_panel {
        let x = size.width - EDGE_IDLE - hidden * st.panel(Side::Right).progress;
        let _ = rp.set_bounds(rect(x, TOPBAR_H, PANEL_W, panel_h));
    }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect {
        position: LogicalPosition::new(x, y).into(),
        size: LogicalSize::new(w, h).into(),
    }
}

/// Sube la barra superior, los dos paneles, el menu "..." y las sugerencias
/// de la barra al tope del orden de apilamiento entre las ventanas hijas, por
/// encima de todas las pestanas (los desplegables ultimos: tienen que quedar
/// sobre los paneles).
pub fn raise_chrome(state: &Shared) {
    let st = state.borrow();
    for wv in [&st.topbar, &st.left_panel, &st.right_panel, &st.menu, &st.suggest.view].into_iter().flatten() {
        raise(wv);
    }
    if let Some(p) = &st.ext_popup {
        raise(&p.webview);
    }
}

#[cfg(windows)]
fn raise(webview: &WebView) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE,
    };
    use wry::WebViewExtWindows;

    let hwnd = webview.hwnd().0;
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOP,
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOOWNERZORDER,
        );
    }
}

#[cfg(not(windows))]
fn raise(_webview: &WebView) {}
