//! Menu "..." de la barra superior (nueva pestana, zoom, imprimir, ajustes,
//! extensiones...).
//!
//! Es un WebView aparte (ui/menu.html) y no un desplegable dentro de la
//! barra: el WebView de la barra mide TOPBAR_H de alto, y agrandarlo para
//! que entre el menu no sirve porque una ventana hija transparente deja ver
//! el escritorio, no la pestana de abajo (ver chrome.rs). Este WebView se
//! crea una vez, oculto, y se muestra/oculta debajo del boton.
//!
//! Se cierra con un clic fuera de el (ver `watch`), con Escape (ver
//! shortcuts.rs) o eligiendo una opcion. No con el `blur` de la pagina: con
//! WebViews anidados el foco rebota entre el menu y la barra varias veces
//! en milisegundos al abrirlo, y el menu se cerraba solo apenas aparecia.

use std::time::{Duration, Instant};

use wry::dpi::{LogicalPosition, LogicalSize};
use wry::http::Request;
use wry::{Rect, WebView};

use crate::state::{Shared, MENU_H, MENU_W, TOPBAR_H};
use crate::{layout, sync};

/// Si el menu se cerro hace menos que esto, un "abrir/cerrar" se ignora: al
/// hacer clic en el boton "..." con el menu abierto, el menu pierde el foco
/// (y se cierra) justo ANTES de que llegue el clic del boton, que sin esto
/// lo volveria a abrir.
const REOPEN_GUARD: Duration = Duration::from_millis(250);

pub fn build(state: &Shared) -> WebView {
    let window = state.borrow().window.clone().expect("la ventana debe existir antes del menu");
    let s = state.clone();
    // Solo puede mostrar su propia pagina (ver security::chrome_builder).
    crate::security::chrome_builder()
        .with_background_color(crate::chrome::PANEL_BG)
        .with_visible(false)
        .with_url("app://localhost/menu")
        .with_bounds(bounds(0.0))
        .with_ipc_handler(move |req: Request<String>| crate::ipc::dispatch(&s, &req.uri().to_string(), req.body()))
        .build_as_child(window.as_ref())
        .expect("no se pudo crear el menu")
}

fn bounds(win_w: f64) -> Rect {
    Rect {
        position: LogicalPosition::new((win_w - MENU_W - 8.0).max(0.0), TOPBAR_H - 4.0).into(),
        size: LogicalSize::new(MENU_W, MENU_H).into(),
    }
}

pub fn toggle(state: &Shared) {
    let (open, recently_closed) = {
        let st = state.borrow();
        (st.menu_open, st.menu_closed_at.is_some_and(|t| t.elapsed() < REOPEN_GUARD))
    };
    if open {
        close(state);
    } else if !recently_closed {
        show(state);
    }
}

pub fn show(state: &Shared) {
    {
        let mut st = state.borrow_mut();
        let Some(window) = st.window.clone() else { return };
        let Some(menu) = &st.menu else { return };
        let size = window.inner_size().to_logical::<f64>(window.scale_factor());
        let _ = menu.set_bounds(bounds(size.width));
        let _ = menu.set_visible(true);
        st.menu_open = true;
    }
    // Descarta el "se apreto desde la ultima consulta" del clic que abrio el
    // menu (sobre el boton "...", fuera del menu): si no, `watch` lo veria y
    // cerraria el menu en su primera pasada.
    crate::panels::mouse_pressed();
    layout::raise_chrome(state);
    sync::push_menu(state);
    sync::push_wallpaper(state);
    state.borrow().wake();
}

/// Mientras el menu esta abierto: lo cierra si se aprieta un boton del mouse
/// fuera de su rectangulo (en la ventana o en cualquier otra aplicacion).
/// Devuelve cuando quiere volver a mirar, o None si el menu esta cerrado.
/// El clic en el propio boton "..." tambien lo cierra aca, y REOPEN_GUARD
/// evita que ese mismo clic lo vuelva a abrir.
pub fn watch(state: &Shared) -> Option<Instant> {
    if !state.borrow().menu_open {
        return None;
    }
    let pressed = crate::panels::mouse_pressed();
    if pressed {
        let outside = match crate::panels::cursor_in_window(state) {
            Some((x, y, win_w, _)) => {
                let (mx, my) = wallpaper_offset(win_w);
                !(x >= mx && x < mx + MENU_W && y >= my && y < my + MENU_H)
            }
            None => true,
        };
        if outside {
            close(state);
            return None;
        }
    }
    Some(Instant::now() + Duration::from_millis(16))
}

pub fn close(state: &Shared) {
    let mut st = state.borrow_mut();
    if !st.menu_open {
        return;
    }
    st.menu_open = false;
    st.menu_closed_at = Some(Instant::now());
    if let Some(menu) = &st.menu {
        let _ = menu.set_visible(false);
    }
}

/// Reubica el menu si esta abierto (la ventana cambio de tamano).
pub fn relayout(state: &Shared) {
    let st = state.borrow();
    if !st.menu_open {
        return;
    }
    let (Some(window), Some(menu)) = (&st.window, &st.menu) else { return };
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
    let _ = menu.set_bounds(bounds(size.width));
}

/// Recorte del fondo animado que le toca al menu (ver sync::push_wallpaper).
pub fn wallpaper_offset(win_w: f64) -> (f64, f64) {
    ((win_w - MENU_W - 8.0).max(0.0), TOPBAR_H - 4.0)
}
