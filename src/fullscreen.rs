//! Pantalla completa de una pagina (el boton de pantalla completa de un video
//! de YouTube, element.requestFullscreen()...).
//!
//! WebView2 no agranda nada por su cuenta: solo hace que el elemento ocupe
//! todo SU WebView y avisa con ContainsFullScreenElementChanged. Sin
//! escucharlo, el video llenaba solo el hueco de la pestana, con la barra de
//! arriba a la vista y los paneles laterales que se seguian abriendo al
//! acercar el cursor al borde. Ahora, mientras la pestana activa tiene algo
//! en pantalla completa, la ventana pasa a pantalla completa sin bordes (todo
//! el monitor, barra de tareas incluida), la pestana la ocupa entera y la
//! barra y los paneles se ocultan (ver layout::relayout).
//!
//! Como en Chrome, cambiar de pestana saca de pantalla completa a la pagina
//! que estaba en ella.

use winit::window::Fullscreen;
use wry::{WebView, WebViewExtWindows};

use crate::state::{Action, AppState, Shared, Side};
use crate::{menu, panels, popup, suggest};

/// Escucha cuando la pagina de una pestana entra o sale de pantalla completa.
/// El cambio se aplica en la proxima vuelta del bucle (ver state::Action):
/// pasar la ventana a pantalla completa la redimensiona, y eso dispara el
/// Resized de winit en el acto.
pub fn attach(state: &Shared, webview: &WebView, id: u64) {
    use webview2_com::ContainsFullScreenElementChangedEventHandler;

    let s = state.clone();
    let handler = ContainsFullScreenElementChangedEventHandler::create(Box::new(move |sender, _| {
        let Some(core) = sender else { return Ok(()) };
        let mut on = windows_core::BOOL(0);
        unsafe { core.ContainsFullScreenElement(&mut on)? };
        s.borrow_mut().request(Action::Fullscreen(id, on.as_bool()));
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { webview.webview().add_ContainsFullScreenElementChanged(&handler, &mut token) };
}

/// Si la pestana activa esta en pantalla completa (la interfaz va oculta).
pub fn is_active(st: &AppState) -> bool {
    st.fullscreen.is_some() && st.fullscreen == st.active_tab().map(|t| t.id)
}

/// La pagina de la pestana `id` entro (`on`) o salio de pantalla completa.
pub fn set(state: &Shared, id: u64, on: bool) {
    {
        let mut st = state.borrow_mut();
        if on {
            // Una pestana de fondo no puede tapar la que se esta viendo.
            if st.active_tab().map(|t| t.id) != Some(id) {
                drop(st);
                exit_page(state, id);
                return;
            }
            st.fullscreen = Some(id);
        } else if st.fullscreen == Some(id) {
            st.fullscreen = None;
        } else {
            return;
        }
    }
    apply(state);
    crate::layout::relayout(state);
}

/// Escape: saca a la pagina de pantalla completa (ver shortcuts.rs). Devuelve
/// si habia algo que sacar.
pub fn exit(state: &Shared) -> bool {
    let id = {
        let st = state.borrow();
        if !is_active(&st) {
            return false;
        }
        st.fullscreen
    };
    if let Some(id) = id {
        exit_page(state, id);
    }
    true
}

/// Antes de mostrar otra pestana (ver tabs::activated): si la que estaba en
/// pantalla completa ya no es la activa (se cambio de pestana o se cerro), la
/// saca y devuelve la ventana a su tamano. Quien llama hace el relayout.
pub fn on_tab_switch(state: &Shared) {
    let left = {
        let mut st = state.borrow_mut();
        match st.fullscreen {
            Some(id) if st.active_tab().map(|t| t.id) != Some(id) => {
                st.fullscreen = None;
                Some(id)
            }
            _ => None,
        }
    };
    if let Some(id) = left {
        exit_page(state, id);
        apply(state);
    }
}

/// Le pide a la pagina que salga de pantalla completa. Su aviso de que salio
/// (ver `attach`) llega despues y termina de restaurar la interfaz.
fn exit_page(state: &Shared, id: u64) {
    let st = state.borrow();
    if let Some(tab) = st.tabs.iter().find(|t| t.id == id) {
        let _ = tab.webview.evaluate_script("document.fullscreenElement && document.exitFullscreen().catch(() => {})");
    }
}

/// Pone la ventana en pantalla completa o la devuelve a como estaba
/// (maximizada o no), y al entrar cierra lo que flotaba sobre la pagina.
fn apply(state: &Shared) {
    let (window, on) = {
        let st = state.borrow();
        let Some(window) = st.window.clone() else { return };
        (window, is_active(&st))
    };
    if on {
        menu::close(state);
        popup::close(state);
        suggest::close(state);
        for side in [Side::Left, Side::Right] {
            panels::close_now(state, side);
        }
    }
    // Sin el prestamo de `state`: esto dispara el Resized de winit en el acto.
    if window.fullscreen().is_some() != on {
        window.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
    }
}
