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
use crate::state::{Shared, UiPiece, EDGE_IDLE, PANEL_W, TOPBAR_H};

/// Fondo de los paneles laterales. Son opacos (a diferencia de la barra):
/// se superponen a la pestana activa, y una ventana hija transparente no
/// deja ver a otra ventana hija de abajo sino lo que haya detras de TODA la
/// ventana (otras ventanas, el escritorio). Si alguna vez un pedazo del
/// panel queda sin pintar, se ve este color oscuro (el mismo del vidrio) en
/// vez de un agujero. La pagina del panel pinta su propio fondo animado
/// encima, asi que no necesita la transparencia de la ventana.
pub const PANEL_BG: (u8, u8, u8, u8) = (18, 17, 31, 255);

/// Crea esa pieza de la interfaz si todavia no existe. Nacen ocultas y se
/// crean despues del primer cuadro (ver main::build_deferred_ui), o en el
/// momento si se necesitan antes (abrir el menu, una pagina en pantalla
/// completa). Nunca dentro de un callback de WebView2 (ver
/// AppState::request_tab): el menu y el aviso se muestran desde una
/// Action, en el bucle de eventos.
pub fn ensure(state: &Shared, piece: UiPiece) {
    {
        let mut st = state.borrow_mut();
        st.ui_pending.retain(|p| *p != piece);
        let exists = match piece {
            UiPiece::Notice => st.notice.is_some(),
            UiPiece::Menu => st.menu.is_some(),
            UiPiece::Suggestions => st.suggest.view.is_some(),
        };
        if exists || st.shutting_down || st.window.is_none() {
            return;
        }
    }
    let wv = match piece {
        UiPiece::Notice => crate::notice::build(state),
        UiPiece::Menu => crate::menu::build(state),
        UiPiece::Suggestions => crate::suggest::build(state),
    };
    security::harden(state, &wv);
    crate::context_menu::attach_ui(&wv);
    crate::shortcuts::attach(state, &wv, false);
    crate::recovery::attach(state, &wv, None);
    {
        let mut st = state.borrow_mut();
        let slot = match piece {
            UiPiece::Notice => &mut st.notice,
            UiPiece::Menu => &mut st.menu,
            UiPiece::Suggestions => &mut st.suggest.view,
        };
        *slot = Some(wv);
    }
    // Nacio arriba de todo: los desplegables quedan en su lugar (ver layout::raise_chrome).
    crate::layout::raise_chrome(state);
}

/// Es el primer WebView de la interfaz: si falla, Michi no puede seguir y lo
/// dice (ver main::fatal) en vez de cerrarse sin explicacion.
pub fn build_topbar(state: &Shared) -> wry::Result<WebView> {
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
        .expect("no se pudo crear el panel de pestañas")
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
