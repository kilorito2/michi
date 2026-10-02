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
pub fn attach(state: &Shared, webview: &WebView) {
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
    }
    true
}

enum Mapped {
    Action(Action),
    NewTab,
}

fn map(vk: u32, ctrl: bool, shift: bool, alt: bool) -> Option<Mapped> {
    use Mapped::{Action as A, NewTab};
    let key = char::from_u32(vk).filter(|c| c.is_ascii_alphanumeric());
    Some(match (ctrl, shift, alt, key, vk) {
        (true, false, false, Some('T'), _) => NewTab,
        (true, true, false, Some('T'), _) => A(Action::ReopenClosedTab),
        (true, false, false, Some('W'), _) => A(Action::CloseActiveTab),
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
