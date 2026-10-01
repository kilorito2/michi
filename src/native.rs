//! Acceso directo a la API de WebView2 (via webview2-com) para lo que wry no
//! expone: el perfil (extensiones, borrar datos por tipo) y el zoom actual.
//! Todas las pestanas y la interfaz comparten el mismo perfil (misma carpeta
//! de datos), asi que da igual desde que WebView se lo pida.

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{take_pwstr, ClearBrowsingDataCompletedHandler};
use windows_core::{Interface, PCWSTR, PWSTR};
use wry::{WebView, WebViewExtWindows};

use crate::import::Cookie;

pub fn profile(webview: &WebView) -> Option<ICoreWebView2Profile> {
    unsafe { webview.webview().cast::<ICoreWebView2_13>().ok()?.Profile().ok() }
}

/// Lee una cadena que WebView2 devuelve por puntero (y la libera).
pub unsafe fn read_pwstr(f: impl FnOnce(*mut PWSTR) -> windows_core::Result<()>) -> String {
    let mut p = PWSTR::null();
    if f(&mut p).is_err() {
        return String::new();
    }
    take_pwstr(p)
}

pub fn zoom(webview: &WebView) -> f64 {
    let mut z = 1.0;
    let _ = unsafe { webview.controller().ZoomFactor(&mut z) };
    z
}

/// Que borrar en "Borrar datos de navegacion" (ver ajustes, seccion Privacidad).
pub struct ClearKinds {
    pub cookies: bool,
    pub cache: bool,
}

pub fn clear_browsing_data(webview: &WebView, kinds: ClearKinds, done: impl FnOnce(bool) + 'static) {
    let mut flags = COREWEBVIEW2_BROWSING_DATA_KINDS(0);
    if kinds.cookies {
        // Cookies + todo el almacenamiento de sitios (localStorage, IndexedDB...).
        flags.0 |= COREWEBVIEW2_BROWSING_DATA_KINDS_ALL_SITE.0;
    }
    if kinds.cache {
        flags.0 |= COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE.0;
    }
    if flags.0 == 0 {
        return done(true);
    }
    let Some(profile2) = profile(webview).and_then(|p| p.cast::<ICoreWebView2Profile2>().ok()) else {
        return done(false);
    };
    let handler = ClearBrowsingDataCompletedHandler::create(Box::new(move |result| {
        done(result.is_ok());
        Ok(())
    }));
    if unsafe { profile2.ClearBrowsingData(flags, &handler) }.is_err() {
        // `done` ya se movio al handler; si la llamada misma falla el handler
        // nunca corre, pero tampoco hay nada que avisar mas alla del log.
        eprintln!("[native] ClearBrowsingData fallo");
    }
}

/// Cadena terminada en NUL para las APIs de WebView2 que esperan PCWSTR.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Agrega cookies (importadas de otro navegador, ver import.rs) al perfil
/// compartido de WebView2. Devuelve cuantas se escribieron. El perfil es el
/// mismo para todas las pestanas y la interfaz, asi que cualquier WebView vale.
pub fn add_cookies(webview: &WebView, cookies: &[Cookie]) -> usize {
    let manager = unsafe {
        webview
            .webview()
            .cast::<ICoreWebView2_2>()
            .ok()
            .and_then(|w| w.CookieManager().ok())
    };
    let Some(manager) = manager else { return 0 };

    let mut added = 0;
    for c in cookies {
        let (name, value, domain, path) = (wide(&c.name), wide(&c.value), wide(&c.domain), wide(&c.path));
        let cookie = unsafe {
            manager.CreateCookie(
                PCWSTR(name.as_ptr()),
                PCWSTR(value.as_ptr()),
                PCWSTR(domain.as_ptr()),
                PCWSTR(path.as_ptr()),
            )
        };
        let Ok(cookie) = cookie else { continue };
        unsafe {
            if let Some(expires) = c.expires {
                let _ = cookie.SetExpires(expires);
            }
            let _ = cookie.SetIsSecure(c.secure);
            let _ = cookie.SetIsHttpOnly(c.http_only);
            // Strict se respeta; el resto (incluido "sin atributo") queda en Lax,
            // que es el valor por defecto de los navegadores y no exige Secure
            // (SameSite=None sin Secure lo rechazaria WebView2).
            let same_site = if c.same_site == 2 {
                COREWEBVIEW2_COOKIE_SAME_SITE_KIND_STRICT
            } else {
                COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX
            };
            let _ = cookie.SetSameSite(same_site);
            if manager.AddOrUpdateCookie(&cookie).is_ok() {
                added += 1;
            }
        }
    }
    added
}

/// Convierte el IStream con los bytes PNG del favicon (lo que entrega
/// ICoreWebView2_15::GetFavicon) en un data URL listo para un <img>. None si el
/// icono esta vacio o no se pudo leer. Con un tope por si el stream fuera enorme.
pub fn stream_to_data_url(stream: windows::Win32::System::Com::IStream) -> Option<String> {
    use base64::Engine;

    const MAX: usize = 512 * 1024;
    let mut bytes = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let mut read = 0u32;
        let hr = unsafe { stream.Read(buf.as_mut_ptr() as *mut _, buf.len() as u32, Some(&mut read)) };
        if read == 0 {
            break; // fin del stream
        }
        bytes.extend_from_slice(&buf[..read as usize]);
        if bytes.len() > MAX || hr.is_err() {
            break;
        }
    }
    if bytes.is_empty() {
        return None;
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Some(format!("data:image/png;base64,{b64}"))
}
