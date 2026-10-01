//! Permisos que piden los sitios (camara, microfono, ubicacion...).
//!
//! WebView2 ya le pregunta al usuario con su propio cuadro la primera vez y
//! recuerda la respuesta en el perfil. Ademas de eso:
//! - Ajustes > Permisos: bloquear un tipo de permiso para todos los sitios
//!   (ni siquiera preguntan).
//! - Nunca a sitios sin conexion segura ni a nuestras paginas propias.
//! - Notificaciones solo si el pedido sale de un clic: el cartel de
//!   "permitir notificaciones" apenas abre la pagina no aparece.
//! - Ajustes > Permisos de sitios: ver y restablecer lo que se le permitio o
//!   bloqueo a cada sitio (lo que WebView2 guardo en el perfil).

use serde_json::{json, Value};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    GetNonDefaultPermissionSettingsCompletedHandler, PermissionRequestedEventHandler,
    SetPermissionStateCompletedHandler,
};
use windows_core::{Interface, BOOL, HSTRING};
use wry::{WebView, WebViewExtWindows};

use crate::native::{self, read_pwstr};
use crate::state::{is_own_page, Shared};
use crate::{security, sync};

/// Permisos que se pueden bloquear en general: (id en ajustes, nombre visible, tipo de WebView2).
pub const KINDS: &[(&str, &str, COREWEBVIEW2_PERMISSION_KIND)] = &[
    ("geolocation", "Ubicacion", COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION),
    ("camera", "Camara", COREWEBVIEW2_PERMISSION_KIND_CAMERA),
    ("microphone", "Microfono", COREWEBVIEW2_PERMISSION_KIND_MICROPHONE),
    ("notifications", "Notificaciones", COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS),
    ("clipboardRead", "Leer el portapapeles", COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ),
    ("multipleDownloads", "Varias descargas automaticas", COREWEBVIEW2_PERMISSION_KIND_MULTIPLE_AUTOMATIC_DOWNLOADS),
    ("fileReadWrite", "Editar archivos y carpetas", COREWEBVIEW2_PERMISSION_KIND_FILE_READ_WRITE),
    ("midi", "Control total de dispositivos MIDI", COREWEBVIEW2_PERMISSION_KIND_MIDI_SYSTEM_EXCLUSIVE_MESSAGES),
    ("sensors", "Sensores de movimiento", COREWEBVIEW2_PERMISSION_KIND_OTHER_SENSORS),
    ("localFonts", "Fuentes instaladas", COREWEBVIEW2_PERMISSION_KIND_LOCAL_FONTS),
    ("windowManagement", "Ventanas en varias pantallas", COREWEBVIEW2_PERMISSION_KIND_WINDOW_MANAGEMENT),
];

/// Los que pueden aparecer guardados para un sitio pero no se bloquean en general.
const OTHER_KINDS: &[(&str, &str, COREWEBVIEW2_PERMISSION_KIND)] = &[
    ("autoplay", "Reproduccion automatica", COREWEBVIEW2_PERMISSION_KIND_AUTOPLAY),
    ("unknown", "Otro permiso", COREWEBVIEW2_PERMISSION_KIND_UNKNOWN_PERMISSION),
];

fn all_kinds() -> impl Iterator<Item = &'static (&'static str, &'static str, COREWEBVIEW2_PERMISSION_KIND)> {
    KINDS.iter().chain(OTHER_KINDS)
}

pub fn is_kind(id: &str) -> bool {
    KINDS.iter().any(|k| k.0 == id)
}

/// Engancha la decision de permisos de una pestana.
pub fn attach(state: &Shared, webview: &WebView) {
    let s = state.clone();
    let handler = PermissionRequestedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let mut kind = COREWEBVIEW2_PERMISSION_KIND_UNKNOWN_PERMISSION;
        let mut user = BOOL(0);
        unsafe {
            args.PermissionKind(&mut kind)?;
            args.IsUserInitiated(&mut user)?;
        }
        let uri = unsafe { read_pwstr(|p| args.Uri(p)) };
        let blocked_by_user = {
            let st = s.borrow();
            KINDS
                .iter()
                .find(|k| k.2 == kind)
                .is_some_and(|k| st.settings.blocked_permissions.iter().any(|b| b == k.0))
        };
        let deny = is_own_page(&uri)
            || !security::is_secure_origin(&uri)
            || blocked_by_user
            || (kind == COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS && !user.as_bool());
        if deny {
            // Que no quede guardado en el perfil como decision del usuario:
            // si despues cambia el ajuste, el sitio puede volver a preguntar.
            if let Ok(args3) = args.cast::<ICoreWebView2PermissionRequestedEventArgs3>() {
                let _ = unsafe { args3.SetSavesInProfile(false) };
            }
            unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)? };
        }
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { webview.webview().add_PermissionRequested(&handler, &mut token) };
}

/// Le manda a la pagina de ajustes los permisos guardados de cada sitio.
pub fn push_site_permissions(state: &Shared) {
    let Some(profile) = profile4(state) else {
        return sync::push_site_permissions(state, &Value::Array(Vec::new()));
    };
    let s = state.clone();
    let handler = GetNonDefaultPermissionSettingsCompletedHandler::create(Box::new(move |_, list| {
        let items = list.map(|l| describe(&l)).unwrap_or_default();
        sync::push_site_permissions(&s, &Value::Array(items));
        Ok(())
    }));
    let _ = unsafe { profile.GetNonDefaultPermissionSettings(&handler) };
}

/// Ajustes > Permisos de sitios > Restablecer: el sitio vuelve a tener que preguntar.
pub fn reset_site_permission(state: &Shared, origin: &str, kind: &str) {
    let Some(&(_, _, kind)) = all_kinds().find(|k| k.0 == kind) else { return };
    let Some(profile) = profile4(state) else { return };
    let s = state.clone();
    let handler = SetPermissionStateCompletedHandler::create(Box::new(move |_| {
        push_site_permissions(&s);
        Ok(())
    }));
    let _ = unsafe {
        profile.SetPermissionState(kind, &HSTRING::from(origin), COREWEBVIEW2_PERMISSION_STATE_DEFAULT, &handler)
    };
}

fn describe(list: &ICoreWebView2PermissionSettingCollectionView) -> Vec<Value> {
    let mut count = 0u32;
    if unsafe { list.Count(&mut count) }.is_err() {
        return Vec::new();
    }
    (0..count)
        .filter_map(|i| unsafe { list.GetValueAtIndex(i) }.ok())
        .filter_map(|setting| {
            let mut kind = COREWEBVIEW2_PERMISSION_KIND_UNKNOWN_PERMISSION;
            let mut state = COREWEBVIEW2_PERMISSION_STATE_DEFAULT;
            unsafe {
                setting.PermissionKind(&mut kind).ok()?;
                setting.PermissionState(&mut state).ok()?;
            }
            let origin = unsafe { read_pwstr(|p| setting.PermissionOrigin(p)) };
            let &(id, label, _) = all_kinds().find(|k| k.2 == kind).unwrap_or(&OTHER_KINDS[1]);
            Some(json!({
                "origin": origin,
                "kind": id,
                "label": label,
                "allowed": state == COREWEBVIEW2_PERMISSION_STATE_ALLOW,
            }))
        })
        .collect()
}

fn profile4(state: &Shared) -> Option<ICoreWebView2Profile4> {
    let st = state.borrow();
    native::profile(st.topbar.as_ref()?)?.cast::<ICoreWebView2Profile4>().ok()
}
