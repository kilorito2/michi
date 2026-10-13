//! Gestor de contrasenas propio.
//!
//! WebView2 sabe guardar contrasenas, pero no deja verlas, buscarlas ni
//! borrarlas de a una (no tiene API para leerlas). Asi que Michi las guarda el
//! mismo, y el guardado de WebView2 queda apagado (ver security::set_autofill).
//!
//! - Guardar: ui/content_init.js avisa cuando se envia un formulario con una
//!   contrasena ("michi:pw:" + {u, p}, solo desde la pagina principal, nunca
//!   un iframe). La barra pregunta "Guardar la contrasena?" y nada se guarda
//!   sin un clic. El sitio sale de la URL real del documento que la envio (la
//!   pone WebView2): una pagina no puede ofrecer contrasenas de otro sitio.
//! - Disco: passwords.json, con cada contrasena cifrada por Windows (DPAPI,
//!   atada a tu usuario de Windows). Otro usuario de la PC, o el archivo
//!   copiado a otra PC, no la puede leer.
//! - Rellenar: solo al tocar la llave de la barra (no sola al cargar la
//!   pagina, donde cualquier script del sitio la podria leer antes que vos), y
//!   solo si la pestana sigue en el mismo sitio.
//! - Ver, copiar o editar una en Ajustes pide Windows Hello (PIN, huella o
//!   cara) si esta configurado; vale por 2 minutos.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::state::{Shared, UserEvent};
use crate::{security, storage, sync};

const FILE: &str = "passwords.json";
/// Entropia extra de DPAPI: otra app del mismo usuario no lo descifra sin ella.
const ENTROPY: &[u8] = b"Michi-contrasenas-v1";
/// Cuanto dura la verificacion de Windows Hello en Ajustes.
const VERIFIED_FOR: Duration = Duration::from_secs(120);
/// Cuanto queda la pregunta "Guardar la contrasena?" si no se responde.
const OFFER_FOR: Duration = Duration::from_secs(180);
/// Lo copiado al portapapeles se borra a los...
const CLIPBOARD_FOR: Duration = Duration::from_secs(30);

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
struct Store {
    entries: Vec<Entry>,
    /// Sitios en los que nunca se ofrece guardar ("Nunca para este sitio").
    never: Vec<String>,
    next_id: u64,
}

#[derive(Serialize, Deserialize, Clone)]
struct Entry {
    id: u64,
    /// "https://sitio.com" (esquema, host y puerto).
    origin: String,
    username: String,
    /// La contrasena cifrada con DPAPI, en base64.
    secret: String,
    created: u64,
    updated: u64,
    #[serde(default)]
    used: u64,
}

/// "Guardar la contrasena?" esperando respuesta en una pestana.
#[derive(Clone)]
pub struct Offer {
    /// Cuando se envio el formulario: a los OFFER_FOR la pregunta se olvida.
    at: Instant,
    origin: String,
    username: String,
    password: String,
    /// Ya habia una para ese usuario: se ofrece actualizarla.
    update: Option<u64>,
}

/// Lo que se pidio en Ajustes y espera la verificacion de Windows Hello.
#[derive(Debug, Clone)]
pub enum Pending {
    Reveal(u64),
    Copy(u64),
    Edit { id: u64, username: String, password: String },
}

#[derive(Default)]
pub struct Passwords {
    store: Store,
    verified_until: Option<Instant>,
    verifying: bool,
    waiting: Vec<Pending>,
}

impl Passwords {
    pub fn load() -> Self {
        Self { store: storage::load(FILE), ..Default::default() }
    }

    fn save(&self) {
        storage::save(FILE, &self.store);
    }

    fn for_origin<'a>(&'a self, origin: &'a str) -> impl Iterator<Item = &'a Entry> {
        self.store.entries.iter().filter(move |e| e.origin == origin)
    }

    /// Cuantas hay guardadas para el sitio de esa direccion (la llave de la barra).
    pub fn count_for(&self, url: &str) -> usize {
        security::origin_of(url).map_or(0, |o| self.for_origin(&o).count())
    }
}

// ---------------------------------------------------------------------------
// Cifrado (DPAPI)
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn dpapi(data: &[u8], protect: bool) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let entropy = CRYPT_INTEGER_BLOB { cbData: ENTROPY.len() as u32, pbData: ENTROPY.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = unsafe {
        if protect {
            CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        } else {
            CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        }
    };
    if ok == 0 || out.pbData.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
    unsafe { LocalFree(out.pbData as _) };
    Some(bytes)
}

fn protect(password: &str) -> Option<String> {
    use base64::Engine;
    dpapi(password.as_bytes(), true).map(|b| base64::engine::general_purpose::STANDARD.encode(b))
}

fn unprotect(secret: &str) -> Option<String> {
    use base64::Engine;
    let blob = base64::engine::general_purpose::STANDARD.decode(secret).ok()?;
    String::from_utf8(dpapi(&blob, false)?).ok()
}

// ---------------------------------------------------------------------------
// Guardar lo que se envio en una pagina
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Captured {
    #[serde(default)]
    u: String,
    p: String,
}

/// Una pagina envio un formulario con contrasena (ver ui/content_init.js).
/// `source` es la URL real del documento que lo mando (la pone WebView2).
pub fn on_captured(state: &Shared, tab_id: u64, source: &str, raw: &str) {
    let Ok(c) = serde_json::from_str::<Captured>(raw) else { return };
    let (username, password) = (c.u.trim().to_string(), c.p);
    if password.is_empty() || password.len() > 512 || username.len() > 256 {
        return;
    }
    // Solo sitios con conexion segura (o este equipo): por HTTP alguien en la
    // red pudo haber cambiado la pagina.
    let Some(origin) = security::origin_of(source).filter(|_| security::is_secure_origin(source)) else { return };
    if !(origin.starts_with("https://") || origin.starts_with("http://localhost")) {
        return;
    }
    let changed = {
        let mut st = state.borrow_mut();
        if !st.settings.save_passwords || st.passwords.store.never.contains(&origin) {
            return;
        }
        let existing = st.passwords.for_origin(&origin).find(|e| e.username == username).map(|e| (e.id, e.secret.clone()));
        let update = match existing {
            Some((_, secret)) if unprotect(&secret).as_deref() == Some(password.as_str()) => return,
            Some((id, _)) => Some(id),
            None => None,
        };
        let Some(tab) = st.tabs.iter_mut().find(|t| t.id == tab_id && !t.incognito) else { return };
        tab.password_offer = Some(Offer { at: Instant::now(), origin, username, password, update });
        true
    };
    if changed {
        sync::push_active_tab(state);
    }
}

/// Lo que muestra la barra para la oferta de la pestana activa.
pub fn offer_view(offer: &Option<Offer>) -> Value {
    match offer.as_ref().filter(|o| o.at.elapsed() < OFFER_FOR) {
        Some(o) => json!({
            "host": security::host_of(&o.origin).unwrap_or_default(),
            "username": o.username,
            "update": o.update.is_some(),
        }),
        None => Value::Null,
    }
}

/// La respuesta a "Guardar la contrasena?" de la pestana activa:
/// "save" | "never" | "dismiss".
pub fn answer(state: &Shared, choice: &str) {
    {
        let mut st = state.borrow_mut();
        let active = st.active;
        let Some(offer) = st.tabs.get_mut(active).and_then(|t| t.password_offer.take()) else { return };
        let p = &mut st.passwords;
        match choice {
            "save" => {
                let Some(secret) = protect(&offer.password) else { return };
                let now = storage::now_secs();
                match offer.update.and_then(|id| p.store.entries.iter_mut().find(|e| e.id == id)) {
                    Some(e) => {
                        e.secret = secret;
                        e.updated = now;
                    }
                    None => {
                        p.store.next_id += 1;
                        let id = p.store.next_id;
                        p.store.entries.push(Entry {
                            id,
                            origin: offer.origin,
                            username: offer.username,
                            secret,
                            created: now,
                            updated: now,
                            used: 0,
                        });
                    }
                }
                p.save();
            }
            "never" if !p.store.never.contains(&offer.origin) => {
                p.store.never.push(offer.origin);
                p.save();
            }
            _ => {}
        }
    }
    sync::push_active_tab(state);
    push_list(state);
}

// ---------------------------------------------------------------------------
// Rellenar (la llave de la barra)
// ---------------------------------------------------------------------------

/// La llave de la barra: un menu con las cuentas guardadas para el sitio de la
/// pestana activa (y "Administrar contrasenas"). Corre un bucle modal de
/// Windows, asi que no se tiene ningun prestamo de `state` mientras dura.
pub fn fill_menu(state: &Shared) {
    let (tab_id, choices) = {
        let st = state.borrow();
        let Some(tab) = st.active_tab() else { return };
        let Some(origin) = security::origin_of(&tab.url) else { return };
        let choices: Vec<(u64, String)> = st
            .passwords
            .for_origin(&origin)
            .map(|e| (e.id, if e.username.is_empty() { "(sin usuario)".to_string() } else { e.username.clone() }))
            .collect();
        (tab.id, choices)
    };
    // Con una sola cuenta, sin menu: se rellena y listo.
    if let [(id, _)] = choices.as_slice() {
        fill(state, tab_id, *id);
        return;
    }
    let mut items: Vec<(usize, String)> = choices.iter().enumerate().map(|(i, (_, u))| (i + 1, format!("Rellenar como {u}"))).collect();
    let manage = choices.len() + 1;
    items.push((0, String::new()));
    items.push((manage, "Administrar contraseñas…".into()));
    match popup_menu(&items) {
        0 => {}
        n if n == manage => state.borrow_mut().request(crate::state::Action::OpenSettings("passwords".into())),
        n => fill(state, tab_id, choices[n - 1].0),
    }
}

/// Rellena el usuario y la contrasena en la pagina de la pestana, si sigue en
/// el sitio de esa cuenta.
fn fill(state: &Shared, tab_id: u64, id: u64) {
    let script = {
        let mut st = state.borrow_mut();
        let Some(entry) = st.passwords.store.entries.iter_mut().find(|e| e.id == id) else { return };
        let (origin, username, secret) = (entry.origin.clone(), entry.username.clone(), entry.secret.clone());
        let same_site = st.tabs.iter().find(|t| t.id == tab_id).and_then(|t| security::origin_of(&t.url)).as_deref() == Some(origin.as_str());
        let Some(password) = same_site.then(|| unprotect(&secret)).flatten() else { return };
        if let Some(e) = st.passwords.store.entries.iter_mut().find(|e| e.id == id) {
            e.used = storage::now_secs();
        }
        st.passwords.save();
        fill_script(&username, &password)
    };
    let st = state.borrow();
    if let Some(tab) = st.tabs.iter().find(|t| t.id == tab_id) {
        let _ = tab.webview.evaluate_script(&script);
    }
}

/// El script que completa el formulario: el primer campo de contrasena visible
/// y el de usuario que esta antes (o el que se anuncia como usuario/correo).
/// Pone el valor como si se hubiera escrito (los sitios hechos con React y
/// similares solo se enteran por los eventos input/change).
fn fill_script(username: &str, password: &str) -> String {
    let (u, p) = (serde_json::to_string(username).unwrap_or_default(), serde_json::to_string(password).unwrap_or_default());
    format!(
        r#"(function (u, p) {{
  function visible(el) {{ return !!(el.offsetWidth || el.offsetHeight || el.getClientRects().length); }}
  function set(el, v) {{
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, v);
    el.dispatchEvent(new Event('input', {{ bubbles: true }}));
    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
  }}
  var pw = Array.prototype.filter.call(document.querySelectorAll('input[type=password]'), visible)[0];
  var root = (pw && pw.form) || document;
  var user = null;
  var inputs = Array.prototype.filter.call(root.querySelectorAll('input'), visible);
  for (var i = 0; i < inputs.length; i++) {{
    if (inputs[i] === pw) break;
    var t = (inputs[i].type || 'text').toLowerCase();
    if (t === 'text' || t === 'email' || t === 'tel') user = inputs[i];
  }}
  if (!user) user = Array.prototype.filter.call(document.querySelectorAll('input[autocomplete~=username], input[type=email]'), visible)[0];
  if (user && u) set(user, u);
  if (pw) {{ set(pw, p); pw.focus(); }}
}})({u}, {p});"#
    )
}

/// Menu emergente de Windows en la posicion del cursor. `items`: (comando,
/// texto); comando 0 = linea separadora. Devuelve el elegido (0 = ninguno).
#[cfg(windows)]
fn popup_menu(items: &[(usize, String)]) -> usize {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, PostMessageW, SetForegroundWindow, TrackPopupMenu,
        MF_SEPARATOR, MF_STRING, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
    };
    let hwnd = crate::win::main_hwnd();
    if hwnd.is_null() {
        return 0;
    }
    unsafe {
        let menu = CreatePopupMenu();
        for (cmd, text) in items {
            if *cmd == 0 {
                AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            } else {
                let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
                AppendMenuW(menu, MF_STRING, *cmd, wide.as_ptr());
            }
        }
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        // Sin esto el menu no se cierra al hacer clic fuera.
        SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y + 12, 0, hwnd, std::ptr::null());
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
        chosen.max(0) as usize
    }
}

// ---------------------------------------------------------------------------
// Ajustes > Contrasenas
// ---------------------------------------------------------------------------

/// La lista para Ajustes: sin ninguna contrasena (esas solo se mandan de a una,
/// al pedir verla, con Windows Hello).
pub fn push_list(state: &Shared) {
    let data = {
        let st = state.borrow();
        let mut entries: Vec<&Entry> = st.passwords.store.entries.iter().collect();
        entries.sort_by(|a, b| a.origin.cmp(&b.origin).then(a.username.cmp(&b.username)));
        let entries: Vec<Value> = entries
            .iter()
            .map(|e| {
                json!({
                    "id": e.id,
                    "origin": e.origin,
                    "host": security::host_of(&e.origin).unwrap_or_default(),
                    "username": e.username,
                    "updated": e.updated,
                    "icon": st.suggest.favicons.get(&e.origin),
                })
            })
            .collect();
        json!({ "entries": entries, "never": st.passwords.store.never })
    };
    sync::push_passwords(state, &data);
}

/// Ver, copiar o editar: con Windows Hello si hace rato que no se verifico.
pub fn request(state: &Shared, pending: Pending) {
    let start = {
        let mut st = state.borrow_mut();
        let p = &mut st.passwords;
        if p.verified_until.is_some_and(|t| Instant::now() < t) {
            drop(st);
            run(state, pending);
            return;
        }
        p.waiting.push(pending);
        !std::mem::replace(&mut p.verifying, true)
    };
    if start {
        let proxy = state.borrow().proxy.clone();
        let hwnd = crate::win::main_hwnd() as isize;
        std::thread::spawn(move || {
            let ok = verify(hwnd);
            if let Some(proxy) = proxy {
                let _ = proxy.send_event(UserEvent::PasswordsVerified(ok));
            }
        });
    }
}

/// Windows Hello: el PIN, la huella o la cara de la cuenta de Windows. Si la PC
/// no lo tiene configurado, no hay con que pedirlo y se sigue (como Chrome).
fn verify(hwnd: isize) -> bool {
    #[cfg(debug_assertions)]
    if std::env::var_os("MICHI_TEST_SKIP_HELLO").is_some() {
        return true;
    }
    use windows::core::{factory, HSTRING};
    use windows_future::IAsyncOperation;
    use windows::Security::Credentials::UI::{
        UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
    };
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::WinRT::IUserConsentVerifierInterop;

    let available = UserConsentVerifier::CheckAvailabilityAsync().and_then(|op| op.join());
    if !matches!(available, Ok(UserConsentVerifierAvailability::Available)) {
        return true;
    }
    let result = (|| -> windows::core::Result<UserConsentVerificationResult> {
        let interop = factory::<UserConsentVerifier, IUserConsentVerifierInterop>()?;
        let msg = HSTRING::from("Michi quiere mostrar una contraseña guardada");
        let op: IAsyncOperation<UserConsentVerificationResult> =
            unsafe { interop.RequestVerificationForWindowAsync(HWND(hwnd as *mut _), &msg)? };
        op.join()
    })();
    matches!(result, Ok(UserConsentVerificationResult::Verified))
}

/// Termino la verificacion de Windows Hello.
pub fn on_verified(state: &Shared, ok: bool) {
    let waiting = {
        let mut st = state.borrow_mut();
        let p = &mut st.passwords;
        p.verifying = false;
        if ok {
            p.verified_until = Some(Instant::now() + VERIFIED_FOR);
        }
        std::mem::take(&mut p.waiting)
    };
    if !ok {
        sync::push_toast(state, "No se verificó tu identidad: la contraseña sigue oculta");
        return;
    }
    for pending in waiting {
        run(state, pending);
    }
}

fn run(state: &Shared, pending: Pending) {
    match pending {
        Pending::Reveal(id) => {
            let secret = state.borrow().passwords.store.entries.iter().find(|e| e.id == id).map(|e| e.secret.clone());
            if let Some(password) = secret.and_then(|s| unprotect(&s)) {
                sync::push_password_revealed(state, &json!({ "id": id, "password": password }));
            }
        }
        Pending::Copy(id) => {
            let secret = state.borrow().passwords.store.entries.iter().find(|e| e.id == id).map(|e| e.secret.clone());
            if let Some(password) = secret.and_then(|s| unprotect(&s)) {
                if copy_to_clipboard(&password) {
                    sync::push_toast(state, "Contraseña copiada. Se borra del portapapeles en 30 segundos");
                }
            }
        }
        Pending::Edit { id, username, password } => {
            {
                let mut st = state.borrow_mut();
                let secret = (!password.is_empty()).then(|| protect(&password)).flatten();
                let Some(e) = st.passwords.store.entries.iter_mut().find(|e| e.id == id) else { return };
                e.username = username.trim().chars().take(256).collect();
                if let Some(secret) = secret {
                    e.secret = secret;
                }
                e.updated = storage::now_secs();
                st.passwords.save();
            }
            push_list(state);
            sync::push_toast(state, "Contraseña actualizada");
        }
    }
}

/// "Agregar" a mano en Ajustes: la direccion del sitio, el usuario y la contrasena.
pub fn add(state: &Shared, site: &str, username: &str, password: &str) {
    let site = site.trim();
    let url = if site.contains("://") { site.to_string() } else { format!("https://{site}") };
    let origin = security::origin_of(&url).filter(|o| o.starts_with("https://") || o.starts_with("http://localhost"));
    let (Some(origin), false) = (origin, password.is_empty() || password.len() > 512) else {
        sync::push_toast(state, "Falta una dirección https:// válida o la contraseña");
        return;
    };
    let Some(secret) = protect(password) else { return };
    {
        let mut st = state.borrow_mut();
        let p = &mut st.passwords;
        let now = storage::now_secs();
        let username: String = username.trim().chars().take(256).collect();
        match p.store.entries.iter_mut().find(|e| e.origin == origin && e.username == username) {
            Some(e) => {
                e.secret = secret;
                e.updated = now;
            }
            None => {
                p.store.next_id += 1;
                let id = p.store.next_id;
                p.store.entries.push(Entry { id, origin, username, secret, created: now, updated: now, used: 0 });
            }
        }
        p.save();
    }
    push_list(state);
    sync::push_active_tab(state);
    sync::push_toast(state, "Contraseña guardada");
}

pub fn delete(state: &Shared, id: u64) {
    {
        let mut st = state.borrow_mut();
        st.passwords.store.entries.retain(|e| e.id != id);
        st.passwords.save();
    }
    push_list(state);
    sync::push_active_tab(state);
}

/// Sacar un sitio de "nunca guardar".
pub fn forget_never(state: &Shared, origin: &str) {
    {
        let mut st = state.borrow_mut();
        st.passwords.store.never.retain(|o| o != origin);
        st.passwords.save();
    }
    push_list(state);
}

/// "Borrar datos de navegacion" > Contrasenas guardadas.
pub fn clear_all(state: &Shared) {
    {
        let mut st = state.borrow_mut();
        st.passwords.store.entries.clear();
        st.passwords.store.never.clear();
        st.passwords.save();
    }
    push_list(state);
    sync::push_active_tab(state);
}

/// Copia al portapapeles y lo borra a los CLIPBOARD_FOR (si para entonces no
/// se copio otra cosa).
#[cfg(windows)]
fn copy_to_clipboard(text: &str) -> bool {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardSequenceNumber, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let hwnd = crate::win::main_hwnd();
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return false;
        }
        EmptyClipboard();
        let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
        let ok = !mem.is_null() && {
            let dst = GlobalLock(mem) as *mut u16;
            if !dst.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
                GlobalUnlock(mem);
            }
            !dst.is_null() && !SetClipboardData(CF_UNICODETEXT, mem as _).is_null()
        };
        CloseClipboard();
        if ok {
            let seq = GetClipboardSequenceNumber();
            let owner = hwnd as isize;
            std::thread::spawn(move || {
                std::thread::sleep(CLIPBOARD_FOR);
                if GetClipboardSequenceNumber() == seq && OpenClipboard(owner as _) != 0 {
                    EmptyClipboard();
                    CloseClipboard();
                }
            });
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_are_encrypted_and_come_back() {
        let secret = protect("gatito-123 ñ").unwrap();
        assert!(!secret.contains("gatito"));
        assert_eq!(unprotect(&secret).as_deref(), Some("gatito-123 ñ"));
        assert_eq!(unprotect("no es base64 de dpapi"), None);
    }

    #[test]
    fn fill_script_escapes_the_values() {
        let s = fill_script("a'\"</script>", "p\"w");
        assert!(s.contains(r#"("a'\"</script>", "p\"w")"#));
    }
}
