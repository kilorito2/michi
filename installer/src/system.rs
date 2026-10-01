//! Lo que el instalador toca de Windows: registro (lista de programas,
//! navegador del sistema), accesos directos, procesos del navegador.
//! Todo en el usuario actual (HKCU, carpetas del usuario): nunca hace falta
//! ser administrador.

use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegGetValueW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};

pub const UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Michi";
const CLIENT_KEY: &str = "Software\\Clients\\StartMenuInternet\\Michi";
const URL_CLASS: &str = "MichiURL";
const HTML_CLASS: &str = "MichiHTML";
/// Nombre con el que figura en "Aplicaciones predeterminadas".
pub const REGISTERED_NAME: &str = "Michi";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub enum Value<'a> {
    Str(&'a str),
    Dword(u32),
}

/// Escribe un valor en HKCU\`path` (`name` None = el valor predeterminado).
pub fn reg_set(path: &str, name: Option<&str>, value: Value) -> Result<(), String> {
    let mut key: HKEY = std::ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(path).as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("No se pudo escribir en el registro ({path}, error {status})."));
    }
    let name_w = name.map(wide);
    let name_ptr = name_w.as_ref().map_or(std::ptr::null(), |n| n.as_ptr());
    let status = match value {
        Value::Str(s) => {
            let data = wide(s);
            unsafe { RegSetValueExW(key, name_ptr, 0, REG_SZ, data.as_ptr() as *const u8, (data.len() * 2) as u32) }
        }
        Value::Dword(d) => unsafe {
            RegSetValueExW(key, name_ptr, 0, REG_DWORD, &d as *const u32 as *const u8, 4)
        },
    };
    unsafe { RegCloseKey(key) };
    if status != ERROR_SUCCESS {
        return Err(format!("No se pudo escribir en el registro ({path}, error {status})."));
    }
    Ok(())
}

pub fn reg_get(path: &str, name: &str) -> Option<String> {
    let mut buf = vec![0u16; 2048];
    let mut size = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide(path).as_ptr(),
            wide(name).as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Borra HKCU\`path` con todo lo que tenga adentro (si no existe, nada).
pub fn reg_delete(path: &str) {
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(path).as_ptr()) };
}

fn reg_delete_value(path: &str, name: &str) {
    let mut key: HKEY = std::ptr::null_mut();
    let ok = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(path).as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    } == ERROR_SUCCESS;
    if ok {
        unsafe {
            RegDeleteValueW(key, wide(name).as_ptr());
            RegCloseKey(key);
        }
    }
}

/// Una instalacion anterior: (version, carpeta).
pub fn installed() -> Option<(String, PathBuf)> {
    let dir = reg_get(UNINSTALL_KEY, "InstallLocation")?;
    let version = reg_get(UNINSTALL_KEY, "DisplayVersion").unwrap_or_default();
    Some((version, PathBuf::from(dir)))
}

/// Registra a Michi como navegador del sistema: aparece en
/// "Aplicaciones predeterminadas" y en "Abrir con" para enlaces y .html.
/// (Windows no deja que un programa se ponga solo como predeterminado: lo
/// elige el usuario en Configuracion.)
pub fn register_browser(exe: &Path) -> Result<(), String> {
    let exe = exe.display().to_string();
    let icon = format!("\"{exe}\",0");
    let open = format!("\"{exe}\" \"%1\"");
    let caps = format!("{CLIENT_KEY}\\Capabilities");
    reg_set(CLIENT_KEY, None, Value::Str(REGISTERED_NAME))?;
    reg_set(&format!("{CLIENT_KEY}\\DefaultIcon"), None, Value::Str(&icon))?;
    reg_set(&format!("{CLIENT_KEY}\\shell\\open\\command"), None, Value::Str(&format!("\"{exe}\"")))?;
    reg_set(&caps, Some("ApplicationName"), Value::Str(REGISTERED_NAME))?;
    reg_set(&caps, Some("ApplicationIcon"), Value::Str(&icon))?;
    reg_set(
        &caps,
        Some("ApplicationDescription"),
        Value::Str("Navegador web con pestanas laterales, fondos animados y seguridad en serio."),
    )?;
    reg_set(&format!("{caps}\\StartMenu"), Some("StartMenuInternet"), Value::Str(REGISTERED_NAME))?;
    for scheme in ["http", "https"] {
        reg_set(&format!("{caps}\\URLAssociations"), Some(scheme), Value::Str(URL_CLASS))?;
    }
    for ext in [".htm", ".html", ".shtml", ".xht", ".xhtml", ".svg"] {
        reg_set(&format!("{caps}\\FileAssociations"), Some(ext), Value::Str(HTML_CLASS))?;
    }
    for (class, name) in [(URL_CLASS, "Direccion web (Michi)"), (HTML_CLASS, "Pagina web (Michi)")] {
        let root = format!("Software\\Classes\\{class}");
        reg_set(&root, None, Value::Str(name))?;
        if class == URL_CLASS {
            reg_set(&root, Some("URL Protocol"), Value::Str(""))?;
        }
        reg_set(&format!("{root}\\DefaultIcon"), None, Value::Str(&icon))?;
        reg_set(&format!("{root}\\shell\\open\\command"), None, Value::Str(&open))?;
    }
    reg_set("Software\\RegisteredApplications", Some(REGISTERED_NAME), Value::Str(&caps))?;
    notify_associations();
    Ok(())
}

pub fn unregister_browser() {
    reg_delete(CLIENT_KEY);
    reg_delete(&format!("Software\\Classes\\{URL_CLASS}"));
    reg_delete(&format!("Software\\Classes\\{HTML_CLASS}"));
    reg_delete_value("Software\\RegisteredApplications", REGISTERED_NAME);
    notify_associations();
}

/// Entrada en "Aplicaciones instaladas" de la version de antes del cambio de
/// nombre (se llamaba "Navegador").
pub const LEGACY_UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Navegador";

/// Borra el registro que dejo la version "Navegador": navegador del sistema,
/// tipos de enlace/archivo y "Aplicaciones instaladas". Sin esto, "navegador.exe"
/// seguia apareciendo en "Aplicaciones predeterminadas" al lado de Michi.
pub fn unregister_legacy() {
    reg_delete("Software\\Clients\\StartMenuInternet\\Navegador");
    reg_delete("Software\\Classes\\NavegadorURL");
    reg_delete("Software\\Classes\\NavegadorHTML");
    reg_delete_value("Software\\RegisteredApplications", "Navegador");
    reg_delete(LEGACY_UNINSTALL_KEY);
    notify_associations();
}

const MUI_CACHE: &str = "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\Shell\\MuiCache";

/// Windows guarda en cache el nombre visible de cada .exe la primera vez que lo
/// muestra ("Abrir con", "Aplicaciones predeterminadas"). Las versiones de Michi
/// sin informacion de version quedaron como "michi.exe": se borra la cache para
/// que lo vuelva a leer del .exe nuevo ("Michi").
pub fn forget_cached_name(exe: &Path) {
    let exe = exe.display().to_string();
    for suffix in ["FriendlyAppName", "ApplicationCompany"] {
        reg_delete_value(MUI_CACHE, &format!("{exe}.{suffix}"));
    }
}

/// Accesos directos de la version "Navegador" (apuntan a un .exe que se va).
pub fn legacy_shortcuts() -> Vec<PathBuf> {
    [
        dirs::data_dir().map(|d| d.join("Microsoft\\Windows\\Start Menu\\Programs\\Navegador.lnk")),
        dirs::desktop_dir().map(|d| d.join("Navegador.lnk")),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn notify_associations() {
    use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, std::ptr::null(), std::ptr::null()) };
}

/// Crea (o reemplaza) un acceso directo .lnk a `exe`.
pub fn shortcut(lnk: &Path, exe: &Path, description: &str) -> Result<(), String> {
    use windows::core::{Interface, HSTRING};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    let fail = |e: windows::core::Error| format!("No se pudo crear el acceso directo {} ({}).", lnk.display(), e.message());
    if let Some(parent) = lnk.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    unsafe {
        // En este hilo (el de la instalacion); si ya estaba iniciado, da igual.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(fail)?;
        link.SetPath(&HSTRING::from(exe)).map_err(fail)?;
        if let Some(dir) = exe.parent() {
            link.SetWorkingDirectory(&HSTRING::from(dir)).map_err(fail)?;
        }
        link.SetDescription(&HSTRING::from(description)).map_err(fail)?;
        link.SetIconLocation(&HSTRING::from(exe), 0).map_err(fail)?;
        let file: IPersistFile = link.cast().map_err(fail)?;
        file.Save(&HSTRING::from(lnk), true).map_err(fail)?;
    }
    Ok(())
}

pub fn start_menu_lnk() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("Microsoft\\Windows\\Start Menu\\Programs\\Michi.lnk"))
}

pub fn desktop_lnk() -> Option<PathBuf> {
    Some(dirs::desktop_dir()?.join("Michi.lnk"))
}

/// Procesos de michi.exe que corren desde `dir` (el navegador abierto
/// no deja reemplazar ni borrar sus archivos).
pub fn running_in(dir: &Path) -> Vec<u32> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let dir = dir.display().to_string().to_lowercase();
    let mut found = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 {
            return found;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snap, &mut entry) != 0;
        while more {
            let len = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            if name.eq_ignore_ascii_case("michi.exe") {
                if let Some(path) = process_path(entry.th32ProcessID) {
                    if path.to_lowercase().starts_with(&dir) {
                        found.push(entry.th32ProcessID);
                    }
                }
            }
            more = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    found
}

fn process_path(pid: u32) -> Option<String> {
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(handle);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Cierra esos procesos (el usuario lo pidio desde el instalador).
pub fn close_processes(pids: &[u32]) {
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_TERMINATE};
    const SYNCHRONIZE: u32 = 0x0010_0000;
    for &pid in pids {
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE, 0, pid);
            if !handle.is_null() {
                TerminateProcess(handle, 0);
                WaitForSingleObject(handle, 5000);
                CloseHandle(handle);
            }
        }
    }
}

/// Mismo endurecimiento que el navegador (security::harden_process): un
/// instalador suele ejecutarse desde Descargas, justo donde alguien podria
/// dejar un .dll con el nombre justo para que se cargue adentro.
pub fn harden_process() {
    use windows_sys::Win32::System::LibraryLoader::{SetDefaultDllDirectories, SetDllDirectoryW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS};
    unsafe {
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
        let empty = [0u16];
        SetDllDirectoryW(empty.as_ptr());
    }
}

/// Tamano total de una carpeta, en KB (para "Aplicaciones instaladas").
pub fn folder_kb(dir: &Path) -> u64 {
    fn walk(dir: &Path) -> u64 {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| match e.metadata() {
                        Ok(m) if m.is_dir() => walk(&e.path()),
                        Ok(m) => m.len(),
                        Err(_) => 0,
                    })
                    .sum()
            })
            .unwrap_or(0)
    }
    walk(dir) / 1024
}
