//! Maneja las peticiones al protocolo personalizado "app://", que sirve
//! nuestras propias paginas (la "nueva pestana" y las 3 de interfaz: barra
//! superior y paneles laterales), el script compartido del fondo animado y
//! los videos de fondo seleccionables como tema. Cualquier ruta que no sea
//! exactamente una de las reconocidas devuelve 404 — en particular, los
//! nombres de video se resuelven contra una lista fija (THEMES), nunca
//! contra una ruta de archivo arbitraria, para que no haya forma de pedir un
//! archivo fuera de assets/wallpapers/optimized.
//!
//! Se registra como protocolo ASINCRONO (ver `handle`): WebView2 llama al
//! handler en el hilo principal, el mismo del bucle de eventos y de la
//! animacion de los paneles. Leer video de disco ahi (antes se leia el
//! archivo ENTERO, hasta ~47MB, en la primera peticion) congelaba toda la
//! interfaz y demoraba el primer cuadro del fondo; ahora cada rango pedido
//! se lee en un hilo aparte y wry devuelve la respuesta al hilo principal.
//!
//! Seguridad: el protocolo se registra tambien en las pestanas (ahi viven la
//! pestana nueva y los ajustes), asi que cualquier pagina web abierta en una
//! pestana puede pedir "http://app.localhost/...". Por eso:
//! - Nuestras paginas no se pueden mostrar dentro de un iframe ajeno
//!   (frame-ancestors / X-Frame-Options): un sitio no puede tapar los
//!   ajustes con algo transparente y hacer que el usuario toque "Borrar
//!   datos" o "Quitar" creyendo que toca otra cosa.
//! - Ninguna respuesta se puede cargar desde otro origen como imagen, video
//!   o script (Cross-Origin-Resource-Policy), y los iconos de extensiones
//!   llevan ademas una clave al azar en la URL (ver icon_url): si no, una
//!   pagina podria averiguar que extensiones tiene instaladas el usuario.
//! - Las paginas llevan una CSP con nonce (ver CSP).

use std::borrow::Cow;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::OnceLock;

use ring::rand::{SecureRandom, SystemRandom};
use wry::http::header::{
    ACCEPT_RANGES, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_SECURITY_POLICY, CONTENT_TYPE, RANGE,
    REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use wry::http::response::Builder;
use wry::http::{Request, Response, StatusCode};
use wry::RequestAsyncResponder;

/// Politica de contenido de nuestras paginas. Solo corren los <script> que
/// trae la propia pagina (marcados con el nonce de esta respuesta): si
/// alguna vez se colara HTML ajeno en una (un titulo de pagina o un nombre
/// de extension sin escapar), sus scripts y atributos tipo onerror= no se
/// ejecutan. Y nada se carga ni se envia fuera de app://.
const CSP: &str = "default-src 'none'; script-src 'nonce-{nonce}'; style-src 'self' 'unsafe-inline'; \
                   img-src 'self' data:; media-src 'self'; base-uri 'none'; form-action 'none'; \
                   frame-ancestors 'none'";

/// Tope de bytes por respuesta parcial de video. El <video> suele pedir
/// "bytes=0-" (todo desde el inicio); devolver un trozo es valido (206 con
/// su Content-Range) y el navegador simplemente pide el siguiente. 2MB son
/// ~1-2s de video a 1080p60: suficiente para arrancar sin esperar de mas.
const MAX_CHUNK: u64 = 2 * 1024 * 1024;

/// Versiones re-codificadas por tools/encode_wallpapers.sh (1080p60 y la
/// version chica pre-difuminada). Los originales 4K de assets/wallpapers
/// NO se sirven: decodificar 4K en 4 superficies a la vez era lo que hacia
/// ir a tirones el fondo.
const DEV_WALLPAPERS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/wallpapers/optimized");

/// Carpeta de los fondos: instalado, "wallpapers" junto al .exe (ver el
/// instalador, installer/); compilando desde el proyecto, la de assets/.
fn wallpapers_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.parent()?.join("wallpapers")))
            .filter(|dir| dir.is_dir())
            .unwrap_or_else(|| PathBuf::from(DEV_WALLPAPERS_DIR))
    })
}

/// Temas disponibles: (id, nombre visible). Las paginas reciben esta lista
/// (ver sync::push_own_pages). Cada uno se sirve como
/// "/wallpapers/<tema>.mp4" (pestana nueva) y "/wallpapers/<tema>.blur.mp4"
/// (barra, paneles, menu; ver ui/wallpaper.js), o con ".jpg"/".blur.jpg" si
/// el tema es una imagen fija (STILLS en ese script).
pub const THEMES: &[(&str, &str)] = &[
    ("sleepy-rainy-evening", "Lluvia nocturna"),
    ("torii-carmesi", "Torii carmesi"),
    ("blindfolded-girl", "Petalos y luna"),
    ("wuthering-waves-chisa", "Wuthering Waves"),
];

pub fn is_theme(id: &str) -> bool {
    THEMES.iter().any(|(t, _)| *t == id)
}

/// Variantes que puede tener cada tema: (sufijo del archivo, tipo MIME).
const VARIANTS: &[(&str, &str)] = &[
    (".blur.mp4", "video/mp4"),
    (".mp4", "video/mp4"),
    (".blur.jpg", "image/jpeg"),
    (".jpg", "image/jpeg"),
];

type Body = Cow<'static, [u8]>;

pub fn handle(request: Request<Vec<u8>>, responder: RequestAsyncResponder) {
    let path = request.uri().path();

    let page: Option<&'static [u8]> = match path {
        "/" | "" => Some(include_bytes!("../ui/newtab.html")),
        "/topbar" => Some(include_bytes!("../ui/topbar.html")),
        "/sidebar-left" => Some(include_bytes!("../ui/sidebar_left.html")),
        "/sidebar-right" => Some(include_bytes!("../ui/sidebar_right.html")),
        "/menu" => Some(include_bytes!("../ui/menu.html")),
        "/suggest" => Some(include_bytes!("../ui/suggest.html")),
        "/settings" => Some(include_bytes!("../ui/settings.html")),
        "/insecure" => Some(include_bytes!("../ui/insecure.html")),
        _ => None,
    };
    if let Some(body) = page {
        return responder.respond(respond_page(body));
    }
    if path == "/wallpaper.js" {
        return responder.respond(respond_full(include_bytes!("../ui/wallpaper.js"), "text/javascript; charset=utf-8"));
    }
    // Logo del navegador (ver tools/make_icon.ps1).
    if path == "/icon.png" {
        return responder.respond(respond_full(include_bytes!("../assets/icon/michi-256.png"), "image/png"));
    }

    // Icono de una extension instalada (pagina de extensiones, barra). Solo
    // con la clave de esta sesion (ver icon_url), solo ids del registro, y
    // solo el archivo que declara su manifest (ver extensions::icon_file).
    if let Some(rest) = path.strip_prefix("/ext-icon/") {
        let Some(id) = rest.strip_prefix(icon_token()).and_then(|r| r.strip_prefix('/')) else {
            return responder.respond(not_found());
        };
        let id = id.to_string();
        std::thread::spawn(move || {
            let response = crate::extensions::icon_file(&id)
                .and_then(|file| {
                    let mime = match file.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()) {
                        Some(e) if e == "svg" => "image/svg+xml",
                        Some(e) if e == "jpg" || e == "jpeg" => "image/jpeg",
                        Some(e) if e == "webp" => "image/webp",
                        _ => "image/png",
                    };
                    let bytes = std::fs::read(file).ok()?;
                    Some(
                        base()
                            .header(CONTENT_TYPE, mime)
                            .header(CONTENT_LENGTH, bytes.len())
                            // Un SVG abierto directo como documento no puede ejecutar nada.
                            .header(CONTENT_SECURITY_POLICY, "default-src 'none'; style-src 'unsafe-inline'; sandbox")
                            .body(Cow::Owned(bytes))
                            .unwrap(),
                    )
                })
                .unwrap_or_else(not_found);
            responder.respond(response);
        });
        return;
    }

    let Some((file, mime)) = path.strip_prefix("/wallpapers/").and_then(wallpaper_file) else {
        return responder.respond(not_found());
    };

    std::thread::spawn(move || {
        let response = respond_ranged(&request, &file, mime).unwrap_or_else(|_| not_found());
        responder.respond(response);
    });
}

/// "<tema><variante>" -> (archivo real, MIME), solo si <tema> es uno de THEMES
/// y <variante> una de VARIANTS. Si ese tema no tiene esa variante (ej. un
/// .jpg de un tema de video) el archivo no existe y la respuesta es 404.
fn wallpaper_file(requested: &str) -> Option<(PathBuf, &'static str)> {
    VARIANTS.iter().find_map(|&(suffix, mime)| {
        let theme = requested.strip_suffix(suffix)?;
        is_theme(theme).then(|| (wallpapers_dir().join(requested), mime))
    })
}

/// URL (relativa a app://) del icono de una extension, con la clave de esta
/// sesion. Las paginas la reciben ya armada (ver extensions::describe).
pub fn icon_url(id: &str) -> String {
    format!("/ext-icon/{}/{id}", icon_token())
}

/// Clave al azar de esta sesion para las URL de iconos de extensiones (ver
/// el comentario del modulo).
fn icon_token() -> &'static str {
    static TOKEN: OnceLock<String> = OnceLock::new();
    TOKEN.get_or_init(|| random_hex(16))
}

/// `bytes` bytes al azar del generador del sistema, en hexadecimal.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    // Si el generador del sistema fallara (en Windows no pasa), mejor
    // cerrarse que usar un valor predecible.
    SystemRandom::new().fill(&mut buf).expect("generador aleatorio del sistema");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Cabeceras de todas las respuestas: el tipo declarado es el real (sin
/// adivinarlo por el contenido), no se pueden cargar desde otro origen, y
/// no se filtra la URL a nadie por el Referer.
fn base() -> Builder {
    Response::builder()
        .header(X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header("Cross-Origin-Resource-Policy", "same-origin")
        .header(REFERRER_POLICY, "no-referrer")
}

fn not_found() -> Response<Body> {
    base().status(404).body(Cow::Borrowed(&b""[..])).unwrap()
}

/// Una de nuestras paginas HTML, con su CSP: cada <script> de la pagina
/// recibe el nonce (nuevo en cada respuesta) que la politica permite.
fn respond_page(body: &'static [u8]) -> Response<Body> {
    let nonce = random_hex(16);
    let html = String::from_utf8_lossy(body).replace("<script", &format!("<script nonce=\"{nonce}\""));
    base()
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .header(CONTENT_LENGTH, html.len())
        .header(CONTENT_SECURITY_POLICY, CSP.replace("{nonce}", &nonce))
        .header(X_FRAME_OPTIONS, "DENY")
        // Ver respond_full.
        .header(CACHE_CONTROL, "no-store")
        .status(200)
        .body(Cow::Owned(html.into_bytes()))
        .unwrap()
}

fn respond_full(body: &'static [u8], mime: &'static str) -> Response<Body> {
    base()
        .header(CONTENT_TYPE, mime)
        .header(CONTENT_LENGTH, body.len())
        // Sin esto, el navegador puede quedarse sirviendo una copia vieja de
        // la pagina (heuristica de cache HTTP sin validadores) despues de
        // recompilar: cada `cargo build` cambia el contenido de esta pagina
        // pero la URL ("/") nunca cambia, asi que sin decirle explicitamente
        // que no la guarde, WebView2 puede reusar la version anterior
        // indefinidamente entre corridas (el cache vive en el disco, no en
        // el proceso).
        .header(CACHE_CONTROL, "no-store")
        .status(200)
        .body(Cow::Borrowed(body))
        .unwrap()
}

/// Responde el archivo respetando la cabecera `Range` que manda el elemento
/// `<video>` del navegador, leyendo de disco SOLO el rango pedido (el cache
/// de archivos del SO se encarga de que releerlo sea gratis). Esto no es
/// opcional: WebView2/Chromium le pide a todo recurso de video/audio un
/// fragmento con `Range: bytes=...` antes de reproducirlo, y si la respuesta
/// ignora ese pedido y devuelve 200 con el archivo entero (en vez de 206 con
/// el fragmento pedido y las cabeceras `Content-Range`/`Accept-Ranges`),
/// Chromium descarta la respuesta como invalida y el video nunca llega a
/// reproducirse — sin ningun error visible en la pagina, solo se queda sin
/// video.
fn respond_ranged(request: &Request<Vec<u8>>, path: &PathBuf, mime: &'static str) -> std::io::Result<Response<Body>> {
    let mut file = File::open(path)?;
    let total = file.metadata()?.len();

    let range = request
        .headers()
        .get(RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_range);

    let Some((start, end)) = range else {
        // Sin cabecera Range: se pide el recurso completo. Igual anunciamos
        // Accept-Ranges para que el navegador sepa que puede pedir
        // fragmentos en la proxima peticion (lo que casi siempre hace).
        let mut body = Vec::with_capacity(total as usize);
        file.read_to_end(&mut body)?;
        return Ok(base()
            .status(200)
            .header(CONTENT_TYPE, mime)
            .header(CONTENT_LENGTH, body.len())
            .header(ACCEPT_RANGES, "bytes")
            .body(Cow::Owned(body))
            .unwrap());
    };

    let end = end
        .unwrap_or(u64::MAX)
        .min(total.saturating_sub(1))
        .min(start.saturating_add(MAX_CHUNK - 1));
    if total == 0 || start >= total || start > end {
        return Ok(base()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(CONTENT_RANGE, format!("bytes */{total}"))
            .body(Cow::Borrowed(&b""[..]))
            .unwrap());
    }

    let mut chunk = vec![0; (end - start + 1) as usize];
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(&mut chunk)?;
    Ok(base()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(CONTENT_TYPE, mime)
        .header(CONTENT_LENGTH, chunk.len())
        .header(CONTENT_RANGE, format!("bytes {start}-{end}/{total}"))
        .header(ACCEPT_RANGES, "bytes")
        .body(Cow::Owned(chunk))
        .unwrap())
}

/// Parsea una cabecera `Range: bytes=start-end` (con `end` opcional, ej.
/// "bytes=1000-"). Solo se soporta un unico rango simple: es la unica forma
/// que mandan los navegadores reales para `<video>`; formas mas raras
/// (multiples rangos, sufijo "-500") se tratan como ausentes.
fn parse_range(header: &str) -> Option<(u64, Option<u64>)> {
    let spec = header.strip_prefix("bytes=")?;
    let (start_s, end_s) = spec.split_once('-')?;
    let start: u64 = start_s.trim().parse().ok()?;
    let end = if end_s.trim().is_empty() {
        None
    } else {
        Some(end_s.trim().parse().ok()?)
    };
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_script_gets_this_responses_nonce() {
        let pages: [&'static [u8]; 8] = [
            include_bytes!("../ui/newtab.html"),
            include_bytes!("../ui/topbar.html"),
            include_bytes!("../ui/sidebar_left.html"),
            include_bytes!("../ui/sidebar_right.html"),
            include_bytes!("../ui/menu.html"),
            include_bytes!("../ui/suggest.html"),
            include_bytes!("../ui/settings.html"),
            include_bytes!("../ui/insecure.html"),
        ];
        for page in pages {
            let response = respond_page(page);
            let csp = response.headers()[CONTENT_SECURITY_POLICY].to_str().unwrap().to_string();
            let nonce = csp.split("'nonce-").nth(1).unwrap().split('\'').next().unwrap().to_string();
            assert_eq!(nonce.len(), 32);
            assert!(csp.contains("frame-ancestors 'none'"));
            let html = String::from_utf8(response.body().to_vec()).unwrap();
            let scripts = html.matches("<script").count();
            assert!(scripts > 0);
            assert_eq!(html.matches(&format!("<script nonce=\"{nonce}\"")).count(), scripts);
            // Sin manejadores en linea (onclick="..."): la CSP los bloquearia.
            assert!(!html.contains(" onclick=") && !html.contains(" onload=") && !html.contains(" onerror="));
        }
        // Cada respuesta, un nonce distinto.
        assert_ne!(
            respond_page(include_bytes!("../ui/menu.html")).headers()[CONTENT_SECURITY_POLICY],
            respond_page(include_bytes!("../ui/menu.html")).headers()[CONTENT_SECURITY_POLICY]
        );
    }

    #[test]
    fn extension_icons_need_the_session_key() {
        let url = icon_url("abc");
        assert!(url.starts_with("/ext-icon/") && url.ends_with("/abc"));
        assert_eq!(url.len(), "/ext-icon/".len() + 32 + "/abc".len());
    }
}
