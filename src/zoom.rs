//! Zoom por sitio, como en Chrome: si agrandas youtube.com al 125%, todas las
//! paginas de youtube.com abren al 125%, en esta pestana y en las proximas.
//!
//! WebView2 no lo hace solo: su zoom es del WebView (de la pestana), no del
//! sitio, y se queda igual al navegar a otro lado. Asi que cada vez que la
//! pestana cambia de sitio se le pone el zoom guardado para ese sitio (o el
//! predeterminado de Ajustes), y cada vez que el usuario cambia el zoom
//! (Ctrl + rueda, Ctrl +/-, el menu) se guarda para el sitio en el que esta.
//! Lo de incognito no se guarda (diria que sitios se visitaron).
//!
//! WebView2 avisa (ZoomFactorChanged) cuando el zoom lo cambia el usuario en
//! la pagina (Ctrl + rueda), pero no cuando lo cambia Michi con
//! SetZoomFactor: los cambios de Michi (Ctrl +/-, el menu) se guardan a mano
//! con `remember` (ver actions.rs).

use webview2_com::ZoomFactorChangedEventHandler;
use wry::{WebView, WebViewExtWindows};

use crate::state::Shared;
use crate::{storage, sync};

/// El sitio de una direccion web, para recordar su zoom. Ni las paginas
/// propias ni los archivos locales: esos usan siempre el predeterminado.
pub fn site_of(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) || crate::state::is_own_page(url) {
        return None;
    }
    crate::security::host_of(url).filter(|h| !h.is_empty())
}

/// Avisa cuando el usuario cambia el zoom de la pestana `id` en la pagina.
pub fn attach(state: &Shared, webview: &WebView, id: u64) {
    let s = state.clone();
    let handler = ZoomFactorChangedEventHandler::create(Box::new(move |sender, _| {
        let Some(controller) = sender else { return Ok(()) };
        let mut zoom = 1.0;
        unsafe { controller.ZoomFactor(&mut zoom)? };
        remember(&s, id, zoom);
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { webview.controller().add_ZoomFactorChanged(&handler, &mut token) };
}

/// La pestana `id` quedo con ese zoom: se guarda para su sitio (al
/// predeterminado, el sitio se olvida).
pub fn remember(state: &Shared, id: u64, zoom: f64) {
    {
        let mut st = state.borrow_mut();
        let default = st.settings.default_zoom;
        let Some(tab) = st.tabs.iter().find(|t| t.id == id) else { return };
        let site = if tab.incognito { None } else { site_of(&tab.url) };
        let Some(site) = site else { return };
        let saved = st.site_zoom.get(&site).copied().unwrap_or(default);
        if (saved - zoom).abs() < 0.001 {
            return;
        }
        if (zoom - default).abs() < 0.001 {
            st.site_zoom.remove(&site);
        } else {
            st.site_zoom.insert(site, zoom);
        }
        // Con "borrar datos al cerrar" no queda en el disco (dice que sitios se visitaron).
        if !st.settings.clear_on_exit {
            storage::save(storage::SITE_ZOOM_FILE, &st.site_zoom);
        }
    }
    sync::push_menu(state);
}

/// Le pone a la pestana `id` el zoom de su sitio (si cambio). Se llama al
/// crearla y cada vez que navega.
pub fn apply(state: &Shared, id: u64) {
    // El controlador y no el WebView: el prestamo de `state` se suelta antes
    // de cambiar el zoom.
    let (controller, target) = {
        let st = state.borrow();
        let Some(tab) = st.tabs.iter().find(|t| t.id == id) else { return };
        let target = site_of(&tab.url).and_then(|s| st.site_zoom.get(&s).copied()).unwrap_or(st.settings.default_zoom);
        if (crate::native::zoom(&tab.webview) - target).abs() < 0.001 {
            return;
        }
        (tab.webview.controller(), target)
    };
    let _ = unsafe { controller.SetZoomFactor(target) };
}

/// Ajustes > Olvidar el zoom de los sitios (y al borrar el historial): las
/// pestanas abiertas vuelven al predeterminado.
pub fn forget_all(state: &Shared) {
    let ids: Vec<u64> = {
        let mut st = state.borrow_mut();
        st.site_zoom.clear();
        st.tabs.iter().map(|t| t.id).collect()
    };
    storage::remove(storage::SITE_ZOOM_FILE);
    for id in ids {
        apply(state, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_sites_have_their_own_zoom() {
        assert_eq!(site_of("https://www.YouTube.com/watch?v=1").as_deref(), Some("www.youtube.com"));
        assert_eq!(site_of("http://example.com:8080/").as_deref(), Some("example.com"));
        assert_eq!(site_of("http://app.localhost/settings"), None);
        assert_eq!(site_of("file:///C:/x.html"), None);
        assert_eq!(site_of("about:blank"), None);
    }
}
