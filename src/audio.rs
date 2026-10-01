//! El nombre y el icono de Michi en el mezclador de volumen.
//!
//! El sonido de las paginas no sale de michi.exe: lo reproduce un proceso de
//! WebView2 (msedgewebview2.exe, el servicio de audio de Chromium). Por eso el
//! mezclador de Windows mostraba el icono de WebView2, y otros mezcladores
//! (SteelSeries Sonar, por ejemplo) el nombre "msedgewebview2". Windows deja
//! ponerle a cada sesion de audio un nombre y un icono propios: cada vez que una
//! pestana empieza a sonar, se buscan las sesiones de los procesos de WebView2
//! de ESTE Michi (el proceso principal de WebView2 y sus hijos) y se les pone
//! "Michi" con el icono del .exe. Nunca se toca el sonido de otro programa.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use windows::core::{Interface, HSTRING, PWSTR};
use windows::Win32::Media::Audio::{
    eRender, IAudioSessionControl, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
    MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};

pub const NAME: &str = "Michi";
const WEBVIEW_EXE: &str = "msedgewebview2.exe";

/// Hay intentos en curso (no hace falta lanzar otros).
static BUSY: AtomicBool = AtomicBool::new(false);

/// Avisa cuando una pestana empieza a sonar (ICoreWebView2_8).
pub fn attach(webview: &wry::WebView) {
    use webview2_com::IsDocumentPlayingAudioChangedEventHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;
    use wry::WebViewExtWindows;

    let Ok(core) = webview.webview().cast::<ICoreWebView2_8>() else { return };
    let handler = IsDocumentPlayingAudioChangedEventHandler::create(Box::new(|sender, _| {
        let Some(sender) = sender else { return Ok(()) };
        let mut playing = windows_core::BOOL::default();
        if let Ok(core) = sender.cast::<ICoreWebView2_8>() {
            let _ = unsafe { core.IsDocumentPlayingAudio(&mut playing) };
        }
        if playing.as_bool() {
            let mut pid = 0u32;
            let _ = unsafe { sender.BrowserProcessId(&mut pid) };
            brand_soon(pid);
        }
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { core.add_IsDocumentPlayingAudioChanged(&handler, &mut token) };
}

/// La sesion de audio aparece un instante despues de que la pagina empieza a
/// sonar: se intenta varias veces, en otro hilo (no traba la interfaz).
fn brand_soon(browser_pid: u32) {
    if browser_pid == 0 || BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        for wait in [300u64, 1200, 3000] {
            std::thread::sleep(Duration::from_millis(wait));
            let _ = brand(browser_pid);
        }
        BUSY.store(false, Ordering::SeqCst);
    });
}

/// Pone nombre e icono de Michi a las sesiones de audio de los procesos de
/// WebView2 que cuelgan de `browser_pid`. Devuelve cuantas encontro.
fn brand(browser_pid: u32) -> windows::core::Result<usize> {
    let ours = webview_family(browser_pid);
    let icon = std::env::current_exe().map(|p| format!("{},0", p.display())).unwrap_or_default();
    let mut found = 0;
    for_each_session(|control, pid| {
        if !ours.contains(&pid) {
            return;
        }
        found += 1;
        unsafe {
            if read(control.GetDisplayName()) != NAME {
                let _ = control.SetDisplayName(&HSTRING::from(NAME), std::ptr::null());
            }
            if !icon.is_empty() && read(control.GetIconPath()) != icon {
                let _ = control.SetIconPath(&HSTRING::from(icon.as_str()), std::ptr::null());
            }
        }
    })?;
    Ok(found)
}

/// Recorre las sesiones de audio de todas las salidas activas (parlantes,
/// auriculares, las virtuales de Sonar...).
fn for_each_session(mut f: impl FnMut(&IAudioSessionControl, u32)) -> windows::core::Result<()> {
    unsafe {
        // Este hilo es propio: se inicia COM aca y se cierra al terminar.
        let init = CoInitializeEx(None, COINIT_MULTITHREADED);
        let result = (|| -> windows::core::Result<()> {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            for i in 0..devices.GetCount()? {
                let Ok(device) = devices.Item(i) else { continue };
                let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
                let Ok(sessions) = manager.GetSessionEnumerator() else { continue };
                for j in 0..sessions.GetCount()? {
                    let Ok(control) = sessions.GetSession(j) else { continue };
                    let Ok(control2) = control.cast::<IAudioSessionControl2>() else { continue };
                    let Ok(pid) = control2.GetProcessId() else { continue };
                    f(&control, pid);
                }
            }
            Ok(())
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        result
    }
}

/// Lee (y libera) una cadena que devuelve la API de audio.
fn read(value: windows::core::Result<PWSTR>) -> String {
    match value {
        Ok(p) if !p.is_null() => unsafe {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        },
        _ => String::new(),
    }
}

/// Procesos de Windows: pid -> (pid del padre, nombre del .exe).
fn processes() -> HashMap<u32, (u32, String)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let mut out = HashMap::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snap, &mut entry) != 0;
        while more {
            let len = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            out.insert(entry.th32ProcessID, (entry.th32ParentProcessID, name));
            more = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    out
}

/// El proceso principal de WebView2 de Michi y todos sus descendientes que
/// sean de WebView2 (ahi vive el servicio de audio).
fn webview_family(root: u32) -> HashSet<u32> {
    let procs = processes();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, (parent, _)) in &procs {
        children.entry(*parent).or_default().push(*pid);
    }
    let mut ours = HashSet::from([root]);
    let mut pending = vec![root];
    while let Some(pid) = pending.pop() {
        for &child in children.get(&pid).into_iter().flatten() {
            let is_webview = procs.get(&child).is_some_and(|(_, name)| name.eq_ignore_ascii_case(WEBVIEW_EXE));
            if is_webview && ours.insert(child) {
                pending.push(child);
            }
        }
    }
    ours
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lista las sesiones de audio de WebView2 que hay ahora (proceso, nombre,
    /// icono). Para mirar a mano: cargo test audio -- --ignored --nocapture
    #[test]
    #[ignore]
    fn list_webview_audio_sessions() {
        let procs = processes();
        for_each_session(|control, pid| {
            let exe = procs.get(&pid).map(|(_, n)| n.as_str()).unwrap_or("?");
            if exe.eq_ignore_ascii_case(WEBVIEW_EXE) {
                let (name, icon) = unsafe { (read(control.GetDisplayName()), read(control.GetIconPath())) };
                println!("pid {pid} {exe}: nombre={name:?} icono={icon:?}");
            }
        })
        .unwrap();
    }

    #[test]
    fn family_includes_only_webview_descendants() {
        // Este proceso de prueba no es de WebView2: queda solo la raiz.
        let me = std::process::id();
        assert_eq!(webview_family(me), HashSet::from([me]));
    }
}
