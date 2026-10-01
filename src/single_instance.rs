//! Un solo Michi abierto a la vez.
//!
//! Cuando Michi es el navegador predeterminado, Windows abre cada enlace o
//! archivo .html ejecutando `michi.exe <direccion>`: sin esto, cada uno
//! levantaba un navegador entero nuevo (otra ventana, otro proceso). Ahora el
//! primer Michi es el "principal": los que se abren despues le pasan lo pedido
//! por un pipe con nombre y terminan; el principal lo abre en una pestana nueva
//! y se trae al frente.
//!
//! Los nombres del mutex y del pipe llevan un hash de la ruta del .exe: cada
//! instalacion (y cada compilacion de prueba) es su propio "un solo Michi".

use std::hash::{Hash, Hasher};

use winit::event_loop::EventLoopProxy;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, ReadFile, WriteFile, OPEN_EXISTING, PIPE_ACCESS_INBOUND};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, WaitNamedPipeW, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};

use crate::state::UserEvent;

const GENERIC_WRITE: u32 = 0x4000_0000;
/// Lo maximo que se acepta por el pipe (una direccion, no mas).
const MAX_MESSAGE: usize = 64 * 1024;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Identificador de esta instalacion: hash de la ruta del .exe.
fn install_id() -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_lowercase())
        .unwrap_or_default()
        .hash(&mut h);
    h.finish()
}

fn pipe_name() -> Vec<u16> {
    wide(&format!("\\\\.\\pipe\\Michi-{:016x}", install_id()))
}

/// Solo lo que un enlace o un archivo puede pedir: web y archivos locales.
/// Nunca javascript:, data:, app:// ni nada que no se navegue. Lo mismo que
/// acepta main::url_from_args, revisado de nuevo del lado del principal: el
/// pipe es una entrada mas y no se le cree a ciegas.
pub fn is_external_target(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("file:///")
}

/// Si ya hay un Michi abierto (de esta instalacion), le pasa `url` (None = solo
/// traerlo al frente) y devuelve false: este proceso tiene que terminar sin
/// abrir otra ventana. Si no hay ninguno, este pasa a ser el principal y
/// devuelve true. Si el principal no contesta (se esta cerrando, colgado), se
/// devuelve true igual: mejor otra ventana que un enlace que "no hace nada".
pub fn claim(url: Option<&str>) -> bool {
    let name = wide(&format!("Local\\Michi-{:016x}", install_id()));
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if mutex.is_null() {
        return true;
    }
    if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        // Somos el principal: el mutex queda abierto hasta que termine el
        // proceso (Windows lo libera solo al salir, incluso si se cuelga).
        return true;
    }
    unsafe { CloseHandle(mutex) };
    !forward(url)
}

/// Le pasa al principal lo pedido. Mensaje: "1<url>" para abrir, "0" para solo
/// traerlo al frente (un mensaje vacio no despertaria la lectura del otro lado).
fn forward(url: Option<&str>) -> bool {
    let message = match url {
        Some(u) => format!("1{u}"),
        None => "0".to_string(),
    };
    // El principal se trae al frente con SetForegroundWindow, y Windows solo
    // se lo permite si el proceso que esta en primer plano (este, recien
    // abierto) le cede el permiso.
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    let pipe = pipe_name();
    for _ in 0..3 {
        let handle = unsafe {
            CreateFileW(pipe.as_ptr(), GENERIC_WRITE, 0, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut())
        };
        if handle != INVALID_HANDLE_VALUE {
            let mut written = 0u32;
            let ok = unsafe {
                WriteFile(handle, message.as_ptr(), message.len() as u32, &mut written, std::ptr::null_mut())
            } != 0;
            unsafe { CloseHandle(handle) };
            return ok && written as usize == message.len();
        }
        // Todas las instancias del pipe ocupadas: esperar a que se libere una.
        if unsafe { GetLastError() } != ERROR_PIPE_BUSY {
            return false;
        }
        unsafe { WaitNamedPipeW(pipe.as_ptr(), 2000) };
    }
    false
}

/// Del lado del principal: escucha a los Michi que se abran despues y le avisa
/// al bucle de eventos con UserEvent::OpenExternal. Corre en su propio hilo
/// (ConnectNamedPipe bloquea hasta que llega alguien).
pub fn listen(proxy: EventLoopProxy<UserEvent>) {
    std::thread::spawn(move || {
        let name = pipe_name();
        loop {
            let pipe = unsafe {
                CreateNamedPipeW(
                    name.as_ptr(),
                    PIPE_ACCESS_INBOUND,
                    // Nunca desde otra PC de la red.
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    PIPE_UNLIMITED_INSTANCES,
                    0,
                    MAX_MESSAGE as u32,
                    0,
                    std::ptr::null(),
                )
            };
            if pipe == INVALID_HANDLE_VALUE {
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
            let connected =
                unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            if connected {
                if let Some(event) = parse(&read_all(pipe)) {
                    if proxy.send_event(event).is_err() {
                        // El bucle de eventos ya termino: el navegador se cierra.
                        unsafe { CloseHandle(pipe) };
                        return;
                    }
                }
            }
            unsafe {
                DisconnectNamedPipe(pipe);
                CloseHandle(pipe);
            }
        }
    });
}

/// Lee hasta que el otro lado cierra (o hasta MAX_MESSAGE).
fn read_all(pipe: windows_sys::Win32::Foundation::HANDLE) -> Vec<u8> {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let mut read = 0u32;
        let ok = unsafe { ReadFile(pipe, buf.as_mut_ptr(), buf.len() as u32, &mut read, std::ptr::null_mut()) } != 0;
        data.extend_from_slice(&buf[..read as usize]);
        if !ok || read == 0 || data.len() > MAX_MESSAGE {
            break;
        }
    }
    data
}

/// "0" = traer al frente; "1<url>" = abrir esa direccion (si es valida).
fn parse(data: &[u8]) -> Option<UserEvent> {
    let text = std::str::from_utf8(data).ok()?;
    match text.split_at_checked(1)? {
        ("0", "") => Some(UserEvent::OpenExternal(None)),
        ("1", url) if is_external_target(url) => Some(UserEvent::OpenExternal(Some(url.to_string()))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened(event: Option<UserEvent>) -> Option<Option<String>> {
        match event {
            Some(UserEvent::OpenExternal(url)) => Some(url),
            _ => None,
        }
    }

    #[test]
    fn accepts_only_web_and_local_files() {
        assert_eq!(opened(parse(b"1https://youtube.com")), Some(Some("https://youtube.com".into())));
        assert_eq!(opened(parse(b"1file:///C:/x/pagina.html")), Some(Some("file:///C:/x/pagina.html".into())));
        assert_eq!(opened(parse(b"0")), Some(None));
        // Nada que ejecute codigo ni paginas propias.
        assert_eq!(opened(parse(b"1javascript:alert(1)")), None);
        assert_eq!(opened(parse(b"1data:text/html,hola")), None);
        assert_eq!(opened(parse(b"1app://localhost/settings")), None);
        // Mensajes mal formados.
        assert_eq!(opened(parse(b"")), None);
        assert_eq!(opened(parse(b"0extra")), None);
        assert_eq!(opened(parse(b"2https://x.com")), None);
        assert_eq!(opened(parse(&[0xff, 0xfe])), None);
    }
}
