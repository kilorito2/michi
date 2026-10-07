//! Todo lo que sobrevive entre ejecuciones: ajustes, marcadores, historial y
//! la sesion (pestanas abiertas). Cada cosa es un JSON propio dentro de
//! %APPDATA%\Michi (ver data_dir). Si un archivo falta o esta roto se
//! usan los valores por defecto: nunca impide arrancar.
//!
//! Se escribe a un .tmp y despues se renombra, para que un corte a mitad de
//! la escritura no deje el archivo real a medias.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::state::DEFAULT_THEME;

/// %APPDATA%\Michi, salvo que MICHI_DATA_DIR diga otra carpeta (para
/// probar una compilacion sin tocar los datos reales).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MICHI_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("Michi")
}

/// Antes el proyecto se llamaba "Navegador": la primera vez, lo que habia en
/// %APPDATA%\Navegador se muda a la carpeta nueva (ajustes, marcadores,
/// historial, sesion). La carpeta "extensions" queda donde esta: WebView2 y
/// extensions.json conocen cada extension por esa ruta, y moverla las
/// desinstalaria. Se llama antes de leer cualquier archivo.
pub fn migrate_old_data() {
    if std::env::var_os("MICHI_DATA_DIR").is_some() {
        return;
    }
    let Some(base) = dirs::config_dir() else { return };
    let (old, new) = (base.join("Navegador"), base.join("Michi"));
    if new.exists() || !old.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(&old) else { return };
    if std::fs::create_dir_all(&new).is_err() {
        return;
    }
    for entry in entries.flatten() {
        if entry.file_name() != "extensions" {
            let _ = std::fs::rename(entry.path(), new.join(entry.file_name()));
        }
    }
    let _ = std::fs::remove_dir(&old); // solo si quedo vacia
}

/// El perfil de WebView2 (cookies, sesiones iniciadas, permisos) vive junto
/// al .exe, en "<nombre del exe>.WebView2": con el nombre nuevo del .exe se
/// usaria uno vacio. La primera vez se renombra el de "navegador.exe".
pub fn migrate_old_profile() {
    let Ok(exe) = std::env::current_exe() else { return };
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name()) else { return };
    let new = dir.join(format!("{}.WebView2", name.to_string_lossy()));
    let old = dir.join("navegador.exe.WebView2");
    if !new.exists() && old.is_dir() {
        let _ = std::fs::rename(old, new);
    }
}

pub fn load<T: DeserializeOwned + Default>(name: &str) -> T {
    std::fs::read(data_dir().join(name))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save<T: Serialize>(name: &str, value: &T) {
    let dir = data_dir();
    let result = std::fs::create_dir_all(&dir)
        .and_then(|_| serde_json::to_vec_pretty(value).map_err(std::io::Error::other))
        .and_then(|bytes| write_atomic(&dir.join(name), &bytes));
    if let Err(err) = result {
        eprintln!("[storage] no se pudo guardar {name}: {err}");
    }
}

pub fn remove(name: &str) {
    let _ = std::fs::remove_file(data_dir().join(name));
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

pub const SETTINGS_FILE: &str = "settings.json";
pub const BOOKMARKS_FILE: &str = "bookmarks.json";
pub const HISTORY_FILE: &str = "history.json";
pub const SESSION_FILE: &str = "session.json";
/// Logo (favicon) de cada sitio visitado, para las sugerencias de la barra
/// (ver suggest.rs). Dice que sitios se visitaron: se borra con el historial.
pub const FAVICONS_FILE: &str = "favicons.json";

/// Tope del historial guardado (las entradas mas viejas se descartan).
pub const HISTORY_MAX: usize = 1000;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Id de SEARCH_ENGINES.
    pub search_engine: String,
    /// "newtab" (arrancar con una pestana nueva) | "restore" (reabrir las de la ultima vez).
    pub startup: String,
    /// Fondo animado (id de protocol::THEMES).
    pub theme: String,
    /// Zoom con el que abre cada pestana (1.0 = 100%).
    pub default_zoom: f64,
    /// Carpeta de descargas; None = la de Windows (Descargas).
    pub downloads_dir: Option<PathBuf>,
    /// Bloquear las ventanas emergentes que un sitio abre por su cuenta (sin
    /// un clic del usuario). Los enlaces target=_blank y lo que se abre con
    /// un clic siguen abriendo pestana (ver security::on_new_window).
    pub block_popups: bool,
    /// Microsoft Defender SmartScreen: avisa antes de abrir sitios de
    /// phishing o malware y revisa la reputacion de las descargas.
    pub smartscreen: bool,
    /// Conexiones seguras (ver security::https_first): "upgrade" (intentar
    /// primero por HTTPS y volver a HTTP si el sitio no lo tiene), "strict"
    /// (igual, pero avisar antes de abrir un sitio por HTTP) u "off".
    pub https_mode: String,
    /// Prevencion de seguimiento de WebView2: "off" | "basic" | "balanced" | "strict".
    pub tracking_prevention: String,
    /// Borrar historial, cookies, cache y descargas al cerrar el navegador
    /// (y no guardar la sesion).
    pub clear_on_exit: bool,
    /// Tipos de permiso (ids de permissions::KINDS) bloqueados para todos los
    /// sitios: no llegan ni a preguntar.
    pub blocked_permissions: Vec<String>,
    /// Sugerencias del buscador mientras se escribe en la barra (ver
    /// suggest.rs). Lo escrito se le envia al buscador elegido, como en
    /// Firefox o Chrome; el historial y los marcadores se sugieren igual.
    pub search_suggestions: bool,
    /// Modo de rendimiento (ver perf.rs): "normal" | "optimized" | "super".
    pub performance: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            search_engine: "google".into(),
            startup: "newtab".into(),
            theme: DEFAULT_THEME.into(),
            default_zoom: 1.0,
            downloads_dir: None,
            block_popups: true,
            smartscreen: true,
            https_mode: "upgrade".into(),
            tracking_prevention: "balanced".into(),
            clear_on_exit: false,
            blocked_permissions: Vec::new(),
            search_suggestions: true,
            performance: "normal".into(),
        }
    }
}

impl Settings {
    pub fn downloads_dir(&self) -> PathBuf {
        self.downloads_dir
            .clone()
            .filter(|p| p.is_dir())
            .or_else(dirs::download_dir)
            .unwrap_or_else(std::env::temp_dir)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Bookmark {
    pub title: String,
    pub url: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HistoryEntry {
    pub title: String,
    pub url: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Session {
    pub urls: Vec<String>,
    pub active: usize,
}

/// (id, nombre visible, URL de busqueda con {q} donde va la consulta)
pub const SEARCH_ENGINES: &[(&str, &str, &str)] = &[
    ("google", "Google", "https://www.google.com/search?q={q}"),
    ("bing", "Bing", "https://www.bing.com/search?q={q}"),
    ("duckduckgo", "DuckDuckGo", "https://duckduckgo.com/?q={q}"),
    ("brave", "Brave Search", "https://search.brave.com/search?q={q}"),
    ("ecosia", "Ecosia", "https://www.ecosia.org/search?q={q}"),
];

pub fn engine(id: &str) -> (&'static str, &'static str, &'static str) {
    SEARCH_ENGINES
        .iter()
        .copied()
        .find(|(eid, _, _)| *eid == id)
        .unwrap_or(SEARCH_ENGINES[0])
}

pub fn search_url(engine_id: &str, query: &str) -> String {
    engine(engine_id).2.replace("{q}", &percent_encode(query))
}

/// Lo que el usuario escribio en la barra de direcciones -> URL a abrir:
/// una URL con esquema tal cual, algo con forma de dominio/IP/localhost con
/// https:// delante, y cualquier otra cosa como busqueda.
pub fn resolve_input(input: &str, engine_id: &str) -> Option<String> {
    let v = input.trim();
    if v.is_empty() {
        return None;
    }
    if has_scheme(v) {
        return Some(v.to_string());
    }
    if !v.contains(' ') && looks_like_host(v) {
        return Some(format!("https://{v}"));
    }
    Some(search_url(engine_id, v))
}

fn has_scheme(v: &str) -> bool {
    // "about:blank", "chrome-extension://...", "https://..."; pero no "localhost:3000".
    let Some((scheme, rest)) = v.split_once(':') else { return false };
    let valid = scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c));
    valid && (rest.starts_with("//") || scheme.eq_ignore_ascii_case("about"))
}

fn looks_like_host(v: &str) -> bool {
    let host_port = v.split(['/', '?', '#']).next().unwrap_or("");
    let host = match host_port.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => h,
        Some(_) => return false,
        None => host_port,
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() >= 2
        && labels
            .iter()
            .all(|l| !l.is_empty() && l.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'))
        // El ultimo tramo de un dominio real nunca es solo digitos, salvo en una IP.
        && (labels.last().is_some_and(|tld| tld.chars().any(|c| c.is_alphabetic()))
            || (labels.len() == 4 && labels.iter().all(|l| l.parse::<u8>().is_ok())))
}

pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_addresses_and_searches() {
        let r = |v| resolve_input(v, "google").unwrap();
        assert_eq!(r("github.com"), "https://github.com");
        assert_eq!(r("localhost:3000/x"), "https://localhost:3000/x");
        assert_eq!(r("192.168.0.1"), "https://192.168.0.1");
        assert_eq!(r("http://example.com"), "http://example.com");
        assert_eq!(r("about:blank"), "about:blank");
        assert_eq!(r("rust lang"), "https://www.google.com/search?q=rust+lang");
        assert_eq!(r("3.14"), "https://www.google.com/search?q=3.14");
        assert_eq!(r("c++"), "https://www.google.com/search?q=c%2B%2B");
        assert_eq!(resolve_input("  ", "google"), None);
        assert_eq!(
            resolve_input("hola mundo", "duckduckgo").unwrap(),
            "https://duckduckgo.com/?q=hola+mundo"
        );
    }
}
