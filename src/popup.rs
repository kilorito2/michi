//! Popup de una extension: lo que en Chrome se abre al tocar su icono en la
//! barra. Es un WebView propio, flotando debajo del icono (como el menu,
//! ver menu.rs), con la pagina `action.default_popup` de la extension y el
//! script ui/ext_popup.js, que le hace ver como "pestana actual" la que el
//! usuario estaba mirando y nos avisa el tamano de su contenido.
//!
//! La pagina del popup es codigo de la EXTENSION (de terceros): su IPC solo
//! entiende popup_size y popup_close (ver `from_page`), nunca los comandos
//! de ipc.rs.
//!
//! Se cierra como el menu: clic fuera, Escape, o window.close() desde la
//! propia extension. Cerrar = soltar el WebView, siempre diferido
//! (Action::ClosePopup) cuando el pedido viene del propio popup: destruirlo
//! dentro de su callback lo romperia mientras se ejecuta.

use std::time::{Duration, Instant};

use serde::Deserialize;
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::http::Request;
use wry::{NewWindowResponse, Rect};

use crate::state::{Action, ExtPopup, Shared, TOPBAR_H};
use crate::{extensions, layout, panels, security, shortcuts};

const MIN_W: f64 = 160.0;
const MIN_H: f64 = 60.0;
/// Los mismos topes que Chrome.
const MAX_W: f64 = 800.0;
const MAX_H: f64 = 600.0;
/// Tamano mientras todavia no llego el del contenido (oculto hasta entonces).
const INITIAL: (f64, f64) = (360.0, 120.0);
/// Ver menu::REOPEN_GUARD: el clic en el icono con el popup abierto lo
/// cierra (clic fuera) justo antes de que llegue el clic que lo reabriria.
const REOPEN_GUARD: Duration = Duration::from_millis(250);

/// Clic en el icono de una extension (barra superior). `right` es el borde
/// derecho del icono en px logicos de la ventana: el popup se alinea ahi.
/// Siempre desde Action::TogglePopup, nunca desde un callback de WebView2:
/// crear el WebView ahi adentro se cuelga (ver AppState::request_tab).
pub fn toggle(state: &Shared, id: String, right: f64) {
    let (same_open, recently_closed_same) = {
        let st = state.borrow();
        let same_open = st.ext_popup.as_ref().is_some_and(|p| p.id == id);
        let recent = st
            .ext_popup_closed
            .as_ref()
            .is_some_and(|(closed_id, at)| *closed_id == id && at.elapsed() < REOPEN_GUARD);
        (same_open, recent)
    };
    close(state);
    if same_open || recently_closed_same {
        return;
    }
    open(state, id, right);
}

fn open(state: &Shared, id: String, right: f64) {
    let Some(url) = extensions::page_url(&id, "popup") else { return };
    let (window, target) = {
        let st = state.borrow();
        let Some(window) = st.window.clone() else { return };
        (window, st.active_tab().map(|t| t.url.clone()).unwrap_or_default())
    };
    let script = include_str!("../ui/ext_popup.js")
        .replace("__TARGET_URL__", &serde_json::to_string(&target).unwrap_or_else(|_| "\"\"".into()));

    let s_ipc = state.clone();
    let s_new = state.clone();
    // Solo se le atiende el IPC a la propia extension: si el popup navega a
    // una pagina web, esa pagina no puede ni cambiarle el tamano.
    let origin = format!("chrome-extension://{id}/");
    let webview = security::base_builder()
        .with_devtools(true)
        .with_visible(false)
        .with_initialization_script(script)
        .with_url(url)
        .with_bounds(bounds(right, INITIAL.0, INITIAL.1))
        .with_ipc_handler(move |req: Request<String>| {
            if req.uri().to_string().starts_with(&origin) {
                from_page(&s_ipc, req.body());
            }
        })
        .with_new_window_req_handler(move |url, _| {
            // Enlaces del popup que abren pestana (ayuda, panel de la
            // extension...). Con el mismo filtro que las pestanas (ver
            // security::popup_target_allowed): nada de archivos locales,
            // data: ni paginas internas.
            let mut st = s_new.borrow_mut();
            if url.starts_with("chrome-extension://") || security::popup_target_allowed(&url, "") {
                st.request_tab(&url);
            }
            st.request(Action::ClosePopup);
            NewWindowResponse::Deny
        })
        .build_as_child(window.as_ref());
    let Ok(webview) = webview else { return };
    security::harden(state, &webview);
    shortcuts::attach(state, &webview);

    state.borrow_mut().ext_popup = Some(ExtPopup { id, webview, right, sized: false });
    // Descarta el clic que abrio el popup (ver menu::show).
    panels::mouse_pressed();
    state.borrow().wake();
}

pub fn close(state: &Shared) {
    let popup = state.borrow_mut().ext_popup.take();
    if let Some(p) = popup {
        state.borrow_mut().ext_popup_closed = Some((p.id.clone(), Instant::now()));
        drop(p);
    }
}

fn bounds(right: f64, w: f64, h: f64) -> Rect {
    Rect {
        position: LogicalPosition::new((right - w).max(0.0), TOPBAR_H - 2.0).into(),
        size: LogicalSize::new(w, h).into(),
    }
}

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum PopupMsg {
    PopupSize { w: f64, h: f64 },
    PopupClose,
}

/// IPC que llega desde la pagina del popup (codigo de la extension).
fn from_page(state: &Shared, raw: &str) {
    let Ok(msg) = serde_json::from_str::<PopupMsg>(raw) else { return };
    match msg {
        PopupMsg::PopupSize { w, h } => {
            let mut st = state.borrow_mut();
            let Some(p) = st.ext_popup.as_mut() else { return };
            let w = w.clamp(MIN_W, MAX_W);
            let h = h.clamp(MIN_H, MAX_H);
            let _ = p.webview.set_bounds(bounds(p.right, w, h));
            if !p.sized {
                p.sized = true;
                let _ = p.webview.set_visible(true);
                let _ = p.webview.focus();
                drop(st);
                layout::raise_chrome(state);
            }
        }
        PopupMsg::PopupClose => state.borrow_mut().request(Action::ClosePopup),
    }
}

/// Como menu::watch: cierra el popup con un clic fuera de el.
pub fn watch(state: &Shared) -> Option<Instant> {
    let rect = {
        let st = state.borrow();
        let p = st.ext_popup.as_ref()?;
        let b = p.webview.bounds().ok()?;
        let scale = st.window.as_ref()?.scale_factor();
        let pos = b.position.to_logical::<f64>(scale);
        let size = b.size.to_logical::<f64>(scale);
        (pos.x, pos.y, size.width, size.height)
    };
    if panels::mouse_pressed() {
        let inside = panels::cursor_in_window(state).is_some_and(|(x, y, _, _)| {
            x >= rect.0 && x < rect.0 + rect.2 && y >= rect.1 && y < rect.1 + rect.3
        });
        if !inside {
            close(state);
            return None;
        }
    }
    Some(Instant::now() + Duration::from_millis(16))
}
