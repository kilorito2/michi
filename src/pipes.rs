//! Pipes con nombre propios (ver single_instance.rs y control.rs), solo para
//! el usuario de Windows que abrio Michi.
//!
//! Los nombres de pipe son de TODA la PC, no de cada sesion: otro usuario
//! del mismo equipo podia crear antes un pipe con el mismo nombre (es
//! predecible) y quedarse con las conexiones, o sumarle instancias al
//! nuestro. Por eso:
//! - El nombre lleva el SID del usuario.
//! - El pipe se crea con permisos solo para ese usuario (y SYSTEM), y su
//!   primera instancia con FILE_FLAG_FIRST_PIPE_INSTANCE: si el nombre ya
//!   existe (alguien lo tomo antes), no se usa.
//! - Siempre queda al menos una instancia nuestra abierta (ver Listener): si
//!   no, en el hueco entre cerrar una y crear la siguiente otro la podia crear.
//! - Quien se conecta (otro michi.exe) mira que el dueno del pipe sea su mismo
//!   usuario antes de mandar nada, y se conecta con "solo identificacion": un
//!   servidor falso no puede hacerse pasar por el.

use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
    SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, TokenUser, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_FIRST_PIPE_INSTANCE;
use windows_sys::Win32::System::Pipes::CreateNamedPipeW;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// El usuario de este proceso: (SID binario, SID en texto "S-1-5-21-...").
fn user() -> Option<&'static (Vec<u8>, String)> {
    static USER: OnceLock<Option<(Vec<u8>, String)>> = OnceLock::new();
    USER.get_or_init(|| unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) != 0;
        CloseHandle(token);
        if !ok {
            return None;
        }
        let sid = (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid;
        let mut text: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(sid, &mut text) == 0 {
            return None;
        }
        let n = (0..).take_while(|&i| *text.add(i) != 0).count();
        let string = String::from_utf16_lossy(std::slice::from_raw_parts(text, n));
        LocalFree(text.cast());
        // Una copia del SID binario (vive adentro de `buf`).
        let sid_len = windows_sys::Win32::Security::GetLengthSid(sid) as usize;
        let bytes = std::slice::from_raw_parts(sid as *const u8, sid_len).to_vec();
        Some((bytes, string))
    })
    .as_ref()
}

/// El SID del usuario en texto (para el nombre de los pipes y control.json).
pub fn user_sid() -> &'static str {
    user().map_or("sin-usuario", |(_, s)| s.as_str())
}

/// "\\.\pipe\<base>-<SID>": cada usuario de la PC, su propio nombre.
pub fn name(base: &str) -> String {
    format!("\\\\.\\pipe\\{base}-{}", user_sid())
}

/// Descriptor de seguridad "solo este usuario y SYSTEM", con este usuario de
/// dueno. Se arma una vez y vive todo el programa.
fn security() -> Option<&'static SecurityDescriptor> {
    static SD: OnceLock<Option<SecurityDescriptor>> = OnceLock::new();
    SD.get_or_init(|| {
        let (_, sid) = user()?;
        let sddl = wide(&format!("O:{sid}D:P(A;;GA;;;{sid})(A;;GA;;;SY)"));
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut())
        } != 0;
        ok.then_some(SecurityDescriptor(sd))
    })
    .as_ref()
}

struct SecurityDescriptor(PSECURITY_DESCRIPTOR);
// Solo se lee (se pasa a CreateNamedPipeW) y nunca se libera.
unsafe impl Send for SecurityDescriptor {}
unsafe impl Sync for SecurityDescriptor {}

/// Crea una instancia del pipe. `first`: tiene que ser la primera de ese
/// nombre en todo el equipo (si ya existe una, de quien sea, falla).
/// `open_mode`: PIPE_ACCESS_INBOUND / PIPE_ACCESS_DUPLEX. None si no se pudo.
pub fn create(name: &[u16], open_mode: u32, pipe_mode: u32, out_buf: u32, in_buf: u32, first: bool) -> Option<HANDLE> {
    let sd = security()?;
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: 0,
    };
    let flags = open_mode | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
    let pipe = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            flags,
            pipe_mode,
            windows_sys::Win32::System::Pipes::PIPE_UNLIMITED_INSTANCES,
            out_buf,
            in_buf,
            0,
            &sa,
        )
    };
    (pipe != INVALID_HANDLE_VALUE).then_some(pipe)
}

/// Si el dueno del pipe abierto en `handle` es este mismo usuario (lo crearon
/// nuestros permisos de `create`, no otra cuenta de la PC).
pub fn owned_by_me(handle: HANDLE) -> bool {
    let Some((me, _)) = user() else { return false };
    let mut owner: PSID = std::ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    if status != 0 {
        return false;
    }
    let same = !owner.is_null() && unsafe { EqualSid(owner, me.as_ptr() as PSID) } != 0;
    unsafe { LocalFree(sd.cast()) };
    same
}

/// Escucha en un pipe dejando SIEMPRE una instancia nuestra abierta: la
/// siguiente se crea antes de soltar la que acaba de conectarse.
pub struct Listener {
    name: Vec<u16>,
    open_mode: u32,
    pipe_mode: u32,
    out_buf: u32,
    in_buf: u32,
    next: Option<HANDLE>,
}

impl Listener {
    pub fn new(name: &str, open_mode: u32, pipe_mode: u32, out_buf: u32, in_buf: u32) -> Self {
        Self { name: wide(name), open_mode, pipe_mode, out_buf, in_buf, next: None }
    }

    /// La proxima conexion (bloquea hasta que llega alguien). Si el nombre lo
    /// tiene otro, reintenta cada segundo: nunca escucha en un pipe ajeno.
    pub fn accept(&mut self) -> HANDLE {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_PIPE_CONNECTED};
        use windows_sys::Win32::System::Pipes::ConnectNamedPipe;
        loop {
            let pipe = match self.next.take() {
                Some(p) => p,
                None => match create(&self.name, self.open_mode, self.pipe_mode, self.out_buf, self.in_buf, true) {
                    Some(p) => p,
                    None => {
                        eprintln!("[seguridad] no se pudo crear el pipe (¿lo tiene otro programa?)");
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        continue;
                    }
                },
            };
            let connected =
                unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            // La que sigue, mientras esta todavia existe (sin hueco).
            self.next = create(&self.name, self.open_mode, self.pipe_mode, self.out_buf, self.in_buf, false);
            if connected {
                return pipe;
            }
            unsafe { CloseHandle(pipe) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipes_are_per_user_and_only_ours() {
        assert!(user_sid().starts_with("S-1-"));
        let n = name("Michi-prueba");
        assert!(n.starts_with("\\\\.\\pipe\\Michi-prueba-S-1-"));

        use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_INBOUND;
        use windows_sys::Win32::System::Pipes::PIPE_TYPE_BYTE;
        let unique = wide(&format!("{n}-{}", std::process::id()));
        let first = create(&unique, PIPE_ACCESS_INBOUND, PIPE_TYPE_BYTE, 0, 0, true).expect("primera instancia");
        assert!(owned_by_me(first));
        // Una segunda "primera instancia" con el mismo nombre no se puede.
        assert!(create(&unique, PIPE_ACCESS_INBOUND, PIPE_TYPE_BYTE, 0, 0, true).is_none());
        // Una instancia mas, con la primera abierta, si.
        let second = create(&unique, PIPE_ACCESS_INBOUND, PIPE_TYPE_BYTE, 0, 0, false).expect("otra instancia");
        unsafe {
            CloseHandle(second);
            CloseHandle(first);
        }
    }
}
