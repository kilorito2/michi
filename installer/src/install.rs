//! Los pasos de instalar y desinstalar. Corren en un hilo aparte y avisan
//! su avance con `progress(fraccion 0..1, paso, detalle)`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::payload::Payload;
use crate::system::{self, Value};

pub const EXE: &str = "michi.exe";
pub const UNINSTALLER: &str = "desinstalar.exe";
/// Perfil de WebView2 del navegador (cookies, contrasenas, permisos...): lo
/// crea WebView2 junto al .exe. Se conserva al actualizar y al desinstalar,
/// salvo que el usuario pida borrar sus datos.
const PROFILE_DIR: &str = "michi.exe.WebView2";

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    pub dir: PathBuf,
    pub desktop: bool,
    pub start_menu: bool,
    pub default_browser: bool,
    /// Fondo y buscador elegidos (se aplican sobre los ajustes que ya haya).
    pub theme: Option<String>,
    pub engine: Option<String>,
}

/// Carpeta de datos del navegador (la misma que storage::data_dir).
pub fn data_dir() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("Michi"))
}

pub fn default_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("Programs").join("Michi")
}

/// La carpeta elegida, siempre terminando en "Michi": si alguien elige
/// "Documentos", los archivos no quedan sueltos entre los suyos (y al
/// desinstalar no hay nada de el en juego).
pub fn normalize_dir(dir: &Path) -> PathBuf {
    let named = dir.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("Michi"));
    if named {
        dir.to_path_buf()
    } else {
        dir.join("Michi")
    }
}

pub fn install(payload: &Payload, opt: &Options, progress: &dyn Fn(f64, &str, &str)) -> Result<(), String> {
    let dir = &opt.dir;
    progress(0.02, "Preparando la carpeta", &dir.display().to_string());
    std::fs::create_dir_all(dir).map_err(|e| {
        format!("No se pudo crear {} ({e}). Elige una carpeta en la que tengas permiso de escritura.", dir.display())
    })?;

    let total = payload.total_bytes().max(1) as f64;
    payload.extract(dir, |done, file| {
        progress(0.04 + 0.82 * done as f64 / total, "Copiando y verificando archivos", file);
    })?;
    let exe = dir.join(EXE);

    progress(0.88, "Creando el desinstalador", UNINSTALLER);
    payload.write_uninstaller(&dir.join(UNINSTALLER))?;

    progress(0.91, "Creando accesos directos", "");
    let description = "Navegar por Internet con Michi";
    for (wanted, lnk) in [(opt.start_menu, system::start_menu_lnk()), (opt.desktop, system::desktop_lnk())] {
        let Some(lnk) = lnk else { continue };
        if wanted {
            system::shortcut(&lnk, &exe, description)?;
        } else {
            let _ = std::fs::remove_file(&lnk);
        }
    }

    progress(0.93, "Quitando la version anterior", "Navegador");
    remove_legacy_install(dir);

    progress(0.94, "Registrando en Windows", "Aplicaciones instaladas");
    // El nombre visible del .exe viene de su informacion de version; Windows lo
    // cachea, asi que se olvida el viejo ("michi.exe") para que lea "Michi".
    system::forget_cached_name(&exe);
    register_uninstall(dir, &payload.index.version)?;
    if opt.default_browser {
        system::register_browser(&exe)?;
    } else {
        system::unregister_browser();
    }

    progress(0.97, "Guardando tus preferencias", "");
    apply_settings(opt);

    progress(1.0, "Listo", "");
    Ok(())
}

/// Antes de llamarse Michi, el navegador se llamaba "Navegador" y se instalaba
/// en su propia carpeta (...\Programs\Navegador, navegador.exe). Al instalar
/// Michi esa version se quita sola: registro, accesos directos y los archivos
/// del programa. El perfil (cookies, sesiones iniciadas) se muda a Michi si
/// Michi todavia no tiene uno; si ya tiene, el viejo queda donde estaba: nunca
/// se borran datos del usuario.
fn remove_legacy_install(michi_dir: &Path) {
    let default = dirs::data_local_dir().map(|d| d.join("Programs").join("Navegador"));
    let old_dir = system::reg_get(system::LEGACY_UNINSTALL_KEY, "InstallLocation")
        .map(PathBuf::from)
        .or(default);

    if let Some(old_dir) = old_dir.filter(|d| d.join("navegador.exe").is_file() && d != michi_dir) {
        let old_profile = old_dir.join("navegador.exe.WebView2");
        let new_profile = michi_dir.join(PROFILE_DIR);
        if old_profile.is_dir() && !new_profile.exists() {
            let _ = std::fs::rename(&old_profile, &new_profile);
        }
        // Solo los archivos que instalaba esa version, nunca la carpeta a ciegas.
        let _ = std::fs::remove_file(old_dir.join("navegador.exe"));
        let _ = std::fs::remove_file(old_dir.join("desinstalar.exe"));
        let _ = std::fs::remove_dir_all(old_dir.join("wallpapers"));
        // Se va solo si quedo vacia (si el perfil viejo quedo ahi, se queda).
        let _ = std::fs::remove_dir(&old_dir);
        system::forget_cached_name(&old_dir.join("navegador.exe"));
    }
    for lnk in system::legacy_shortcuts() {
        let _ = std::fs::remove_file(lnk);
    }
    system::unregister_legacy();
}

fn register_uninstall(dir: &Path, version: &str) -> Result<(), String> {
    let key = system::UNINSTALL_KEY;
    let exe = dir.join(EXE).display().to_string();
    let uninstall = format!("\"{}\" --uninstall", dir.join(UNINSTALLER).display());
    system::reg_set(key, Some("DisplayName"), Value::Str("Michi"))?;
    system::reg_set(key, Some("DisplayVersion"), Value::Str(version))?;
    system::reg_set(key, Some("Publisher"), Value::Str("Michi"))?;
    system::reg_set(key, Some("DisplayIcon"), Value::Str(&format!("{exe},0")))?;
    system::reg_set(key, Some("InstallLocation"), Value::Str(&dir.display().to_string()))?;
    system::reg_set(key, Some("UninstallString"), Value::Str(&uninstall))?;
    system::reg_set(key, Some("InstallDate"), Value::Str(&today()))?;
    system::reg_set(key, Some("NoModify"), Value::Dword(1))?;
    system::reg_set(key, Some("NoRepair"), Value::Dword(1))?;
    let kb = system::folder_kb(dir).min(u32::MAX as u64) as u32;
    system::reg_set(key, Some("EstimatedSize"), Value::Dword(kb))
}

/// Ajustes guardados del navegador (vacio si todavia no hay).
pub fn current_settings() -> serde_json::Map<String, serde_json::Value> {
    data_dir()
        .and_then(|dir| std::fs::read(dir.join("settings.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// Fondo y buscador elegidos en el instalador, sobre los ajustes que ya haya
/// (el resto queda como estaba). El instalador arranca con los actuales
/// marcados, asi que si el usuario no los toca no cambia nada.
fn apply_settings(opt: &Options) {
    let Some(dir) = data_dir() else { return };
    let file = dir.join("settings.json");
    let before = current_settings();
    let mut settings = before.clone();
    if let Some(theme) = &opt.theme {
        settings.insert("theme".into(), theme.clone().into());
    }
    if let Some(engine) = &opt.engine {
        settings.insert("searchEngine".into(), engine.clone().into());
    }
    if settings == before {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(file, serde_json::to_vec_pretty(&settings).unwrap_or_default());
}

pub fn uninstall(dir: &Path, delete_data: bool, progress: &dyn Fn(f64, &str, &str)) -> Result<(), String> {
    progress(0.1, "Quitando los accesos directos", "");
    for lnk in [system::start_menu_lnk(), system::desktop_lnk()].into_iter().flatten() {
        let _ = std::fs::remove_file(lnk);
    }

    progress(0.3, "Quitando el registro de Windows", "");
    system::unregister_browser();
    system::reg_delete(system::UNINSTALL_KEY);

    progress(0.5, "Borrando los archivos del programa", &dir.display().to_string());
    // Solo lo que puso el instalador, por nombre: nunca "toda la carpeta".
    let mut known: Vec<PathBuf> = vec![dir.join(EXE), dir.join(UNINSTALLER), dir.join("wallpapers")];
    if delete_data {
        known.push(dir.join(PROFILE_DIR));
    }
    for path in &known {
        let _ = if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) };
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().ends_with(".nuevo") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    // La carpeta solo si quedo vacia.
    let _ = std::fs::remove_dir(dir);
    if dir.join(EXE).exists() {
        return Err(format!(
            "No se pudo borrar {}. Cierra Michi y vuelve a intentarlo.",
            dir.join(EXE).display()
        ));
    }

    if delete_data {
        progress(0.8, "Borrando tus datos", "historial, marcadores, ajustes, extensiones");
        if let Some(data) = data_dir() {
            let _ = std::fs::remove_dir_all(data);
        }
    }
    progress(1.0, "Listo", "");
    Ok(())
}

/// Fecha de hoy como AAAAMMDD (la que muestra "Aplicaciones instaladas").
fn today() -> String {
    let days = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0) as i64;
    // Dias desde 1970-01-01 -> fecha civil (algoritmo de Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chosen_folder_always_ends_in_michi() {
        assert_eq!(normalize_dir(Path::new("C:\\Users\\ana\\Documents")), Path::new("C:\\Users\\ana\\Documents\\Michi"));
        assert_eq!(normalize_dir(Path::new("D:\\Apps\\michi")), Path::new("D:\\Apps\\michi"));
        assert_eq!(normalize_dir(Path::new("C:\\")), Path::new("C:\\Michi"));
    }

    #[test]
    fn install_date_format() {
        let d = today();
        assert_eq!(d.len(), 8);
        assert!(d.starts_with("20"));
    }
}
