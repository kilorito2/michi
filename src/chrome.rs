//! Construye los 3 WebViews de interfaz (barra superior, panel de pestanas a
//! la derecha, panel de descargas/historial a la izquierda). Los tres
//! muestran detras de su vidrio una version ya difuminada del fondo animado
//! (ver ui/wallpaper.js y tools/encode_wallpapers.sh).
//!
//! Se cargan desde el protocolo "app://" (ver src/protocol.rs) y no con
//! with_html: with_html usa NavigateToString, que deja la pagina con origen
//! "about:blank", y ahi la ruta relativa del video de fondo
//! ('/videos/<tema>.mp4') no resuelve a nada — el fondo animado nunca se
//! cargaba en la barra ni en los paneles.

use wry::dpi::{LogicalPosition, LogicalSize};
use wry::http::Request;
use wry::{Rect, WebView};

use crate::security;
use crate::state::{Shared, EDGE_IDLE, PANEL_W, TOPBAR_H};

/// Fondo de los paneles laterales. Son opacos (a diferencia de la barra):
/// se superponen a la pestana activa, y una ventana hija transparente no
/// deja ver a otra ventana hija de abajo sino lo que haya detras de TODA la
/// ventana (otras ventanas, el escritorio). Si alguna vez un pedazo del
/// panel queda sin pintar, se ve este color oscuro (el mismo del vidrio) en
/// vez de un agujero. La pagina del panel pinta su propio fondo animado
/// encima, asi que no necesita la transparencia de la ventana.
pub const PANEL_BG: (u8, u8, u8, u8) = (18, 17, 31, 255);

pub fn build_topbar(state: &Shared) -> WebView {
    let window = state
        .borrow()
        .window
        .clone()
        .expect("la ventana debe existir antes de crear la interfaz");
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());

    let s = state.clone();
    // Solo puede mostrar su propia pagina (ver security::chrome_builder).
    security::chrome_builder()
        .with_transparent(true)
        .with_url("app://localhost/topbar")
        .with_bounds(Rect {
            position: LogicalPosition::new(0.0, 0.0).into(),
            size: LogicalSize::new(size.width, TOPBAR_H).into(),
        })
        .with_ipc_handler(move |req: Request<String>| {
            crate::ipc::dispatch(&s, &req.uri().to_string(), req.body());
        })
        .build_as_child(window.as_ref())
        .expect("no se pudo crear la barra superior")
}

pub fn build_right_panel(state: &Shared) -> WebView {
    let window = state
        .borrow()
        .window
        .clone()
        .expect("la ventana debe existir antes de crear la interfaz");
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());

    let s = state.clone();
    // Solo puede mostrar su propia pagina (ver security::chrome_builder).
    security::chrome_builder()
        // Opaco a proposito (ver PANEL_BG).
        .with_background_color(PANEL_BG)
        .with_url("app://localhost/sidebar-right")
        .with_bounds(Rect {
            // Cerrado: solo asoma su franja de EDGE_IDLE (ver layout::position_panels).
            position: LogicalPosition::new((size.width - EDGE_IDLE).max(0.0), TOPBAR_H).into(),
            size: LogicalSize::new(PANEL_W, (size.height - TOPBAR_H).max(0.0)).into(),
        })
        .with_ipc_handler(move |req: Request<String>| {
            crate::ipc::dispatch(&s, &req.uri().to_string(), req.body());
        })
        .build_as_child(window.as_ref())
        .expect("no se pudo crear el panel de pestanas")
}

pub fn build_left_panel(state: &Shared) -> WebView {
    let window = state
        .borrow()
        .window
        .clone()
        .expect("la ventana debe existir antes de crear la interfaz");
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());

    let s = state.clone();
    // Solo puede mostrar su propia pagina (ver security::chrome_builder).
    security::chrome_builder()
        // Opaco a proposito (ver PANEL_BG).
        .with_background_color(PANEL_BG)
        .with_url("app://localhost/sidebar-left")
        .with_bounds(Rect {
            // Cerrado: solo asoma su franja de EDGE_IDLE (ver layout::position_panels).
            position: LogicalPosition::new(EDGE_IDLE - PANEL_W, TOPBAR_H).into(),
            size: LogicalSize::new(PANEL_W, (size.height - TOPBAR_H).max(0.0)).into(),
        })
        .with_ipc_handler(move |req: Request<String>| {
            crate::ipc::dispatch(&s, &req.uri().to_string(), req.body());
        })
        .build_as_child(window.as_ref())
        .expect("no se pudo crear el panel de descargas")
}
