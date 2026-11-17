//! Ejecuta las acciones encoladas con AppState::request (atajos de teclado,
//! menu "..."). Corre en about_to_wait (main.rs), fuera de cualquier callback
//! de WebView2 y sin prestamos de `state` vivos (ver state::Action).

use wry::WebViewExtWindows;

use crate::state::{own_page_path, Action, Shared, Side, SETTINGS_URL};
use crate::storage::Bookmark;
use crate::{fullscreen, layout, menu, native, panels, popup, sync, tabs};

/// Pasos de zoom (los mismos que Chrome/Edge).
const ZOOM_STEPS: &[f64] = &[0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0];

pub fn run_pending(state: &Shared) {
    // De a una: una accion puede encolar otras (se corren en esta misma vuelta).
    loop {
        let next = {
            let mut st = state.borrow_mut();
            if st.actions.is_empty() {
                None
            } else {
                Some(st.actions.remove(0))
            }
        };
        match next {
            Some(action) => run(state, action),
            None => break,
        }
    }
}

fn run(state: &Shared, action: Action) {
    match action {
        Action::CloseActiveTab => {
            let id = state.borrow().active_tab().map(|t| t.id);
            if let Some(id) = id {
                tabs::close_tab(state, id);
            }
        }
        Action::ReopenClosedTab => {
            let url = state.borrow_mut().closed_tabs.pop();
            if let Some(url) = url {
                tabs::open_tab(state, &url, false);
            }
        }
        Action::CycleTab(delta) => {
            {
                let mut st = state.borrow_mut();
                let n = st.tabs.len() as i32;
                if n < 2 {
                    return;
                }
                st.active = (st.active as i32 + delta).rem_euclid(n) as usize;
            }
            tabs::activated(state);
        }
        Action::GoToTab(n) => {
            {
                let mut st = state.borrow_mut();
                let last = st.tabs.len().saturating_sub(1);
                let target = if n < 0 { last } else { n as usize };
                if st.tabs.is_empty() || target > last || target == st.active {
                    return;
                }
                st.active = target;
            }
            tabs::activated(state);
        }
        Action::FocusAddressBar => {
            menu::close(state);
            let st = state.borrow();
            if let Some(tb) = &st.topbar {
                let _ = tb.focus();
                let _ = tb.evaluate_script("window.focusAddress && window.focusAddress()");
            }
        }
        Action::ToggleBookmark => toggle_bookmark(state),
        Action::ShowLibrary => {
            menu::close(state);
            panels::open_held(state, Side::Left);
        }
        Action::OpenSettings(section) => {
            menu::close(state);
            open_settings(state, &section);
        }
        Action::Print => {
            menu::close(state);
            if let Some(wv) = state.borrow().active_tab().and_then(|t| t.webview.as_ref()) {
                let _ = wv.print();
            }
        }
        Action::DevTools => {
            menu::close(state);
            if let Some(wv) = state.borrow().active_tab().and_then(|t| t.webview.as_ref()) {
                wv.open_devtools();
            }
        }
        Action::Zoom(step) => {
            // El controlador y no el WebView: el prestamo se suelta antes de
            // cambiar el zoom (ver zoom::apply).
            let (id, target, controller) = {
                let st = state.borrow();
                let Some(t) = st.active_tab() else { return };
                let Some(wv) = &t.webview else { return };
                let target = zoom_target(native::zoom(wv), step, st.settings.default_zoom);
                (t.id, target, wv.controller())
            };
            let _ = unsafe { controller.SetZoomFactor(target) };
            // Queda recordado para el sitio (ver zoom.rs).
            crate::zoom::remember(state, id, target);
            sync::push_menu(state);
        }
        Action::ToggleMenu => menu::toggle(state),
        Action::CloseMenu => menu::close(state),
        Action::TogglePopup(id, right) => popup::toggle(state, id, right),
        Action::ClosePopup => popup::close(state),
        Action::LoadUrl(id, url) => {
            // El puntero COM y no el WebView: el prestamo de `state` se suelta
            // antes de navegar (los eventos de la navegacion lo vuelven a pedir).
            let core = {
                let mut st = state.borrow_mut();
                // La decidio Michi: puede ir a una pagina propia (ver security::guard_own_page).
                crate::security::grant_own_page(&mut st, id, &url);
                st.tabs.iter().find(|t| t.id == id).and_then(|t| t.webview.as_ref()).map(|w| w.webview())
            };
            if let Some(core) = core {
                let _ = unsafe { core.Navigate(&windows_core::HSTRING::from(url.as_str())) };
            }
        }
        Action::CloseTab(id) => tabs::close_tab(state, id),
        Action::Fullscreen(id, on) => fullscreen::set(state, id, on),
        Action::Shutdown => crate::security::shutdown(state),
    }
}

/// El zoom que sigue a `current` un paso para arriba (`step` > 0), para
/// abajo (< 0) o el predeterminado (0).
fn zoom_target(current: f64, step: i32, default: f64) -> f64 {
    match step {
        0 => default,
        s if s > 0 => ZOOM_STEPS.iter().copied().find(|z| *z > current + 0.001).unwrap_or(5.0),
        _ => ZOOM_STEPS.iter().rev().copied().find(|z| *z < current - 0.001).unwrap_or(0.25),
    }
}

/// Agrega o quita la pagina activa de marcadores.
pub fn toggle_bookmark(state: &Shared) {
    {
        let mut st = state.borrow_mut();
        let Some(tab) = st.active_tab() else { return };
        if !tabs::is_bookmarkable(&tab.url) {
            return;
        }
        let (url, title) = (tab.url.clone(), tab.title.clone());
        if let Some(i) = st.bookmarks.iter().position(|b| b.url == url) {
            st.bookmarks.remove(i);
        } else {
            let title = if title.is_empty() { url.clone() } else { title };
            st.bookmarks.push(Bookmark { title, url, folder: String::new() });

        }
        st.save_bookmarks();
    }
    sync::push_bookmarks(state);
    sync::push_active_tab(state);
    sync::push_menu(state);
}

/// Cambia a la pestana de ajustes si ya hay una abierta (en esa seccion), o
/// abre una nueva.
fn open_settings(state: &Shared, section: &str) {
    let existing = {
        let st = state.borrow();
        st.tabs.iter().position(|t| own_page_path(&t.url) == Some("/settings"))
    };
    match existing {
        Some(i) => {
            {
                let mut st = state.borrow_mut();
                st.active = i;
                // Todavia sin abrir (ver Tab::webview): se abre ya en esa seccion.
                let tab = &mut st.tabs[i];
                if tab.is_lazy() && !section.is_empty() {
                    tab.url = format!("{SETTINGS_URL}#{section}");
                }
            }
            tabs::activated(state);
            let st = state.borrow();
            let js = format!("location.hash = {}", serde_json::to_string(section).unwrap_or_default());
            if let Some(wv) = &st.tabs[i].webview {
                let _ = wv.evaluate_script(&js);
            }
        }
        None => {
            let url = if section.is_empty() { SETTINGS_URL.to_string() } else { format!("{SETTINGS_URL}#{section}") };
            tabs::open_tab(state, &url, false);
        }
    }
    layout::relayout(state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_moves_one_step_and_stays_in_range() {
        assert_eq!(zoom_target(1.0, 1, 1.0), 1.1);
        assert_eq!(zoom_target(1.0, -1, 1.0), 0.9);
        // Un zoom entre pasos (Ctrl + rueda) salta al paso siguiente.
        assert_eq!(zoom_target(1.05, 1, 1.0), 1.1);
        assert_eq!(zoom_target(1.05, -1, 1.0), 1.0);
        assert_eq!(zoom_target(5.0, 1, 1.0), 5.0);
        assert_eq!(zoom_target(0.25, -1, 1.0), 0.25);
        assert_eq!(zoom_target(2.0, 0, 1.25), 1.25);
    }
}
