//! Descargas, con progreso, pausa, reanudar y cancelar.
//!
//! wry solo avisa "empezo" y "termino" (with_download_*_handler): nada de
//! progreso. Por eso se engancha directo el DownloadStarting de WebView2,
//! que entrega la operacion de descarga (ICoreWebView2DownloadOperation)
//! con sus eventos de bytes recibidos y cambio de estado.
//!
//! Las operaciones (punteros COM) viven en un thread_local y no en AppState:
//! solo se usan desde el hilo principal, y asi DownloadEntry sigue siendo un
//! dato simple que se serializa para el panel.
//!
//! Seguridad:
//! - Tipos de archivo que pueden ejecutar algo (.exe, .msi, .bat, .ps1, .js,
//!   .lnk...: ver DANGEROUS_EXTENSIONS): la descarga queda en espera hasta
//!   que el usuario elige "Conservar" o "Descartar" en el panel, como en
//!   Chrome (ver on_security_check). Si es una pagina guardando un archivo
//!   de esos por su cuenta (File System Access), se bloquea. Todo lo demas
//!   (imagenes, PDF, documentos, .zip...) se descarga sin preguntar.
//! - SmartScreen (ver security.rs) puede frenar un archivo malicioso: queda
//!   como "bloqueado".
//! - Todo lo descargado lleva la marca de la Web (ver ensure_mark_of_the_web).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    BytesReceivedChangedEventHandler, DownloadStartingEventHandler, SaveFileSecurityCheckStartingEventHandler,
    StateChangedEventHandler,
};
use windows_core::{Interface, BOOL, HSTRING};
use wry::{WebView, WebViewExtWindows};

use crate::native::read_pwstr;
use crate::panels;
use crate::state::{DownloadEntry, Shared, Side};
use crate::sync;

/// Cada cuanto, como mucho, se repinta el progreso en el panel.
const PROGRESS_EVERY: Duration = Duration::from_millis(150);

thread_local! {
    static OPS: RefCell<HashMap<u64, ICoreWebView2DownloadOperation>> = RefCell::new(HashMap::new());
    /// Archivos peligrosos esperando la decision del usuario: el evento de
    /// WebView2 queda en suspenso (deferral) hasta entonces.
    static PENDING: RefCell<HashMap<u64, (ICoreWebView2SaveFileSecurityCheckStartingEventArgs, ICoreWebView2Deferral)>> =
        RefCell::new(HashMap::new());
}

/// Engancha las descargas de una pestana.
pub fn attach(state: &Shared, webview: &WebView) {
    on_security_check(state, webview);
    let Ok(wv4) = webview.webview().cast::<ICoreWebView2_4>() else { return };
    let s = state.clone();
    let handler = DownloadStartingEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let op = unsafe { args.DownloadOperation()? };

        let suggested = unsafe { read_pwstr(|p| args.ResultFilePath(p)) };
        let name = Path::new(&suggested)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "descarga".into());
        let source = unsafe { read_pwstr(|p| op.Uri(p)) };

        // El paquete de una extension que baja el boton de la propia tienda:
        // se instala (verificado y con confirmacion) en vez de guardarse.
        let mime = unsafe { read_pwstr(|p| op.MimeType(p)) };
        if let Some((store, ext_id)) = crate::extensions::store_download(&source, &name, &mime) {
            unsafe { args.SetCancel(true)? };
            crate::extensions::install_from_store(&s, store, ext_id);
            return Ok(());
        }
        let dir = s.borrow().settings.downloads_dir();
        let path = unique_path(&dir, &name);
        unsafe {
            args.SetResultFilePath(&HSTRING::from(path.as_path()))?;
            // Sin el cuadro de descargas propio de WebView2: lo mostramos
            // nosotros en el panel izquierdo.
            args.SetHandled(true)?;
        }

        let id = {
            let mut st = s.borrow_mut();
            let id = st.alloc_id();
            st.downloads.push(DownloadEntry {
                id,
                file_name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(name),
                status: "downloading".into(),
                path: Some(path),
                received: 0,
                total: total_bytes(&op),
                dangerous: false,
                insecure_source: source.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("http://")),
            });
            id
        };
        OPS.with(|ops| ops.borrow_mut().insert(id, op.clone()));

        let s_bytes = s.clone();
        let last_push = Rc::new(Cell::new(Instant::now() - PROGRESS_EVERY));
        let on_bytes = BytesReceivedChangedEventHandler::create(Box::new(move |op, _| {
            let Some(op) = op else { return Ok(()) };
            let mut received = 0i64;
            unsafe { op.BytesReceived(&mut received)? };
            let total = total_bytes(&op);
            update(&s_bytes, id, |d| {
                d.received = received.max(0) as u64;
                d.total = total;
            });
            if last_push.get().elapsed() >= PROGRESS_EVERY {
                last_push.set(Instant::now());
                sync::push_downloads(&s_bytes);
            }
            Ok(())
        }));
        let s_state = s.clone();
        let on_state = StateChangedEventHandler::create(Box::new(move |op, _| {
            let Some(op) = op else { return Ok(()) };
            let status = status_of(&op);
            let finished = matches!(status, "completed" | "failed" | "cancelled" | "blocked");
            let mut completed_path = None;
            update(&s_state, id, |d| {
                // Descartado por el usuario: queda asi, aunque WebView2 lo
                // reporte despues como bloqueado (ver decide).
                if d.status != "cancelled" {
                    d.status = status.into();
                }
                if status == "completed" {
                    d.received = d.total.unwrap_or(d.received);
                    completed_path = d.path.clone();
                }
            });
            if let Some(path) = completed_path {
                ensure_mark_of_the_web(&path, &unsafe { read_pwstr(|p| op.Uri(p)) });
            }
            if finished {
                OPS.with(|ops| ops.borrow_mut().remove(&id));
            }
            sync::push_downloads(&s_state);
            Ok(())
        }));
        let mut token = 0i64;
        unsafe {
            op.add_BytesReceivedChanged(&on_bytes, &mut token)?;
            op.add_StateChanged(&on_state, &mut token)?;
        }

        sync::push_downloads(&s);
        // Como el cuadro de descargas de Chrome/Edge: que se vea que empezo.
        panels::open_held(&s, Side::Left);
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { wv4.add_DownloadStarting(&handler, &mut token) };
}

/// WebView2 avisa cuando se va a guardar un archivo de un tipo peligroso
/// segun la lista del sistema (ejecutables, scripts, accesos directos...).
/// Si es una de nuestras descargas, queda en espera de que el usuario la
/// conserve o la descarte desde el panel (ver decide); si no (una pagina
/// guardando un archivo por su cuenta), no se guarda.
fn on_security_check(state: &Shared, webview: &WebView) {
    let Ok(wv26) = webview.webview().cast::<ICoreWebView2_26>() else { return };
    let s = state.clone();
    let handler = SaveFileSecurityCheckStartingEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let path = PathBuf::from(unsafe { read_pwstr(|p| args.FilePath(p)) });
        // WebView2 avisa por CUALQUIER archivo (una imagen, un PDF...), no
        // solo por los peligrosos: los demas siguen sin preguntar nada.
        let ext = unsafe { read_pwstr(|p| args.FileExtension(p)) };
        let ext = if ext.is_empty() {
            path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            ext
        };
        if !is_dangerous_extension(&ext) {
            return Ok(());
        }
        let id = find_download(&s, &path);
        let Some(id) = id else {
            eprintln!("[seguridad] bloqueado: una pagina intento guardar {}", path.display());
            unsafe { args.SetCancelSave(true)? };
            return Ok(());
        };
        let deferral = unsafe { args.GetDeferral()? };
        PENDING.with(|p| p.borrow_mut().insert(id, (args.clone(), deferral)));
        update(&s, id, |d| d.dangerous = true);
        sync::push_downloads(&s);
        panels::open_held(&s, Side::Left);
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { wv26.add_SaveFileSecurityCheckStarting(&handler, &mut token) };
}

/// Tipos de archivo que pueden ejecutar codigo al abrirlos (o instalar algo,
/// o esconder un ejecutable adentro, como las imagenes de disco): solo estos
/// esperan "Conservar" en el panel. Imagenes, videos, PDF, documentos,
/// .zip... se descargan sin preguntar.
const DANGEROUS_EXTENSIONS: &[&str] = &[
    // Programas e instaladores
    "exe", "com", "scr", "pif", "msi", "msp", "msix", "msixbundle", "appx", "appxbundle", "appinstaller",
    "application", "appref-ms", "gadget", "cpl", "dll", "ocx", "sys", "drv", "jar", "jnlp",
    // Scripts
    "bat", "cmd", "ps1", "ps1xml", "psc1", "psm1", "psd1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "ws", "wsc",
    "hta", "py", "pyw", "pyc",
    // Accesos directos, registro y configuracion del sistema
    "lnk", "url", "website", "scf", "reg", "inf", "msc", "settingcontent-ms", "library-ms", "search-ms",
    "searchconnector-ms",
    // Ayuda compilada y complementos de Office
    "chm", "hlp", "xll",
    // Imagenes de disco: se montan con un clic y lo de adentro no lleva la marca de la Web
    "iso", "img", "vhd", "vhdx",
];

fn is_dangerous_extension(ext: &str) -> bool {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    DANGEROUS_EXTENSIONS.contains(&ext.as_str())
}

/// La descarga en curso que se guarda en `path` (comparando sin mayusculas,
/// como Windows; si no, por nombre de archivo).
fn find_download(state: &Shared, path: &Path) -> Option<u64> {
    let st = state.borrow();
    let active = || st.downloads.iter().rev().filter(|d| d.status == "downloading" || d.status == "paused");
    let same = |a: &Path, b: &Path| a.as_os_str().to_string_lossy().eq_ignore_ascii_case(&b.as_os_str().to_string_lossy());
    active()
        .find(|d| d.path.as_deref().is_some_and(|p| same(p, path)))
        .or_else(|| {
            let name = path.file_name()?;
            active().find(|d| d.path.as_deref().and_then(Path::file_name).is_some_and(|n| same(Path::new(n), Path::new(name))))
        })
        .map(|d| d.id)
}

/// Pausar / reanudar / cancelar una descarga en curso, o conservar /
/// descartar un archivo peligroso (botones del panel).
pub fn control(state: &Shared, id: u64, action: &str) {
    let pending = PENDING.with(|p| p.borrow().contains_key(&id));
    match action {
        "keep" | "discard" => return decide(state, id, action == "keep"),
        "cancel" if pending => return decide(state, id, false),
        _ => {}
    }
    let op = OPS.with(|ops| ops.borrow().get(&id).cloned());
    let Some(op) = op else { return };
    let _ = unsafe {
        match action {
            "pause" => op.Pause(),
            "resume" => op.Resume(),
            "cancel" => op.Cancel(),
            _ => return,
        }
    };
    // Pausar no siempre dispara StateChanged enseguida: reflejarlo ya.
    let status = status_of(&op);
    update(state, id, |d| d.status = status.into());
    sync::push_downloads(state);
}

/// La decision del usuario sobre un archivo peligroso. Al conservarlo se
/// salta el aviso propio de WebView2 (no se veria: su cuadro de descargas
/// esta oculto, y el usuario ya eligio).
fn decide(state: &Shared, id: u64, keep: bool) {
    let Some((args, deferral)) = PENDING.with(|p| p.borrow_mut().remove(&id)) else { return };
    unsafe {
        let _ = if keep { args.SetSuppressDefaultPolicy(true) } else { args.SetCancelSave(true) };
        let _ = deferral.Complete();
    }
    update(state, id, |d| {
        d.dangerous = false;
        if !keep {
            d.status = "cancelled".into();
        }
    });
    sync::push_downloads(state);
}

fn status_of(op: &ICoreWebView2DownloadOperation) -> &'static str {
    let mut st = COREWEBVIEW2_DOWNLOAD_STATE::default();
    let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON::default();
    unsafe {
        let _ = op.State(&mut st);
        let _ = op.InterruptReason(&mut reason);
    }
    if st == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
        "completed"
    } else if st == COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS {
        "downloading"
    } else if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED {
        "paused"
    } else if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED {
        "cancelled"
    } else if [
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_MALICIOUS,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_BLOCKED_BY_POLICY,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_SECURITY_CHECK_FAILED,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_HASH_MISMATCH,
    ]
    .contains(&reason)
    {
        // SmartScreen, el antivirus o una directiva lo freno.
        "blocked"
    } else {
        // Interrumpida por otra razon: se puede reintentar si WebView2 lo permite.
        let mut can_resume = BOOL(0);
        let _ = unsafe { op.CanResume(&mut can_resume) };
        if can_resume.as_bool() {
            "paused"
        } else {
            "failed"
        }
    }
}

fn total_bytes(op: &ICoreWebView2DownloadOperation) -> Option<u64> {
    let mut total = 0i64;
    let _ = unsafe { op.TotalBytesToReceive(&mut total) };
    (total > 0).then_some(total as u64)
}

fn update(state: &Shared, id: u64, f: impl FnOnce(&mut DownloadEntry)) {
    if let Some(d) = state.borrow_mut().downloads.iter_mut().find(|d| d.id == id) {
        f(d);
    }
}

/// Marca de la Web (Mark of the Web): el flujo alternativo Zone.Identifier
/// que dice "este archivo vino de Internet". Con ella Windows SmartScreen
/// revisa un programa antes de ejecutarlo y Office abre los documentos en
/// Vista protegida. WebView2 normalmente ya la pone; si falta, la ponemos.
/// (En discos sin NTFS, como muchos pendrives, no se puede: se ignora.)
fn ensure_mark_of_the_web(path: &Path, source: &str) {
    let stream = PathBuf::from(format!("{}:Zone.Identifier", path.display()));
    if stream.exists() {
        return;
    }
    let _ = std::fs::write(&stream, zone_identifier(source));
}

fn zone_identifier(source: &str) -> String {
    let lower = source.to_ascii_lowercase();
    let host_url = if lower.starts_with("https://") || lower.starts_with("http://") {
        // Sin usuario/contrasena que pudiera traer la URL, y en una sola linea.
        let (scheme, rest) = source.split_once("://").unwrap_or(("", source));
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let host = rest[..end].rsplit_once('@').map_or(&rest[..end], |(_, h)| h);
        format!("{scheme}://{host}{}", &rest[end..]).replace(['\r', '\n'], "")
    } else {
        "about:internet".to_string()
    };
    format!("[ZoneTransfer]\r\nZoneId=3\r\nHostUrl={host_url}\r\n")
}

/// "archivo.zip" -> "archivo (1).zip" si ya existe (como Chrome), para no
/// pisar una descarga anterior.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|candidate| !candidate.exists())
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names_do_not_overwrite() {
        let dir = std::env::temp_dir().join(format!("navegador-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unique_path(&dir, "a.zip"), dir.join("a.zip"));
        std::fs::write(dir.join("a.zip"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.zip"), dir.join("a (1).zip"));
        std::fs::write(dir.join("a (1).zip"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.zip"), dir.join("a (2).zip"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_risky_file_types_ask_before_saving() {
        for ext in [".exe", "MSI", ".ps1", ".lnk", ".iso", ".js"] {
            assert!(is_dangerous_extension(ext), "{ext}");
        }
        for ext in [".png", ".jpg", ".pdf", ".zip", ".mp4", ".docx", ".txt", ""] {
            assert!(!is_dangerous_extension(ext), "{ext}");
        }
    }

    #[test]
    fn mark_of_the_web_says_internet_without_credentials() {
        assert_eq!(
            zone_identifier("https://ana:clave@example.com/f/setup.exe?x=1"),
            "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=https://example.com/f/setup.exe?x=1\r\n"
        );
        assert_eq!(zone_identifier("blob:https://example.com/123"), "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=about:internet\r\n");
    }

    #[cfg(windows)]
    #[test]
    fn writes_the_zone_identifier_stream() {
        let dir = std::env::temp_dir().join(format!("navegador-motw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("setup.exe");
        std::fs::write(&file, b"MZ").unwrap();
        ensure_mark_of_the_web(&file, "https://example.com/setup.exe");
        let zone = std::fs::read_to_string(format!("{}:Zone.Identifier", file.display())).unwrap();
        assert!(zone.contains("ZoneId=3"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
