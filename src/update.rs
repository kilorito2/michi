//! Actualizaciones: Michi se fija en GitHub si hay una version nueva y, si
//! el usuario quiere, la descarga y abre su instalador.
//!
//! - Al iniciar (unos segundos despues, para no demorar el arranque) y como
//!   mucho cada 12 horas, si esta encendido en Ajustes > Acerca de. Se le
//!   pregunta a la API de GitHub por la ultima version publicada
//!   (github.com/kilorito2/michi/releases); no se envia nada mas.
//! - Si hay una mas nueva, la barra muestra "Actualizar" y Ajustes > Acerca de
//!   ofrece "Descargar e instalar". Nada se descarga ni se instala solo.
//! - El instalador se descarga por HTTPS desde GitHub y se compara con el
//!   tamano y la huella SHA-256 que publica GitHub para ese archivo. Despues
//!   se abre, y Michi se cierra para que pueda reemplazarlo.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::state::{AppState, Shared, UserEvent};
use crate::storage;

const API: &str = "https://api.github.com/repos/kilorito2/michi/releases/latest";
const STATE_FILE: &str = "update.json";
/// Cada cuanto se vuelve a mirar, como mucho.
const CHECK_EVERY: u64 = 12 * 60 * 60;
/// Espera despues de arrancar antes de mirar.
const FIRST_DELAY: Duration = Duration::from_secs(8);
/// Tope de lo que se acepta descargar (el instalador pesa unas decenas de MB).
const MAX_SIZE: u64 = 1024 * 1024 * 1024;

/// Una version publicada en GitHub.
#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    /// La pagina de la version (notas).
    pub page: String,
    pub asset_url: String,
    pub asset_name: String,
    pub size: u64,
    /// Huella SHA-256 del instalador, en hexadecimal (si GitHub la da).
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release, f64),
    Installing,
    Failed(String),
}

#[derive(Default)]
pub struct Status {
    pub phase: Phase,
    /// Cuando hacer la busqueda automatica del arranque.
    check_at: Option<Instant>,
    /// La busqueda en curso la pidio el usuario (si falla, se le dice).
    manual: bool,
}

/// Novedades de los hilos que buscan y descargan.
#[derive(Debug, Clone)]
pub enum Event {
    Checked(Result<Option<Release>, String>),
    Progress(f64),
    Downloaded(PathBuf),
    DownloadFailed(String),
}

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    last_check: u64,
}

/// Al iniciar: programa la busqueda automatica (si toca).
pub fn check_on_start(state: &Shared) {
    let mut st = state.borrow_mut();
    if !st.settings.check_updates {
        return;
    }
    let saved: Saved = storage::load(STATE_FILE);
    if storage::now_secs().saturating_sub(saved.last_check) >= CHECK_EVERY {
        st.update.check_at = Some(Instant::now() + FIRST_DELAY);
    }
}

/// Desde el bucle de eventos: lanza la busqueda programada cuando llega su
/// hora. Devuelve cuando volver a mirar.
pub fn tick(state: &Shared) -> Option<Instant> {
    let at = state.borrow().update.check_at?;
    if Instant::now() < at {
        return Some(at);
    }
    state.borrow_mut().update.check_at = None;
    start_check(state, false);
    None
}

/// Busca una version nueva en otro hilo ("Buscar ahora" en Ajustes, o la
/// automatica del arranque).
pub fn start_check(state: &Shared, manual: bool) {
    let proxy = {
        let mut st = state.borrow_mut();
        if matches!(st.update.phase, Phase::Checking | Phase::Downloading(..) | Phase::Installing) {
            return;
        }
        st.update.phase = Phase::Checking;
        st.update.manual = manual;
        st.proxy.clone()
    };
    push(state);
    let Some(proxy) = proxy else { return };
    std::thread::spawn(move || {
        let result = fetch_latest().map(|r| r.filter(|r| is_newer(&r.version, env!("CARGO_PKG_VERSION"))));
        let _ = proxy.send_event(UserEvent::Update(Event::Checked(result)));
    });
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .https_only(true)
        .user_agent(concat!("Michi/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn fetch_latest() -> Result<Option<Release>, String> {
    let response = agent()
        .get(API)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(404, _) => "Todavía no hay versiones publicadas".to_string(),
            ureq::Error::Status(code, _) => format!("GitHub respondió con un error ({code})"),
            ureq::Error::Transport(_) => "No se pudo conectar con GitHub".to_string(),
        })?;
    let mut body = Vec::new();
    response
        .into_reader()
        .take(2 * 1024 * 1024)
        .read_to_end(&mut body)
        .map_err(|_| "No se pudo leer la respuesta de GitHub".to_string())?;
    let json: Value = serde_json::from_slice(&body).map_err(|_| "GitHub respondió algo inesperado".to_string())?;
    Ok(parse_release(&json))
}

/// La version publicada y su instalador (Michi-Setup-x.y.z.exe).
fn parse_release(json: &Value) -> Option<Release> {
    let tag = json.get("tag_name")?.as_str()?;
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    let asset = json.get("assets")?.as_array()?.iter().find(|a| {
        a.get("name")
            .and_then(Value::as_str)
            .is_some_and(|n| n.starts_with("Michi-Setup") && n.to_ascii_lowercase().ends_with(".exe"))
    })?;
    let asset_url = asset.get("browser_download_url")?.as_str()?.to_string();
    if !asset_url.starts_with("https://github.com/") {
        return None;
    }
    Some(Release {
        version,
        page: json.get("html_url").and_then(Value::as_str).unwrap_or_default().to_string(),
        asset_url,
        asset_name: asset.get("name")?.as_str()?.to_string(),
        size: asset.get("size").and_then(Value::as_u64).unwrap_or(0),
        sha256: asset
            .get("digest")
            .and_then(Value::as_str)
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(|h| h.to_ascii_lowercase()),
    })
}

/// "0.1.10" es mas nueva que "0.1.9" (numero por numero, no como texto).
fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> { v.split(['.', '-', '+']).map_while(|p| p.parse().ok()).collect() };
    let (a, b) = (parse(candidate), parse(current));
    !a.is_empty() && a > b
}

pub fn on_event(state: &Shared, event: Event) {
    match event {
        Event::Checked(result) => {
            let mut st = state.borrow_mut();
            let manual = st.update.manual;
            st.update.phase = match result {
                Ok(Some(release)) => Phase::Available(release),
                Ok(None) => Phase::UpToDate,
                Err(e) if manual => Phase::Failed(e),
                // Sin internet al arrancar no es algo para avisar.
                Err(_) => Phase::Idle,
            };
            if !matches!(st.update.phase, Phase::Idle) {
                storage::save(STATE_FILE, &Saved { last_check: storage::now_secs() });
            }
        }
        Event::Progress(p) => {
            let mut st = state.borrow_mut();
            if let Phase::Downloading(_, progress) = &mut st.update.phase {
                *progress = p;
            }
        }
        Event::DownloadFailed(e) => state.borrow_mut().update.phase = Phase::Failed(e),
        Event::Downloaded(path) => {
            state.borrow_mut().update.phase = Phase::Installing;
            push(state);
            if std::process::Command::new(&path).spawn().is_ok() {
                // El instalador reemplaza michi.exe: Michi tiene que estar cerrado.
                crate::security::quit(state);
            } else {
                state.borrow_mut().update.phase = Phase::Failed("No se pudo abrir el instalador".into());
            }
        }
    }
    push(state);
}

/// "Descargar e instalar": baja el instalador de la version nueva en otro
/// hilo, lo verifica y lo abre (ver on_event, Downloaded).
pub fn install(state: &Shared) {
    let (release, proxy) = {
        let mut st = state.borrow_mut();
        let Phase::Available(release) = st.update.phase.clone() else { return };
        st.update.phase = Phase::Downloading(release.clone(), 0.0);
        (release, st.proxy.clone())
    };
    push(state);
    let Some(proxy) = proxy else { return };
    std::thread::spawn(move || {
        let send = |e: Event| {
            let _ = proxy.send_event(UserEvent::Update(e));
        };
        match download(&release, &|p| send(Event::Progress(p))) {
            Ok(path) => send(Event::Downloaded(path)),
            Err(e) => send(Event::DownloadFailed(e)),
        }
    });
}

fn download(release: &Release, progress: &dyn Fn(f64)) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("Michi-Update");
    std::fs::create_dir_all(&dir).map_err(|_| "No se pudo preparar la carpeta temporal".to_string())?;
    // Solo el nombre del archivo (nunca una ruta que venga de afuera).
    let name = std::path::Path::new(&release.asset_name)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "Michi-Setup.exe".into());
    let path = dir.join(name);

    let response = agent().get(&release.asset_url).call().map_err(|_| "No se pudo descargar la versión nueva".to_string())?;
    let total = response.header("Content-Length").and_then(|v| v.parse::<u64>().ok()).unwrap_or(release.size);
    let mut reader = response.into_reader().take(MAX_SIZE);
    let mut file = std::fs::File::create(&path).map_err(|_| "No se pudo guardar el instalador".to_string())?;
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let (mut done, mut last_pct) = (0u64, 0u64);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|_| "Se corto la descarga".to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|_| "No se pudo guardar el instalador".to_string())?;
        hash.update(&buf[..n]);
        done += n as u64;
        if total > 0 && done * 100 / total > last_pct {
            last_pct = done * 100 / total;
            progress(done as f64 / total as f64);
        }
    }
    drop(file);

    let fail = |msg: &str| {
        let _ = std::fs::remove_file(&path);
        Err(msg.to_string())
    };
    if release.size > 0 && done != release.size {
        return fail("La descarga quedó incompleta");
    }
    let hex: String = hash.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect();
    if release.sha256.as_ref().is_some_and(|expected| *expected != hex) {
        return fail("El instalador descargado no coincide con el publicado: no se abrió");
    }
    Ok(path)
}

/// Lo que muestran Ajustes > Acerca de y la barra.
pub fn view(st: &AppState) -> Value {
    let (phase, release, progress, error) = match &st.update.phase {
        Phase::Idle => ("idle", None, 0.0, None),
        Phase::Checking => ("checking", None, 0.0, None),
        Phase::UpToDate => ("uptodate", None, 0.0, None),
        Phase::Available(r) => ("available", Some(r), 0.0, None),
        Phase::Downloading(r, p) => ("downloading", Some(r), *p, None),
        Phase::Installing => ("installing", None, 1.0, None),
        Phase::Failed(e) => ("failed", None, 0.0, Some(e.as_str())),
    };
    json!({
        "phase": phase,
        "version": release.map(|r| r.version.clone()),
        "page": release.map(|r| r.page.clone()),
        "progress": progress,
        "error": error,
    })
}

/// Avisa a la barra (el boton "Actualizar") y a las paginas propias.
pub fn push(state: &Shared) {
    {
        let st = state.borrow();
        if let Some(tb) = &st.topbar {
            let _ = tb.evaluate_script(&format!("window.onUpdate && window.onUpdate({})", view(&st)));
        }
    }
    crate::sync::push_own_pages(state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_number_by_number() {
        assert!(is_newer("0.1.7", "0.1.6"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.6", "0.1.6"));
        assert!(!is_newer("0.1.5", "0.1.6"));
        assert!(!is_newer("nada", "0.1.6"));
    }

    #[test]
    fn finds_the_installer_of_a_release() {
        let json = serde_json::json!({
            "tag_name": "v0.2.0",
            "html_url": "https://github.com/kilorito2/michi/releases/tag/v0.2.0",
            "assets": [
                { "name": "notas.txt", "browser_download_url": "https://github.com/x/notas.txt", "size": 1 },
                {
                    "name": "Michi-Setup-0.2.0.exe",
                    "browser_download_url": "https://github.com/kilorito2/michi/releases/download/v0.2.0/Michi-Setup-0.2.0.exe",
                    "size": 1234,
                    "digest": "sha256:ABCDEF"
                }
            ]
        });
        let r = parse_release(&json).unwrap();
        assert_eq!(r.version, "0.2.0");
        assert_eq!(r.asset_name, "Michi-Setup-0.2.0.exe");
        assert_eq!(r.size, 1234);
        assert_eq!(r.sha256.as_deref(), Some("abcdef"));

        // Un instalador que no se baja de github.com no se acepta.
        let mut evil = json.clone();
        evil["assets"][1]["browser_download_url"] = "https://malo.com/Michi-Setup.exe".into();
        assert!(parse_release(&evil).is_none());
    }

    /// Descarga de verdad el instalador de la ultima version publicada y lo
    /// verifica (usa la red: `cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn downloads_and_verifies_the_latest_installer() {
        let release = fetch_latest().unwrap().unwrap();
        assert!(release.sha256.is_some(), "GitHub no dio la huella SHA-256");
        let path = download(&release, &|_| {}).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), release.size);
        let _ = std::fs::remove_file(path);
    }
}
