//! Aviso de pantalla completa: "<sitio> esta en pantalla completa. Presiona
//! Esc para salir", arriba al centro, unos segundos (como Chrome).
//!
//! En pantalla completa la pagina ocupa todo el monitor y la barra de Michi
//! se oculta (ver fullscreen.rs). Sin este aviso, un sitio podia pasar a
//! pantalla completa con cualquier clic y dibujar una barra falsa, con otra
//! direccion y su candado: phishing que no se distingue de la barra real. El
//! aviso es un WebView de la interfaz (como el menu), por encima de la
//! pestana: la pagina no lo puede tapar ni sacar.

use std::time::{Duration, Instant};

use wry::dpi::{LogicalPosition, LogicalSize};
use wry::http::Request;
use wry::{Rect, WebView};

use crate::state::Shared;

/// Cuanto se ve el aviso.
const SHOW_FOR: Duration = Duration::from_secs(4);
const W: f64 = 520.0;
const H: f64 = 46.0;
/// Distancia al borde de arriba.
const TOP: f64 = 24.0;

pub fn build(state: &Shared) -> WebView {
    let window = state.borrow().window.clone().expect("la ventana debe existir antes del aviso");
    let s = state.clone();
    // Solo puede mostrar su propia pagina (ver security::chrome_builder).
    crate::security::chrome_builder()
        .with_background_color(crate::chrome::PANEL_BG)
        .with_visible(false)
        .with_url("app://localhost/notice")
        .with_bounds(rect(0.0))
        .with_ipc_handler(move |req: Request<String>| crate::ipc::dispatch(&s, &req.uri().to_string(), req.body()))
        .build_as_child(window.as_ref())
        .expect("no se pudo crear el aviso de pantalla completa")
}

fn rect(win_w: f64) -> Rect {
    let w = W.min(win_w);
    Rect {
        position: LogicalPosition::new(((win_w - w) / 2.0).max(0.0), TOP).into(),
        size: LogicalSize::new(w, H).into(),
    }
}

/// Muestra el aviso para `host` (el sitio de la pestana que paso a pantalla
/// completa; "" = no tiene, como un archivo local).
pub fn show(state: &Shared, host: &str) {
    {
        let mut st = state.borrow_mut();
        let Some(window) = st.window.clone() else { return };
        let Some(view) = &st.notice else { return };
        let size = window.inner_size().to_logical::<f64>(window.scale_factor());
        let host = serde_json::to_string(host).unwrap_or_else(|_| "\"\"".into());
        let _ = view.evaluate_script(&format!("window.onNotice && window.onNotice({host})"));
        let _ = view.set_bounds(rect(size.width));
        let _ = view.set_visible(true);
        st.notice_until = Some(Instant::now() + SHOW_FOR);
    }
    crate::layout::raise_chrome(state);
    state.borrow().wake();
}

pub fn hide(state: &Shared) {
    let mut st = state.borrow_mut();
    if st.notice_until.take().is_none() {
        return;
    }
    if let Some(view) = &st.notice {
        let _ = view.set_visible(false);
    }
}

/// Lo vuelve a ubicar si cambio el tamano de la ventana (al pasar a pantalla
/// completa la ventana crece despues de mostrarlo).
pub fn relayout(state: &Shared) {
    let st = state.borrow();
    if st.notice_until.is_none() {
        return;
    }
    let (Some(window), Some(view)) = (&st.window, &st.notice) else { return };
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
    let _ = view.set_bounds(rect(size.width));
}

/// Desde el bucle de eventos: lo oculta cuando se cumple su tiempo. Devuelve
/// cuando volver a mirar.
pub fn tick(state: &Shared) -> Option<Instant> {
    let until = state.borrow().notice_until?;
    if Instant::now() >= until {
        hide(state);
        return None;
    }
    Some(until)
}
