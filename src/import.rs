//! Importar datos de otros navegadores instalados: marcadores, historial,
//! cookies (las sesiones iniciadas) y las pestanas abiertas.
//!
//! Dos familias:
//! - Firefox y derivados (LibreWolf, Waterfox, Zen, Floorp): todo. Guardan sus
//!   datos en bases SQLite y sus cookies en TEXTO PLANO (`cookies.sqlite`), asi
//!   que la sesion se puede copiar leyendo el archivo, sin descifrar nada. Las
//!   pestanas abiertas estan en un JSON comprimido con LZ4 (formato mozLz40).
//! - Chromium (Chrome, Edge, Brave, Vivaldi, Opera): marcadores e historial.
//!   Sus cookies van cifradas con la proteccion de datos de Windows (DPAPI) y
//!   no se importan.
//!
//! Para no chocar con el navegador de origen abierto (que bloquea sus bases),
//! cada archivo SQLite se copia a una carpeta temporal y se lee la copia.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::state::{Shared, UserEvent};
use crate::storage::{Bookmark, HistoryEntry};

/// Un perfil importable, ya detectado (ver `scan`). `key` identifica de donde
/// sacar los datos cuando el usuario confirma (ver `run`).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub key: String,
    /// Nombre visible del navegador ("Mozilla Firefox").
    pub browser: String,
    /// Nombre del perfil ("default-release", "Perfil 1").
    pub profile: String,
    /// Que tiene disponible este perfil (para marcar/desmarcar en la interfaz).
    pub bookmarks: bool,
    pub history: bool,
    pub cookies: bool,
    pub tabs: bool,
}

/// Lo que se logro leer de un perfil, listo para volcar en Michi (ver `apply`).
/// Viaja por UserEvent al hilo principal: las cookies se agregan ahi, porque la
/// API de cookies de WebView2 es COM y vive en el hilo de la interfaz.
#[derive(Clone, Debug, Default)]
pub struct ImportData {
    pub source: String,
    pub bookmarks: Vec<Bookmark>,
    pub history: Vec<HistoryEntry>,
    pub cookies: Vec<Cookie>,
    pub tabs: Vec<String>,
    /// El navegador de origen cifra sus cookies (Chromium): no se importaron.
    pub cookies_encrypted: bool,
}

#[derive(Clone, Debug)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    /// Segundos desde 1970; None = cookie de sesion (se va al cerrar).
    pub expires: Option<f64>,
    pub secure: bool,
    pub http_only: bool,
    /// 0/1 = Lax, 2 = Strict (ver native::add_cookies).
    pub same_site: i32,
}

// --- Catalogo de navegadores conocidos ---

enum Family {
    Firefox,
    Chromium,
}

struct Known {
    label: &'static str,
    family: Family,
    /// Carpeta base, relativa a %APPDATA% (Firefox) o %LOCALAPPDATA% (Chromium).
    /// Opera es la excepcion: va en %APPDATA% aunque sea Chromium.
    rel: &'static str,
    roaming: bool,
}

const KNOWN: &[Known] = &[
    Known { label: "Mozilla Firefox", family: Family::Firefox, rel: "Mozilla/Firefox", roaming: true },
    Known { label: "LibreWolf", family: Family::Firefox, rel: "librewolf", roaming: true },
    Known { label: "Waterfox", family: Family::Firefox, rel: "Waterfox", roaming: true },
    Known { label: "Zen", family: Family::Firefox, rel: "zen", roaming: true },
    Known { label: "Floorp", family: Family::Firefox, rel: "Floorp", roaming: true },
    Known { label: "Google Chrome", family: Family::Chromium, rel: "Google/Chrome/User Data", roaming: false },
    Known { label: "Microsoft Edge", family: Family::Chromium, rel: "Microsoft/Edge/User Data", roaming: false },
    Known { label: "Brave", family: Family::Chromium, rel: "BraveSoftware/Brave-Browser/User Data", roaming: false },
    Known { label: "Vivaldi", family: Family::Chromium, rel: "Vivaldi/User Data", roaming: false },
    Known { label: "Opera", family: Family::Chromium, rel: "Opera Software/Opera Stable", roaming: true },
    Known { label: "Opera GX", family: Family::Chromium, rel: "Opera Software/Opera GX Stable", roaming: true },
];

fn base_dir(roaming: bool) -> Option<PathBuf> {
    if roaming {
        dirs::config_dir() // %APPDATA%
    } else {
        dirs::data_local_dir() // %LOCALAPPDATA%
    }
}

/// Recorre los navegadores conocidos y arma la lista de perfiles con datos
/// para importar. No lee los datos todavia, solo mira que archivos existen.
pub fn scan() -> Vec<Source> {
    let mut out = Vec::new();
    for k in KNOWN {
        let Some(root) = base_dir(k.roaming).map(|b| b.join(k.rel)) else { continue };
        if !root.is_dir() {
            continue;
        }
        match k.family {
            Family::Firefox => scan_firefox(k, &root, &mut out),
            Family::Chromium => scan_chromium(k, &root, &mut out),
        }
    }
    out
}

fn scan_firefox(k: &Known, root: &Path, out: &mut Vec<Source>) {
    for (name, dir) in firefox_profiles(root) {
        let places = dir.join("places.sqlite");
        let has_places = places.is_file();
        let source = Source {
            key: format!("firefox|{}", dir.display()),
            browser: k.label.to_string(),
            profile: name,
            bookmarks: has_places,
            history: has_places,
            cookies: dir.join("cookies.sqlite").is_file(),
            tabs: firefox_session_file(&dir).is_some(),
        };
        if source.bookmarks || source.cookies || source.tabs {
            out.push(source);
        }
    }
}

/// Lee profiles.ini y devuelve (nombre, carpeta absoluta) de cada perfil.
fn firefox_profiles(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(ini) = std::fs::read_to_string(root.join("profiles.ini")) else { return Vec::new() };
    let mut profiles = Vec::new();
    let (mut name, mut path, mut relative, mut in_profile) = (String::new(), String::new(), true, false);
    let flush = |name: &str, path: &str, relative: bool, out: &mut Vec<(String, PathBuf)>| {
        if path.is_empty() {
            return;
        }
        let dir = if relative { root.join(path) } else { PathBuf::from(path) };
        if dir.is_dir() {
            let label = if name.is_empty() { "Perfil".to_string() } else { name.to_string() };
            out.push((label, dir));
        }
    };
    for line in ini.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            if in_profile {
                flush(&name, &path, relative, &mut profiles);
            }
            in_profile = line.starts_with("[Profile");
            name.clear();
            path.clear();
            relative = true;
            continue;
        }
        if !in_profile {
            continue;
        }
        if let Some(v) = line.strip_prefix("Name=") {
            name = v.to_string();
        } else if let Some(v) = line.strip_prefix("Path=") {
            path = v.replace('/', "\\");
        } else if let Some(v) = line.strip_prefix("IsRelative=") {
            relative = v.trim() != "0";
        }
    }
    if in_profile {
        flush(&name, &path, relative, &mut profiles);
    }
    profiles
}

fn firefox_session_file(dir: &Path) -> Option<PathBuf> {
    let candidates = [
        dir.join("sessionstore-backups").join("recovery.jsonlz4"),
        dir.join("sessionstore-backups").join("previous.jsonlz4"),
        dir.join("sessionstore.jsonlz4"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

fn scan_chromium(k: &Known, root: &Path, out: &mut Vec<Source>) {
    for (name, dir) in chromium_profiles(root) {
        let has_history = dir.join("History").is_file();
        let has_bookmarks = dir.join("Bookmarks").is_file();
        if !has_history && !has_bookmarks {
            continue;
        }
        out.push(Source {
            key: format!("chromium|{}", dir.display()),
            browser: k.label.to_string(),
            profile: name,
            bookmarks: has_bookmarks,
            history: has_history,
            // Cifradas con DPAPI: no se importan (ver el comentario del modulo).
            cookies: false,
            tabs: false,
        });
    }
}

/// Carpetas de perfil de un navegador Chromium. Toma los nombres visibles de
/// "Local State" si estan; si no, usa las carpetas que encuentra.
fn chromium_profiles(root: &Path) -> Vec<(String, PathBuf)> {
    let names = chromium_profile_names(root);
    let mut out = Vec::new();
    let mut add = |dir: PathBuf| {
        if !dir.is_dir() || out.iter().any(|(_, d): &(String, PathBuf)| *d == dir) {
            return;
        }
        let key = dir.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let label = names.get(&key).cloned().unwrap_or(key);
        out.push((label, dir));
    };
    // Perfiles nombrados en Local State (Default, Profile 1, ...).
    for key in names.keys() {
        add(root.join(key));
    }
    // Por si Local State no estaba: carpetas habituales y el propio root (Opera).
    add(root.join("Default"));
    for n in 1..=9 {
        add(root.join(format!("Profile {n}")));
    }
    add(root.to_path_buf());
    out
}

fn chromium_profile_names(root: &Path) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let Ok(bytes) = std::fs::read(root.join("Local State")) else { return map };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return map };
    if let Some(cache) = json.get("profile").and_then(|p| p.get("info_cache")).and_then(|c| c.as_object()) {
        for (dir, info) in cache {
            if let Some(name) = info.get("name").and_then(|n| n.as_str()) {
                map.insert(dir.clone(), name.to_string());
            }
        }
    }
    map
}

// --- Lectura de los datos (en un hilo aparte) ---

/// Que datos quiere el usuario de este perfil.
#[derive(Clone, Copy)]
pub struct Wants {
    pub bookmarks: bool,
    pub history: bool,
    pub cookies: bool,
    pub tabs: bool,
}

/// Lanza la importacion en segundo plano y avisa por UserEvent al terminar.
/// Leer varias bases SQLite y copiarlas no debe bloquear el bucle de eventos
/// (ni, con el, los callbacks de WebView2).
pub fn run(state: &Shared, key: String, wants: Wants) {
    let (proxy, label) = {
        let st = state.borrow();
        let label = st.import_sources.iter().find(|s| s.key == key).map(|s| s.browser.clone());
        (st.proxy.clone(), label)
    };
    let Some(proxy) = proxy else { return };
    let label = label.unwrap_or_else(|| "otro navegador".to_string());

    std::thread::spawn(move || {
        let data = read(&key, &label, wants);
        let event = match data {
            Some(d) => UserEvent::ImportReady(Box::new(d)),
            None => UserEvent::ImportFailed(format!("No se pudo leer los datos de {label}")),
        };
        let _ = proxy.send_event(event);
    });
}

fn read(key: &str, label: &str, wants: Wants) -> Option<ImportData> {
    let (family, dir) = key.split_once('|')?;
    let dir = PathBuf::from(dir);
    let mut data = ImportData { source: label.to_string(), ..Default::default() };

    match family {
        "firefox" => {
            if wants.bookmarks {
                data.bookmarks = with_copy(&dir.join("places.sqlite"), firefox_bookmarks).unwrap_or_default();
            }
            if wants.history {
                data.history = with_copy(&dir.join("places.sqlite"), firefox_history).unwrap_or_default();
            }
            if wants.cookies {
                data.cookies = with_copy(&dir.join("cookies.sqlite"), firefox_cookies).unwrap_or_default();
            }
            if wants.tabs {
                data.tabs = firefox_session_file(&dir).and_then(|p| firefox_tabs(&p)).unwrap_or_default();
            }
        }
        "chromium" => {
            if wants.bookmarks {
                data.bookmarks = chromium_bookmarks(&dir.join("Bookmarks")).unwrap_or_default();
            }
            if wants.history {
                data.history = with_copy(&dir.join("History"), chromium_history).unwrap_or_default();
            }
            if wants.cookies {
                data.cookies_encrypted = true;
            }
        }
        _ => return None,
    }
    Some(data)
}

/// Copia una base SQLite (y sus `-wal`/`-shm`) a una carpeta temporal y abre la
/// copia: el navegador de origen puede estar abierto y tener bloqueado el
/// original. La copia se borra al final. Devuelve None si no existe o falla.
fn with_copy<T>(src: &Path, f: impl FnOnce(&rusqlite::Connection) -> Option<T>) -> Option<T> {
    if !src.is_file() {
        return None;
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    let tmp = std::env::temp_dir().join(format!("michi-import-{}-{}", std::process::id(), stamp));
    std::fs::create_dir_all(&tmp).ok()?;
    let name = src.file_name()?;
    let dst = tmp.join(name);
    let _ = std::fs::copy(src, &dst);
    for ext in ["-wal", "-shm"] {
        let extra = append_ext(src, ext);
        if extra.is_file() {
            let _ = std::fs::copy(&extra, append_ext(&dst, ext));
        }
    }
    let result = rusqlite::Connection::open(&dst).ok().and_then(|conn| f(&conn));
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// `foo.sqlite` + `-wal` => `foo.sqlite-wal` (lo que SQLite espera, no `.wal`).
fn append_ext(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

fn query_pairs(conn: &rusqlite::Connection, sql: &str) -> Option<Vec<(String, String)>> {
    let mut stmt = conn.prepare(sql).ok()?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0).unwrap_or_default(), r.get::<_, String>(1)?)))
        .ok()?;
    Some(rows.flatten().collect())
}

fn firefox_bookmarks(conn: &rusqlite::Connection) -> Option<Vec<Bookmark>> {
    let pairs = query_pairs(
        conn,
        "SELECT COALESCE(b.title, p.title, ''), p.url \
         FROM moz_bookmarks b JOIN moz_places p ON b.fk = p.id \
         WHERE b.type = 1 AND p.url LIKE 'http%' ORDER BY b.dateAdded",
    )?;
    Some(pairs.into_iter().map(|(title, url)| Bookmark { title, url }).collect())
}

fn firefox_history(conn: &rusqlite::Connection) -> Option<Vec<HistoryEntry>> {
    // Mas antiguas primero, para que al concatenarlas las recientes queden al
    // final (como el historial propio, que crece agregando al final).
    let pairs = query_pairs(
        conn,
        "SELECT COALESCE(title, ''), url FROM moz_places \
         WHERE url LIKE 'http%' AND last_visit_date IS NOT NULL \
         ORDER BY last_visit_date DESC LIMIT 2000",
    )?;
    Some(pairs.into_iter().rev().map(|(title, url)| HistoryEntry { title, url }).collect())
}

fn firefox_cookies(conn: &rusqlite::Connection) -> Option<Vec<Cookie>> {
    let mut stmt = conn
        .prepare(
            "SELECT name, value, host, path, expiry, isSecure, isHttpOnly, sameSite FROM moz_cookies",
        )
        .ok()?;
    let rows = stmt
        .query_map([], |r| {
            let expiry: i64 = r.get(4).unwrap_or(0);
            Ok(Cookie {
                name: r.get(0)?,
                value: r.get(1).unwrap_or_default(),
                domain: r.get(2)?,
                path: r.get::<_, String>(3).unwrap_or_else(|_| "/".into()),
                expires: (expiry > 0).then_some(expiry as f64),
                secure: r.get::<_, i64>(5).unwrap_or(0) != 0,
                http_only: r.get::<_, i64>(6).unwrap_or(0) != 0,
                same_site: r.get::<_, i64>(7).unwrap_or(1) as i32,
            })
        })
        .ok()?;
    Some(rows.flatten().filter(|c| !c.name.is_empty() && !c.domain.is_empty()).collect())
}

fn chromium_history(conn: &rusqlite::Connection) -> Option<Vec<HistoryEntry>> {
    let pairs = query_pairs(
        conn,
        "SELECT COALESCE(title, ''), url FROM urls \
         WHERE url LIKE 'http%' ORDER BY last_visit_time DESC LIMIT 2000",
    )?;
    Some(pairs.into_iter().rev().map(|(title, url)| HistoryEntry { title, url }).collect())
}

/// Marcadores de Chromium: un JSON con un arbol de carpetas.
fn chromium_bookmarks(path: &Path) -> Option<Vec<Bookmark>> {
    let bytes = std::fs::read(path).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let mut out = Vec::new();
    if let Some(roots) = json.get("roots").and_then(|r| r.as_object()) {
        for node in roots.values() {
            collect_chromium_bookmarks(node, &mut out);
        }
    }
    Some(out)
}

fn collect_chromium_bookmarks(node: &serde_json::Value, out: &mut Vec<Bookmark>) {
    match node.get("type").and_then(|t| t.as_str()) {
        Some("url") => {
            if let Some(url) = node.get("url").and_then(|u| u.as_str()) {
                if url.starts_with("http") {
                    let title = node.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                    out.push(Bookmark { title, url: url.to_string() });
                }
            }
        }
        Some("folder") => {
            if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
                for child in children {
                    collect_chromium_bookmarks(child, out);
                }
            }
        }
        _ => {}
    }
}

/// Pestanas abiertas de Firefox, desde su sessionstore (JSON comprimido mozLz40).
fn firefox_tabs(path: &Path) -> Option<Vec<String>> {
    let json: serde_json::Value = serde_json::from_slice(&read_mozlz4(path)?).ok()?;
    let mut urls = Vec::new();
    for window in json.get("windows").and_then(|w| w.as_array())?.iter() {
        let Some(tabs) = window.get("tabs").and_then(|t| t.as_array()) else { continue };
        for tab in tabs {
            let entries = tab.get("entries").and_then(|e| e.as_array());
            // "index" es 1-based y apunta a la entrada visible del historial de
            // esa pestana; si falta, usamos la ultima.
            let idx = tab.get("index").and_then(|i| i.as_u64()).map(|i| i.saturating_sub(1) as usize);
            if let Some(entries) = entries {
                let entry = idx.and_then(|i| entries.get(i)).or_else(|| entries.last());
                if let Some(url) = entry.and_then(|e| e.get("url")).and_then(|u| u.as_str()) {
                    if url.starts_with("http") {
                        urls.push(url.to_string());
                    }
                }
            }
        }
    }
    Some(urls)
}

/// Descomprime el formato mozLz40 de Firefox: 8 bytes de firma "mozLz40\0", 4
/// bytes con el tamano original (LE) y a continuacion un bloque LZ4.
fn read_mozlz4(path: &Path) -> Option<Vec<u8>> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 12 || &data[0..8] != b"mozLz40\0" {
        return None;
    }
    let size = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
    lz4_flex::decompress(&data[12..], size).ok()
}

// --- Volcar lo leido en Michi (hilo principal) ---

/// Mezcla los datos importados con los de Michi: agrega lo que falta (sin
/// duplicar por URL), inyecta las cookies en el perfil de WebView2 y abre las
/// pestanas. Avisa el resultado con un toast.
pub fn apply(state: &Shared, data: ImportData) {
    use std::collections::HashSet;

    let (added_bm, added_hist) = {
        let mut st = state.borrow_mut();

        let have: HashSet<String> = st.bookmarks.iter().map(|b| b.url.clone()).collect();
        let before = st.bookmarks.len();
        for b in &data.bookmarks {
            if !have.contains(&b.url) {
                st.bookmarks.push(b.clone());
            }
        }
        let added_bm = st.bookmarks.len() - before;
        if added_bm > 0 {
            st.save_bookmarks();
        }

        let have: HashSet<String> = st.history.iter().map(|h| h.url.clone()).collect();
        let before = st.history.len();
        for h in &data.history {
            if !have.contains(&h.url) {
                st.history.push(h.clone());
            }
        }
        let excess = st.history.len().saturating_sub(crate::storage::HISTORY_MAX);
        st.history.drain(..excess);
        let added_hist = st.history.len().saturating_sub(before.saturating_sub(excess));
        if !data.history.is_empty() {
            st.save_history();
        }

        (added_bm, added_hist)
    };

    // Cookies: via el perfil compartido de WebView2 (cualquier WebView sirve).
    let added_cookies = {
        let st = state.borrow();
        match (&st.topbar, data.cookies.is_empty()) {
            (Some(tb), false) => crate::native::add_cookies(tb, &data.cookies),
            _ => 0,
        }
    };

    // Pestanas: con tope, para no abrir decenas de golpe.
    let mut opened = 0;
    for url in data.tabs.iter().take(30) {
        if crate::security::is_safe_to_open(url) {
            state.borrow_mut().request_tab(url);
            opened += 1;
        }
    }

    crate::sync::push_bookmarks(state);
    crate::sync::push_history(state);

    let mut parts = Vec::new();
    if added_bm > 0 {
        parts.push(plural(added_bm, "marcador", "marcadores"));
    }
    if added_hist > 0 {
        parts.push(format!("{added_hist} del historial"));
    }
    if added_cookies > 0 {
        parts.push(plural(added_cookies, "cookie", "cookies"));
    }
    if opened > 0 {
        parts.push(plural(opened, "pestana", "pestanas"));
    }

    let msg = if parts.is_empty() {
        if data.cookies_encrypted {
            format!("De {} se importaron los marcadores y el historial. Las sesiones de este navegador van cifradas y no se pueden importar.", data.source)
        } else {
            format!("No habia nada nuevo para importar de {}", data.source)
        }
    } else {
        let mut m = format!("Importado de {}: {}", data.source, join_es(&parts));
        if data.cookies_encrypted {
            m.push_str(". Las sesiones de este navegador van cifradas y no se importan");
        }
        m
    };
    crate::sync::push_toast(state, &msg);
}

fn plural(n: usize, singular: &str, plural: &str) -> String {
    format!("{n} {}", if n == 1 { singular } else { plural })
}

/// ["a", "b", "c"] -> "a, b y c".
fn join_es(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} y {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_in_spanish() {
        assert_eq!(join_es(&["a".into()]), "a");
        assert_eq!(join_es(&["a".into(), "b".into()]), "a y b");
        assert_eq!(join_es(&["a".into(), "b".into(), "c".into()]), "a, b y c");
    }

    #[test]
    fn mozlz4_rejects_foreign_data() {
        let tmp = std::env::temp_dir().join(format!("michi-mozlz4-{}", std::process::id()));
        std::fs::write(&tmp, b"not a mozLz40 file").unwrap();
        assert!(read_mozlz4(&tmp).is_none());
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn chromium_bookmarks_walk_tree() {
        let json = serde_json::json!({
            "roots": {
                "bookmark_bar": {
                    "type": "folder",
                    "children": [
                        { "type": "url", "name": "Rust", "url": "https://rust-lang.org" },
                        { "type": "folder", "children": [
                            { "type": "url", "name": "Local", "url": "file:///x" },
                            { "type": "url", "name": "Docs", "url": "https://doc.rust-lang.org" }
                        ]}
                    ]
                }
            }
        });
        let tmp = std::env::temp_dir().join(format!("michi-bm-{}.json", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec(&json).unwrap()).unwrap();
        let bm = chromium_bookmarks(&tmp).unwrap();
        let _ = std::fs::remove_file(&tmp);
        // Se queda solo con http(s), y recorre las subcarpetas.
        assert_eq!(bm.len(), 2);
        assert!(bm.iter().any(|b| b.url == "https://rust-lang.org"));
        assert!(bm.iter().any(|b| b.url == "https://doc.rust-lang.org"));
        assert!(bm.iter().all(|b| !b.url.starts_with("file")));
    }
}
