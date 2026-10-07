//! Atajos de teclado del navegador (Ctrl+T, Ctrl+W, Ctrl+Tab...).
//!
//! Con el foco dentro de un WebView, las teclas nunca llegan a la ventana de
//! winit: las recibe WebView2. Su evento AcceleratorKeyPressed se dispara
//! ANTES de que la pagina vea la tecla, para toda combinacion con Ctrl/Alt y
//! las teclas de funcion, y sirve para marcarla como manejada. Es nativo: una
//! pagina no puede dispararlo ni bloquearlo (a diferencia de escuchar keydown
//! con un script inyectado).
//!
//! Los que ya resuelve WebView2 solo (F5/Ctrl+R recargar, Alt+flechas,
//! Ctrl+F buscar, Ctrl+P imprimir, Ctrl+rueda/+/-/0 zoom, F12 herramientas
//! de desarrollador) no se tocan.
//!
//! La accion se encola (AppState::request) en vez de ejecutarse aca: el
//! callback corre dentro de WebView2, y por ejemplo cerrar la pestana desde
//! el callback de su propio WebView lo destruiria mientras se ejecuta.

use std::time::{Duration, Instant};

use webview2_com::AcceleratorKeyPressedEventHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
use wry::{WebView, WebViewExtWindows};

use crate::state::{Action, Shared};

const VK_TAB: u32 = 0x09;
const VK_PRIOR: u32 = 0x21; // Re Pag
const VK_NEXT: u32 = 0x22; // Av Pag
const VK_ESCAPE: u32 = 0x1B;

/// Engancha los atajos a un WebView (cada pestana y cada pieza de la interfaz).
/// `chrome`: es la barra o un panel lateral (ver restore_focus).
pub fn attach(state: &Shared, webview: &WebView, chrome: bool) {
    track_focus(state, webview, chrome);
    let s = state.clone();
    let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
        let mut vk = 0u32;
        unsafe {
            args.KeyEventKind(&mut kind)?;
            args.VirtualKey(&mut vk)?;
        }
        if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN {
            return Ok(());
        }
        // GetAsyncKeyState y no GetKeyState: con el foco en una pagina las
        // teclas las recibe el proceso de WebView2, no este hilo, asi que el
        // estado "segun los mensajes de este hilo" que da GetKeyState nunca
        // ve apretado el Ctrl. El asincrono es el del teclado real, ahora.
        let down = |key: u16| unsafe { GetAsyncKeyState(key as i32) } < 0;
        if handle_key(&s, vk, down(VK_CONTROL), down(VK_SHIFT), down(VK_MENU)) {
            unsafe { args.SetHandled(true)? };
        }
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { webview.controller().add_AcceleratorKeyPressed(&handler, &mut token) };
}

/// Recuerda que pieza tiene el foco del teclado: la barra o un panel (se
/// vuelve a ella al regresar a la ventana) o, con cualquier otra, la pestana.
fn track_focus(state: &Shared, webview: &WebView, chrome: bool) {
    use webview2_com::FocusChangedEventHandler;
    let s = state.clone();
    let handler = FocusChangedEventHandler::create(Box::new(move |controller, _| {
        let mut st = s.borrow_mut();
        if st.focus_settle_until.is_some_and(|t| Instant::now() < t) {
            return Ok(());
        }
        st.focus_owner = if chrome { controller } else { None };
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { webview.controller().add_GotFocus(&handler, &mut token) };
}

/// Al volver a la ventana (Alt+Tab, clic en la barra de tareas) wry le da el
/// foco a TODOS los WebView, uno atras de otro, y se queda el que se creo
/// primero: una pestana ya cerrada u oculta, que no recibe las teclas. Se lo
/// devolvemos a quien lo tenia (la barra o un panel) o a la pestana al frente.
pub fn restore_focus(state: &Shared) {
    let owner = {
        let mut st = state.borrow_mut();
        // Los avisos de foco de esa tanda no dicen quien lo tenia.
        st.focus_settle_until = Some(Instant::now() + Duration::from_millis(400));
        st.focus_owner.clone()
    };
    let moved = owner
        .is_some_and(|c| unsafe { c.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC) }.is_ok());
    if !moved {
        if let Some(t) = state.borrow().active_tab() {
            let _ = t.webview.focus();
        }
    }
}

/// Atajos que llegan a la ventana principal (winit) en vez de a un WebView:
/// pasa cuando ningun WebView tiene el foco, por ejemplo justo al volver a
/// la ventana con Alt+Tab o al hacer clic en el borde.
pub fn handle_winit_key(state: &Shared, event: &winit::event::KeyEvent, mods: winit::keyboard::ModifiersState) {
    use winit::keyboard::{KeyCode, PhysicalKey};
    if event.state != winit::event::ElementState::Pressed {
        return;
    }
    let PhysicalKey::Code(code) = event.physical_key else { return };
    let letters = [
        KeyCode::KeyA, KeyCode::KeyB, KeyCode::KeyC, KeyCode::KeyD, KeyCode::KeyE, KeyCode::KeyF,
        KeyCode::KeyG, KeyCode::KeyH, KeyCode::KeyI, KeyCode::KeyJ, KeyCode::KeyK, KeyCode::KeyL,
        KeyCode::KeyM, KeyCode::KeyN, KeyCode::KeyO, KeyCode::KeyP, KeyCode::KeyQ, KeyCode::KeyR,
        KeyCode::KeyS, KeyCode::KeyT, KeyCode::KeyU, KeyCode::KeyV, KeyCode::KeyW, KeyCode::KeyX,
        KeyCode::KeyY, KeyCode::KeyZ,
    ];
    let vk = match code {
        KeyCode::Tab => VK_TAB,
        KeyCode::PageUp => VK_PRIOR,
        KeyCode::PageDown => VK_NEXT,
        KeyCode::Escape => VK_ESCAPE,
        KeyCode::Digit1 | KeyCode::Numpad1 => b'1' as u32,
        KeyCode::Digit2 | KeyCode::Numpad2 => b'2' as u32,
        KeyCode::Digit3 | KeyCode::Numpad3 => b'3' as u32,
        KeyCode::Digit4 | KeyCode::Numpad4 => b'4' as u32,
        KeyCode::Digit5 | KeyCode::Numpad5 => b'5' as u32,
        KeyCode::Digit6 | KeyCode::Numpad6 => b'6' as u32,
        KeyCode::Digit7 | KeyCode::Numpad7 => b'7' as u32,
        KeyCode::Digit8 | KeyCode::Numpad8 => b'8' as u32,
        KeyCode::Digit9 | KeyCode::Numpad9 => b'9' as u32,
        c => match letters.iter().position(|l| *l == c) {
            Some(i) => b'A' as u32 + i as u32,
            None => return,
        },
    };
    handle_key(state, vk, mods.control_key(), mods.shift_key(), mods.alt_key());
}

/// Si la tecla (con los modificadores apretados ahora) es un atajo, encola
/// su accion y devuelve true (la tecla queda "manejada").
fn handle_key(state: &Shared, vk: u32, ctrl: bool, shift: bool, alt: bool) -> bool {
    // Escape solo es nuestro si el menu esta abierto o la pagina esta en
    // pantalla completa (como en Chrome, sale siempre, aunque el sitio no
    // lo maneje); si no, es de la pagina (cerrar un modal...).
    if vk == VK_ESCAPE && !ctrl && !shift && !alt {
        if crate::fullscreen::exit(state) {
            return true;
        }
        let (menu_open, popup_open) = {
            let st = state.borrow();
            (st.menu_open, st.ext_popup.is_some())
        };
        if !menu_open && !popup_open {
            return false;
        }
        let mut st = state.borrow_mut();
        st.request(Action::CloseMenu);
        st.request(Action::ClosePopup);
        return true;
    }

    let Some(action) = map(vk, ctrl, shift, alt) else { return false };
    let mut st = state.borrow_mut();
    match action {
        Mapped::Action(a) => st.request(a),
        Mapped::NewTab => st.request_tab(crate::state::HOME_URL),
        Mapped::NewIncognitoTab => st.request_tab_in(crate::state::HOME_URL, true),
    }
    true
}

const VK_NUMPAD1: u32 = 0x61;
const VK_NUMPAD9: u32 = 0x69;

/// Ctrl+'1'..'9' -> indice de pestana (el 9 es siempre la ultima: -1).
fn tab_index(digit: char) -> i32 {
    if digit == '9' {
        -1
    } else {
        digit as i32 - '1' as i32
    }
}

enum Mapped {
    Action(Action),
    NewTab,
    NewIncognitoTab,
}

fn map(vk: u32, ctrl: bool, shift: bool, alt: bool) -> Option<Mapped> {
    use Mapped::{Action as A, NewIncognitoTab, NewTab};
    let key = char::from_u32(vk).filter(|c| c.is_ascii_alphanumeric());
    Some(match (ctrl, shift, alt, key, vk) {
        (true, false, false, Some('T'), _) => NewTab,
        (true, true, false, Some('N'), _) => NewIncognitoTab,
        (true, true, false, Some('T'), _) => A(Action::ReopenClosedTab),
        (true, false, false, Some('W'), _) => A(Action::CloseActiveTab),
        // Ctrl+1..8: esa pestana; Ctrl+9: la ultima (como Chrome). Tambien con
        // el teclado numerico (VK_NUMPAD1..9).
        (true, false, false, Some(d @ '1'..='9'), _) => A(Action::GoToTab(tab_index(d))),
        (true, false, false, _, VK_NUMPAD1..=VK_NUMPAD9) => A(Action::GoToTab(tab_index((b'1' + (vk - VK_NUMPAD1) as u8) as char))),
        (true, false, false, _, VK_TAB) | (true, false, false, _, VK_NEXT) => A(Action::CycleTab(1)),
        (true, true, false, _, VK_TAB) | (true, false, false, _, VK_PRIOR) => A(Action::CycleTab(-1)),
        (true, false, false, Some('L'), _) | (false, false, true, Some('D'), _) => A(Action::FocusAddressBar),
        (true, false, false, Some('D'), _) => A(Action::ToggleBookmark),
        (true, false, false, Some('H'), _) | (true, false, false, Some('J'), _) => A(Action::ShowLibrary),
        (true, true, false, Some('O'), _) => A(Action::ShowLibrary),
        (false, false, true, Some('F'), _) => A(Action::ToggleMenu),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab_target(vk: u32, ctrl: bool, shift: bool, alt: bool) -> Option<i32> {
        match map(vk, ctrl, shift, alt) {
            Some(Mapped::Action(Action::GoToTab(n))) => Some(n),
            _ => None,
        }
    }

    #[test]
    fn ctrl_digits_pick_a_tab_and_nine_is_the_last() {
        for (i, digit) in (b'1'..=b'8').enumerate() {
            assert_eq!(tab_target(digit as u32, true, false, false), Some(i as i32));
        }
        assert_eq!(tab_target(b'9' as u32, true, false, false), Some(-1));
    }

    #[test]
    fn ctrl_shift_n_opens_an_incognito_tab() {
        assert!(matches!(map(b'N' as u32, true, true, false), Some(Mapped::NewIncognitoTab)));
        // Ctrl+N solo no es nuestro, ni Ctrl+Mayus+N con Alt.
        assert!(map(b'N' as u32, true, false, false).is_none());
        assert!(map(b'N' as u32, true, true, true).is_none());
    }

    #[test]
    fn numpad_digits_work_too() {
        assert_eq!(tab_target(VK_NUMPAD1, true, false, false), Some(0));
        assert_eq!(tab_target(VK_NUMPAD1 + 4, true, false, false), Some(4));
        assert_eq!(tab_target(VK_NUMPAD9, true, false, false), Some(-1));
    }

    #[test]
    fn digits_need_exactly_ctrl() {
        assert_eq!(tab_target(b'1' as u32, false, false, false), None);
        assert_eq!(tab_target(b'1' as u32, true, true, false), None);
        assert_eq!(tab_target(b'1' as u32, true, false, true), None);
        // Ctrl+0 es el zoom de WebView2: no se toca.
        assert_eq!(tab_target(b'0' as u32, true, false, false), None);
    }
}
