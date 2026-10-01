//! Extensiones de navegador (las mismas de Chrome/Edge: WebView2 es Chromium).
//!
//! WebView2 no tiene tienda ni barra de extensiones propia: solo sabe
//! registrar una carpeta DESEMPAQUETADA en el perfil (AddBrowserExtension),
//! listarlas, activarlas/desactivarlas y quitarlas. Una vez registrada, la
//! extension queda instalada en el perfil entre ejecuciones (lo recuerda
//! WebView2), mientras su carpeta siga existiendo. Todo lo demas lo ponemos
//! nosotros:
//!
//! - Instalar desde Chrome Web Store / Edge Add-ons: con una pagina de
//!   extension abierta, la barra ofrece "Agregar" (ver store_id); se baja el
//!   .crx del mismo servicio que usan Chrome/Edge para actualizar, se
//!   desempaqueta en %APPDATA%\Michi\extensions\<id> y se registra.
//! - Instalar desde un .crx/.zip del disco, o registrar una carpeta.
//! - Para mostrar version, descripcion, icono, popup y opciones hace falta
//!   el manifest, y WebView2 solo nos da id y nombre: por eso guardamos un
//!   registro id -> carpeta (extensions.json).
//!
//! Descargar y descomprimir se hace en otro hilo; el resultado vuelve al
//! bucle de eventos como UserEvent::ExtensionUnpacked/ExtensionFailed, y
//! recien ahi (hilo principal, que es el unico que puede hablar con
//! WebView2) se registra con install_dir.
//!
//! Seguridad, antes de registrar nada:
//! - Los .crx se verifican con su firma (ver crx.rs), y uno de la tienda
//!   tiene que ser exactamente la extension pedida: aunque se lo cambien en
//!   el camino (ver download), no se instala. Un .zip o una carpeta no
//!   tienen firma: se avisa.
//! - Se descomprime con topes (ver unpack) en una carpeta temporal, y recien
//!   se pone en su lugar si el usuario confirma (ver Staged).
//! - El usuario confirma viendo de donde viene y lo que va a poder hacer
//!   (ver confirm_install), como el "Agregar X?" de Chrome.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    BrowserExtensionEnableCompletedHandler, BrowserExtensionRemoveCompletedHandler,
    ProfileAddBrowserExtensionCompletedHandler, ProfileGetBrowserExtensionsCompletedHandler,
};
use windows_core::{Interface, BOOL, HSTRING};

use crate::native::read_pwstr;
use crate::state::{Shared, UserEvent};
use crate::{crx, native, protocol, storage, sync};

const REGISTRY_FILE: &str = "extensions.json";
/// Tope de tamano de un .crx descargado (los mas grandes rondan los 30MB).
const MAX_CRX: u64 = 100 * 1024 * 1024;
/// Topes de lo que puede ocupar una extension descomprimida: un .zip de
/// pocos KB puede "inflarse" hasta llenar el disco (zip bomba).
const MAX_UNPACKED: u64 = 512 * 1024 * 1024;
const MAX_FILES: usize = 20_000;
const CANCELLED: &str = "Instalacion cancelada.";

pub fn extensions_dir() -> PathBuf {
    storage::data_dir().join("extensions")
}

/// id de extension -> carpeta desempaquetada. Global (no en AppState) porque
/// tambien lo consulta protocol.rs, desde sus hilos, para servir los iconos.
fn registry() -> &'static Mutex<BTreeMap<String, PathBuf>> {
    static REG: OnceLock<Mutex<BTreeMap<String, PathBuf>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(storage::load(REGISTRY_FILE)))
}

fn save_registry(reg: &BTreeMap<String, PathBuf>) {
    storage::save(REGISTRY_FILE, reg);
}

// ---------------------------------------------------------------------------
// Tiendas
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    Chrome,
    Edge,
}

/// Si la URL es la pagina de una extension en Chrome Web Store o Edge
/// Add-ons, devuelve la tienda y el id de la extension (32 letras a-p).
pub fn store_id(url: &str) -> Option<(Store, String)> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let path = path.split(['?', '#']).next().unwrap_or("");
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let store = match (host, segments.as_slice()) {
        ("chromewebstore.google.com", ["detail", ..]) => Store::Chrome,
        ("chrome.google.com", ["webstore", "detail", ..]) => Store::Chrome,
        ("microsoftedge.microsoft.com", ["addons", "detail", ..]) => Store::Edge,
        _ => return None,
    };
    let id = segments.iter().rev().find(|s| is_extension_id(s))?;
    Some((store, id.to_string()))
}

/// Una descarga que es el paquete de una extension de una tienda: lo que
/// hace el boton PROPIO de Chrome Web Store o Complementos de Edge (en vez
/// del "Agregar a Michi" de la barra). En lugar de guardarlo como un archivo
/// mas, se instala con install_from_store: se vuelve a pedir a la tienda
/// oficial por su id, se verifica la firma y se pide confirmacion.
/// Solo desde servidores de Google o Microsoft.
pub fn store_download(url: &str, file_name: &str, mime: &str) -> Option<(Store, String)> {
    let is_crx = mime.eq_ignore_ascii_case("application/x-chrome-extension")
        || file_name.to_ascii_lowercase().ends_with(".crx");
    if !is_crx {
        return None;
    }
    let authority = url.split_once("://")?.1.split(['/', '?', '#']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h).split(':').next()?.to_ascii_lowercase();
    let under = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    let store = if under("microsoft.com") {
        Store::Edge
    } else if under("google.com") || under("googleusercontent.com") {
        Store::Chrome
    } else {
        return None;
    };
    // El nombre del archivo empieza con el id ("<ID>_<version>.crx": Chrome
    // Web Store lo pone en MAYUSCULAS); si no, el id va en la URL del
    // servicio de actualizacion ("x=id%3D<id>...").
    let id_at = |s: &str| s.get(..32).map(str::to_ascii_lowercase).filter(|id| is_extension_id(id));
    let from_url = || {
        let start = url.find("id%3D").map(|i| i + 5).or_else(|| url.find("id=").map(|i| i + 3))?;
        id_at(&url[start..])
    };
    let id = id_at(file_name).or_else(from_url)?;
    Some((store, id))
}

fn is_extension_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

fn crx_url(store: Store, id: &str) -> String {
    match store {
        Store::Chrome => {
            // La tienda solo entrega versiones compatibles con el Chromium que
            // dice ser el cliente: le pasamos la version real del motor.
            let version = wry::webview_version().unwrap_or_else(|_| "130.0.0.0".into());
            format!(
                "https://clients2.google.com/service/update2/crx?response=redirect&prodversion={version}\
                 &acceptformat=crx3&x=id%3D{id}%26installsource%3Dondemand%26uc"
            )
        }
        Store::Edge => format!(
            "https://edge.microsoft.com/extensionwebstorebase/v1/crx?response=redirect\
             &x=id%3D{id}%26installsource%3Dondemand%26uc"
        ),
    }
}

// ---------------------------------------------------------------------------
// Instalar
// ---------------------------------------------------------------------------

pub fn install_from_store(state: &Shared, store: Store, id: String) {
    set_status(state, "Descargando extension...");
    let proxy = state.borrow().proxy.clone();
    std::thread::spawn(move || {
        let origin = match store {
            Store::Chrome => "Chrome Web Store, firma verificada",
            Store::Edge => "Complementos de Edge, firma verificada",
        };
        let result = download(&crx_url(store, &id)).and_then(|bytes| {
            let package = crx::verify(&bytes)?;
            if package.id != id {
                return Err("La tienda entrego una extension distinta de la pedida: no se instalo.".into());
            }
            let staged = Staged::unpack(package.archive)?;
            staged.confirm(origin)?;
            staged.commit(&extensions_dir().join(&id))
        });
        send_result(proxy, result);
    });
}

/// Instalar desde un .crx o .zip del disco (elegido con un dialogo).
pub fn install_from_file(state: &Shared) {
    let proxy = state.borrow().proxy.clone();
    std::thread::spawn(move || {
        let Some(file) = rfd::FileDialog::new()
            .set_parent(&crate::win::Owner)
            .set_title("Instalar extension")
            .add_filter("Extension (.crx, .zip)", &["crx", "zip"])
            .pick_file()
        else {
            return;
        };
        let result = std::fs::read(&file).map_err(|e| e.to_string()).and_then(|bytes| {
            let (archive, origin) = if crx::is_crx(&bytes) {
                (crx::verify(&bytes)?.archive, "archivo .crx, firma verificada")
            } else {
                (&bytes[..], "archivo .zip SIN FIRMA: no hay forma de saber quien lo hizo ni si fue modificado")
            };
            let staged = Staged::unpack(archive)?;
            staged.confirm(origin)?;
            staged.commit(&extensions_dir().join(format!("local-{}", unix_millis())))
        });
        send_result(proxy, result);
    });
}

/// Registrar una carpeta ya desempaquetada (como "Cargar descomprimida" en
/// Chrome): se usa en su lugar, sin copiarla.
pub fn install_from_folder(state: &Shared) {
    let proxy = state.borrow().proxy.clone();
    std::thread::spawn(move || {
        let Some(dir) = rfd::FileDialog::new().set_parent(&crate::win::Owner).set_title("Carpeta de la extension").pick_folder() else {
            return;
        };
        let result = if !dir.join("manifest.json").is_file() {
            Err("Esa carpeta no tiene un manifest.json: no es una extension desempaquetada.".into())
        } else if confirm_install(&dir, "carpeta descomprimida SIN FIRMA (para desarrolladores)") {
            Ok(dir)
        } else {
            Err(CANCELLED.into())
        };
        send_result(proxy, result);
    });
}

/// Una extension descomprimida en una carpeta temporal (dentro de
/// extensions_dir) hasta que el usuario confirma: asi cancelar no toca la
/// version ya instalada (reinstalar desde la tienda la reemplaza). Si no se
/// llega a `commit`, la carpeta temporal se borra sola.
struct Staged {
    dir: PathBuf,
    /// La carpeta con el manifest.json (`dir` o una subcarpeta).
    root: PathBuf,
}

impl Staged {
    fn unpack(zip: &[u8]) -> Result<Self, String> {
        let dir = extensions_dir().join(format!(".nueva-{}", unix_millis()));
        let mut staged = Staged { root: dir.clone(), dir };
        staged.root = unpack(zip, &staged.dir)?;
        Ok(staged)
    }

    fn confirm(&self, origin: &str) -> Result<(), String> {
        if confirm_install(&self.root, origin) {
            Ok(())
        } else {
            Err(CANCELLED.into())
        }
    }

    /// La pone en `dest` (reemplazando lo que hubiera) y devuelve su raiz.
    fn commit(self, dest: &Path) -> Result<PathBuf, String> {
        let rel = self.root.strip_prefix(&self.dir).map(Path::to_path_buf).unwrap_or_default();
        let _ = std::fs::remove_dir_all(dest);
        // El antivirus suele tener abiertos un momento los archivos recien
        // escritos, y mientras tanto la carpeta no se puede mover.
        let mut attempt = 0;
        while let Err(err) = std::fs::rename(&self.dir, dest) {
            attempt += 1;
            if attempt == 20 {
                return Err(err.to_string());
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        Ok(dest.join(rel))
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Carpetas temporales de instalaciones que no terminaron (el navegador se
/// cerro con la confirmacion abierta): al arrancar no hay ninguna en curso.
pub fn clean_staging() {
    let Ok(entries) = std::fs::read_dir(extensions_dir()) else { return };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(".nueva-") {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Antes de instalar, como Chrome: el nombre, de donde viene y lo que va a
/// poder hacer. Corre en el hilo de la instalacion (el dialogo es modal).
fn confirm_install(root: &Path, origin: &str) -> bool {
    let manifest = read_manifest(root).unwrap_or(Value::Null);
    let name = manifest
        .get("name")
        .and_then(Value::as_str)
        .map(|n| localize(root, &manifest, n))
        .unwrap_or_else(|| "esta extension".into());
    let warnings = permission_warnings(&manifest);
    let mut text = format!("Agregar \"{name}\"?\n\nOrigen: {origin}.\n\n");
    if warnings.is_empty() {
        text.push_str("No pide permisos especiales.");
    } else {
        text.push_str("Va a poder:\n");
        for warning in &warnings {
            text.push_str(&format!("  - {warning}\n"));
        }
    }
    rfd::MessageDialog::new()
        .set_parent(&crate::win::Owner)
        .set_level(rfd::MessageLevel::Warning)
        .set_title("Agregar extension")
        .set_description(text)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes
}

/// Lo que va a poder hacer una extension, en palabras (una version corta de
/// los avisos de Chrome), segun los permisos de su manifest. Lo mas
/// peligroso primero.
fn permission_warnings(manifest: &Value) -> Vec<String> {
    const APIS: &[(&str, &str)] = &[
        ("debugger", "Controlar las paginas con el depurador (ver y cambiar todo lo que hacen)"),
        ("nativeMessaging", "Comunicarse con programas instalados en el equipo"),
        ("proxy", "Cambiar el proxy (puede ver o desviar todo tu trafico)"),
        ("management", "Administrar tus otras extensiones"),
        ("privacy", "Cambiar tu configuracion de privacidad"),
        ("contentSettings", "Cambiar lo que pueden hacer los sitios (camara, microfono, JavaScript...)"),
        ("history", "Leer y cambiar tu historial de navegacion"),
        ("tabs", "Ver la direccion de todas tus pestanas"),
        ("webNavigation", "Ver la direccion de todas tus pestanas"),
        ("bookmarks", "Leer y cambiar tus marcadores"),
        ("downloads", "Administrar tus descargas"),
        ("desktopCapture", "Capturar lo que se ve en tu pantalla"),
        ("tabCapture", "Capturar el contenido de tus pestanas"),
        ("clipboardRead", "Leer lo que copias"),
        ("clipboardWrite", "Cambiar lo que copias y pegas"),
        ("geolocation", "Saber tu ubicacion"),
        ("topSites", "Ver tus sitios mas visitados"),
        ("declarativeNetRequest", "Bloquear o cambiar contenido de las paginas"),
        ("webRequestBlocking", "Bloquear o cambiar contenido de las paginas"),
        ("notifications", "Mostrar notificaciones"),
    ];
    let strings = |v: Option<&Value>| -> Vec<String> {
        v.and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default()
    };
    // En Manifest V2 los sitios van mezclados con el resto en "permissions".
    let (mut hosts, apis): (Vec<String>, Vec<String>) = strings(manifest.get("permissions"))
        .into_iter()
        .partition(|p| p.contains("://") || p == "<all_urls>");
    hosts.extend(strings(manifest.get("host_permissions")));
    for script in manifest.get("content_scripts").and_then(Value::as_array).into_iter().flatten() {
        hosts.extend(strings(script.get("matches")));
    }

    let mut out = Vec::new();
    if hosts.iter().any(|h| h == "<all_urls>" || host_of_pattern(h) == Some("*")) {
        out.push("Leer y cambiar todos tus datos en todos los sitios web".to_string());
    } else {
        let mut names: Vec<&str> = hosts
            .iter()
            .filter_map(|h| host_of_pattern(h))
            .map(|h| h.trim_start_matches("*."))
            .collect();
        names.sort_unstable();
        names.dedup();
        if !names.is_empty() {
            let shown = names.iter().take(3).copied().collect::<Vec<_>>().join(", ");
            out.push(match names.len().saturating_sub(3) {
                0 => format!("Leer y cambiar tus datos en {shown}"),
                more => format!("Leer y cambiar tus datos en {shown} y {more} sitios mas"),
            });
        }
    }
    for (perm, text) in APIS {
        if apis.iter().any(|a| a == perm) && !out.iter().any(|o| o == text) {
            out.push(text.to_string());
        }
    }
    out
}

/// Host de un patron de coincidencia ("https://*.google.com/*" -> "*.google.com").
fn host_of_pattern(pattern: &str) -> Option<&str> {
    let host = pattern.split_once("://")?.1.split('/').next()?;
    (!host.is_empty()).then_some(host)
}

fn send_result(proxy: Option<winit::event_loop::EventLoopProxy<UserEvent>>, result: Result<PathBuf, String>) {
    let Some(proxy) = proxy else { return };
    let _ = proxy.send_event(match result {
        Ok(dir) => UserEvent::ExtensionUnpacked(dir),
        Err(err) => UserEvent::ExtensionFailed(err),
    });
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    // El pedido a la tienda va por HTTPS, pero Complementos de Edge redirige
    // el paquete a su red de descargas, que solo lo sirve por HTTP (su
    // certificado HTTPS no es valido para ese nombre; Edge tambien lo baja
    // asi). Alguien en la red podria cambiar el paquete en el camino: por
    // eso lo que protege no es el transporte sino la firma, que se verifica
    // siempre antes de instalar (ver crx.rs e install_from_store).
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(120)).build();
    let response = agent
        .get(url)
        .call()
        .map_err(|e| format!("No se pudo descargar la extension ({e})."))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_CRX)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Fallo la descarga ({e})."))?;
    Ok(bytes)
}

/// Descomprime el .zip de una extension (ya sin el encabezado del .crx) en
/// `dest` (reemplazando lo que hubiera) y devuelve la carpeta que tiene el
/// manifest.json.
fn unpack(zip_bytes: &[u8], dest: &Path) -> Result<PathBuf, String> {
    unpack_limited(zip_bytes, dest, MAX_UNPACKED)
}

fn unpack_limited(zip_bytes: &[u8], dest: &Path, max_bytes: u64) -> Result<PathBuf, String> {
    let too_big = || "La extension descomprimida es demasiado grande.".to_string();
    let mut archive =
        zip::ZipArchive::new(Cursor::new(zip_bytes)).map_err(|e| format!("No es una extension valida ({e})."))?;
    if archive.len() > MAX_FILES {
        return Err(too_big());
    }

    let _ = std::fs::remove_dir_all(dest);
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut budget = max_bytes;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        // enclosed_name descarta rutas con ".." o absolutas: un zip no puede
        // escribir fuera de la carpeta de la extension.
        let Some(rel) = entry.enclosed_name() else { continue };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut file = std::fs::File::create(&out).map_err(|e| e.to_string())?;
            // Se cuenta lo que de verdad sale al descomprimir, no el tamano
            // que declara el zip (que puede mentir).
            let written = std::io::copy(&mut (&mut entry).take(budget + 1), &mut file).map_err(|e| e.to_string())?;
            if written > budget {
                return Err(too_big());
            }
            budget -= written;
        }
    }

    // Algunos .zip traen todo dentro de una carpeta.
    let root = if dest.join("manifest.json").is_file() {
        dest.to_path_buf()
    } else {
        std::fs::read_dir(dest)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.join("manifest.json").is_file())
            .ok_or("El archivo no tiene un manifest.json: no es una extension.")?
    };
    // Chromium se niega a cargar una extension desempaquetada con carpetas
    // que empiezan con "_" que no conoce; "_metadata" (las firmas de la
    // tienda) viene en todo .crx y no hace falta para funcionar.
    let _ = std::fs::remove_dir_all(root.join("_metadata"));
    Ok(root)
}

/// Registra en WebView2 una extension ya desempaquetada en `dir`. Hilo principal.
pub fn install_dir(state: &Shared, dir: PathBuf) {
    let Some(profile) = profile7(state) else {
        return set_status(state, "Este WebView2 no soporta extensiones (actualiza Microsoft Edge WebView2).");
    };
    set_status(state, "Instalando...");
    let s = state.clone();
    let path = dir.clone();
    let handler = ProfileAddBrowserExtensionCompletedHandler::create(Box::new(move |result, ext| {
        match (result, ext) {
            (Ok(()), Some(ext)) => {
                let (id, name) = unsafe { (read_pwstr(|p| ext.Id(p)), read_pwstr(|p| ext.Name(p))) };
                {
                    let mut reg = registry().lock().unwrap();
                    reg.insert(id, path);
                    save_registry(&reg);
                }
                set_status(&s, &format!("Extension instalada: {name}"));
            }
            (Err(err), _) => {
                set_status(&s, &format!("No se pudo instalar la extension ({}).", err.message()));
            }
            (Ok(()), None) => set_status(&s, "No se pudo instalar la extension."),
        }
        refresh(&s);
        Ok(())
    }));
    if let Err(err) = unsafe { profile.AddBrowserExtension(&HSTRING::from(dir.as_path()), &handler) } {
        set_status(state, &format!("No se pudo instalar la extension ({}).", err.message()));
    }
}

// ---------------------------------------------------------------------------
// Listar / activar / quitar
// ---------------------------------------------------------------------------

/// Pide la lista actual a WebView2 y se la manda a la pagina de ajustes.
pub fn refresh(state: &Shared) {
    let Some(profile) = profile7(state) else {
        return sync::push_extensions(state, &Value::Array(vec![]), false);
    };
    let s = state.clone();
    let handler = ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(move |_, list| {
        // Solo las que instalo el usuario (las del registro). WebView2 trae
        // las suyas propias (visor de PDF, portapapeles...), y quitar esas
        // romperia cosas como abrir un PDF.
        let reg = registry().lock().unwrap().clone();
        let items: Vec<Value> = list
            .map(|l| {
                collect(&l)
                    .iter()
                    .filter(|e| reg.contains_key(&unsafe { read_pwstr(|p| e.Id(p)) }))
                    .map(describe)
                    .collect()
            })
            .unwrap_or_default();
        // Barra de iconos: las activas que tienen popup (las demas no tienen
        // nada que mostrar al tocarlas).
        let toolbar: Vec<Value> = items
            .iter()
            .filter(|x| x["enabled"].as_bool() == Some(true) && x["popup"].as_bool() == Some(true))
            .map(|x| json!({ "id": x["id"], "name": x["name"], "icon": x["icon"] }))
            .collect();
        sync::push_toolbar(&s, &Value::Array(toolbar));
        sync::push_extensions(&s, &Value::Array(items), true);
        Ok(())
    }));
    let _ = unsafe { profile.GetBrowserExtensions(&handler) };
}

pub fn set_enabled(state: &Shared, id: String, enabled: bool) {
    with_extension(state, id, move |s, ext| {
        let s2 = s.clone();
        let handler = BrowserExtensionEnableCompletedHandler::create(Box::new(move |_| {
            refresh(&s2);
            Ok(())
        }));
        let _ = unsafe { ext.Enable(enabled, &handler) };
    });
}

pub fn remove(state: &Shared, id: String) {
    let id2 = id.clone();
    with_extension(state, id, move |s, ext| {
        let s2 = s.clone();
        let handler = BrowserExtensionRemoveCompletedHandler::create(Box::new(move |result| {
            if result.is_ok() {
                let mut reg = registry().lock().unwrap();
                if let Some(dir) = reg.remove(&id2) {
                    // Solo borramos carpetas que desempaquetamos nosotros; una
                    // cargada con "desde carpeta" es del usuario.
                    if let Some(own) = owned_root(&dir) {
                        let _ = std::fs::remove_dir_all(own);
                    }
                }
                save_registry(&reg);
            }
            refresh(&s2);
            Ok(())
        }));
        let _ = unsafe { ext.Remove(&handler) };
    });
}

/// Pagina interna de la extension para abrir en una pestana: "popup" (lo que
/// en Chrome se abre al tocar su icono) u "options".
pub fn page_url(id: &str, which: &str) -> Option<String> {
    let dir = registry().lock().unwrap().get(id)?.clone();
    let manifest = read_manifest(&dir)?;
    let page = match which {
        "popup" => manifest
            .pointer("/action/default_popup")
            .or_else(|| manifest.pointer("/browser_action/default_popup")),
        "options" => manifest.pointer("/options_ui/page").or_else(|| manifest.get("options_page")),
        _ => None,
    }?
    .as_str()?
    .trim_start_matches('/');
    Some(format!("chrome-extension://{id}/{page}"))
}

/// Archivo del icono mas grande que declara la extension (para protocol.rs).
pub fn icon_file(id: &str) -> Option<PathBuf> {
    let dir = registry().lock().ok()?.get(id)?.clone();
    let manifest = read_manifest(&dir)?;
    let icons = manifest.get("icons")?.as_object()?;
    let (_, rel) = icons
        .iter()
        .filter_map(|(size, path)| Some((size.parse::<u32>().ok()?, path.as_str()?)))
        .max_by_key(|(size, _)| *size)?;
    let path = dir.join(rel.trim_start_matches('/'));
    // Que el manifest no pueda apuntar fuera de la carpeta de la extension.
    let canon = path.canonicalize().ok()?;
    canon.starts_with(dir.canonicalize().ok()?).then_some(canon)
}

fn with_extension(
    state: &Shared,
    id: String,
    f: impl FnOnce(&Shared, ICoreWebView2BrowserExtension) + 'static,
) {
    let Some(profile) = profile7(state) else { return };
    let s = state.clone();
    let handler = ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(move |_, list| {
        let found = list.and_then(|l| {
            collect(&l)
                .into_iter()
                .find(|e| unsafe { read_pwstr(|p| e.Id(p)) } == id)
        });
        match found {
            Some(ext) => f(&s, ext),
            None => refresh(&s),
        }
        Ok(())
    }));
    let _ = unsafe { profile.GetBrowserExtensions(&handler) };
}

fn collect(list: &ICoreWebView2BrowserExtensionList) -> Vec<ICoreWebView2BrowserExtension> {
    let mut count = 0u32;
    if unsafe { list.Count(&mut count) }.is_err() {
        return Vec::new();
    }
    (0..count).filter_map(|i| unsafe { list.GetValueAtIndex(i) }.ok()).collect()
}

fn describe(ext: &ICoreWebView2BrowserExtension) -> Value {
    let (id, name) = unsafe { (read_pwstr(|p| ext.Id(p)), read_pwstr(|p| ext.Name(p))) };
    let mut enabled = BOOL(0);
    let _ = unsafe { ext.IsEnabled(&mut enabled) };

    let dir = registry().lock().unwrap().get(&id).cloned();
    let manifest = dir.as_deref().and_then(read_manifest);
    let text = |key: &str| -> String {
        manifest
            .as_ref()
            .and_then(|m| m.get(key))
            .and_then(Value::as_str)
            .map(|s| localize(dir.as_deref().unwrap(), manifest.as_ref().unwrap(), s))
            .unwrap_or_default()
    };
    let description = text("description");
    let version = text("version");
    // URL ya armada, con la clave de la sesion (ver protocol::icon_url).
    let icon = icon_file(&id).is_some().then(|| protocol::icon_url(&id));
    json!({
        "id": id,
        "name": name,
        "enabled": enabled.as_bool(),
        "version": version,
        "description": description,
        "icon": icon,
        "popup": page_url(&id, "popup").is_some(),
        "options": page_url(&id, "options").is_some(),
    })
}

fn read_manifest(dir: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).ok()?).ok()
}

/// Los manifest suelen traer "__MSG_nombre__" en vez del texto: se busca en
/// _locales/<idioma>/messages.json, primero en espanol y si no en el idioma
/// por defecto de la extension.
fn localize(dir: &Path, manifest: &Value, s: &str) -> String {
    let Some(key) = s.strip_prefix("__MSG_").and_then(|k| k.strip_suffix("__")) else {
        return s.to_string();
    };
    let default = manifest.get("default_locale").and_then(Value::as_str).unwrap_or("en");
    for locale in ["es", "es_419", default] {
        let Some(messages) = std::fs::read(dir.join("_locales").join(locale).join("messages.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        else {
            continue;
        };
        let found = messages.as_object().and_then(|m| {
            m.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .and_then(|(_, v)| v.get("message")?.as_str())
        });
        if let Some(text) = found {
            return text.to_string();
        }
    }
    s.to_string()
}

/// Si `dir` esta dentro de extensions_dir(), la carpeta de primer nivel que
/// la contiene (la que creo unpack) — la que hay que borrar al desinstalar.
fn owned_root(dir: &Path) -> Option<PathBuf> {
    let base = extensions_dir();
    let rel = dir.strip_prefix(&base).ok()?;
    let first = rel.components().next()?;
    Some(base.join(first))
}

fn profile7(state: &Shared) -> Option<ICoreWebView2Profile7> {
    let st = state.borrow();
    let webview = st.topbar.as_ref()?;
    native::profile(webview)?.cast::<ICoreWebView2Profile7>().ok()
}

fn set_status(state: &Shared, message: &str) {
    state.borrow_mut().extension_status = message.to_string();
    sync::push_extension_status(state);
}

fn unix_millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_store_pages() {
        let id = "cjpalhdlnbpafiamejdnhcphjbkeiagm";
        assert_eq!(
            store_id(&format!("https://chromewebstore.google.com/detail/ublock-origin/{id}?hl=es")),
            Some((Store::Chrome, id.into()))
        );
        assert_eq!(
            store_id(&format!("https://chrome.google.com/webstore/detail/x/{id}")),
            Some((Store::Chrome, id.into()))
        );
        assert_eq!(
            store_id(&format!("https://microsoftedge.microsoft.com/addons/detail/ublock/{id}")),
            Some((Store::Edge, id.into()))
        );
        assert_eq!(store_id("https://chromewebstore.google.com/"), None);
        assert_eq!(store_id(&format!("https://evil.com/detail/x/{id}")), None);
    }

    #[test]
    fn store_package_downloads_are_installed_instead_of_saved() {
        let id = "ddkjiahejlhfcafbddmgiahcphecmpfh";
        // Lo que entrega de verdad Chrome Web Store: el id en mayusculas.
        let upper = id.to_ascii_uppercase();
        assert_eq!(
            store_download(
                &format!("https://clients2.googleusercontent.com/crx/blobs/AZPV/{upper}_2026_930_1227_0.crx"),
                &format!("{upper}_2026_930_1227_0.crx"),
                "application/x-chrome-extension"
            ),
            Some((Store::Chrome, id.into()))
        );
        assert_eq!(
            store_download(
                &format!("https://clients2.google.com/service/update2/crx?response=redirect&x=id%3D{id}%26uc"),
                "extension.crx",
                ""
            ),
            Some((Store::Chrome, id.into()))
        );
        assert_eq!(
            store_download(&format!("http://msedgeextensions.f.tlu.dl.delivery.mp.microsoft.com/x/{id}.crx"), &format!("{id}.crx"), ""),
            Some((Store::Edge, id.into()))
        );
        // Desde cualquier otro sitio, o si no es un .crx: una descarga normal.
        assert_eq!(store_download("https://evil.com/x.crx", &format!("{id}.crx"), ""), None);
        assert_eq!(store_download("https://google.com.evil.com/x.crx", &format!("{id}.crx"), ""), None);
        assert_eq!(store_download("https://clients2.google.com/foto.png", "foto.png", "image/png"), None);
    }

    #[test]
    fn explains_what_an_extension_can_do() {
        let all_sites = json!({
            "permissions": ["tabs", "storage", "<all_urls>", "nativeMessaging"],
        });
        assert_eq!(
            permission_warnings(&all_sites),
            vec![
                "Leer y cambiar todos tus datos en todos los sitios web",
                "Comunicarse con programas instalados en el equipo",
                "Ver la direccion de todas tus pestanas",
            ]
        );
        let some_sites = json!({
            "host_permissions": ["https://*.github.com/*", "https://gitlab.com/*"],
            "content_scripts": [{ "matches": ["https://github.com/*", "https://a.com/*", "https://b.com/*"] }],
            "permissions": ["tabs", "webNavigation"],
        });
        assert_eq!(
            permission_warnings(&some_sites),
            vec![
                "Leer y cambiar tus datos en a.com, b.com, github.com y 1 sitios mas",
                "Ver la direccion de todas tus pestanas",
            ]
        );
        assert!(permission_warnings(&json!({ "permissions": ["storage"] })).is_empty());
        assert_eq!(
            permission_warnings(&json!({ "content_scripts": [{ "matches": ["*://*/*"] }] }))[0],
            "Leer y cambiar todos tus datos en todos los sitios web"
        );
    }

    #[test]
    fn unpacking_stops_a_zip_bomb() {
        use std::io::Write;
        let mut zip_bytes = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut zip_bytes));
            let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("manifest.json", options).unwrap();
            zip.write_all(br#"{"name":"x","version":"1"}"#).unwrap();
            zip.start_file("relleno.bin", options).unwrap();
            zip.write_all(&vec![0u8; 4 << 20]).unwrap();
            zip.finish().unwrap();
        }
        // 4 MB de ceros entran en unos pocos KB comprimidos.
        assert!(zip_bytes.len() < 64 << 10);
        let dest = std::env::temp_dir().join(format!("navegador-zipbomb-{}", std::process::id()));
        assert!(unpack_limited(&zip_bytes, &dest, 1 << 20).unwrap_err().contains("demasiado grande"));
        assert!(unpack_limited(&zip_bytes, &dest, 8 << 20).is_ok());
        std::fs::remove_dir_all(&dest).unwrap();
    }

    /// Descarga extensiones reales de las dos tiendas y verifica su firma.
    /// Usa la red: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn verifies_real_store_packages() {
        for (store, id) in [
            (Store::Chrome, "ddkjiahejlhfcafbddmgiahcphecmpfh"), // uBlock Origin Lite
            (Store::Edge, "odfafepnkmbhccpbejgmiehpchacaeak"),   // uBlock Origin
        ] {
            let bytes = download(&crx_url(store, id)).unwrap();
            let package = crx::verify(&bytes).unwrap();
            assert_eq!(package.id, id, "{store:?}");
            let mut tampered = bytes.clone();
            let last = tampered.len() - 1;
            tampered[last] ^= 0xff;
            assert!(crx::verify(&tampered).is_err());
        }
    }
}
