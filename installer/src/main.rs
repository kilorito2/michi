//! Instalador (y desinstalador) de Michi.
//!
//! Misma base que el navegador: una ventana sin bordes de winit con un
//! WebView2 que muestra la interfaz (installer/ui/index.html), con el fondo
//! animado y el vidrio del navegador. El trabajo pesado (copiar, registrar)
//! corre en otro hilo y le avisa el avance a la pagina.
//!
//! - Sin argumentos: instala (o actualiza) lo que viene pegado al .exe (ver
//!   payload.rs). Todo en el usuario actual: no pide ser administrador.
//! - `--uninstall`: desinstala (es lo que ejecuta "Aplicaciones instaladas").
//!   Un programa no puede borrar su propio .exe mientras corre, asi que el
//!   desinstalador se copia a la carpeta temporal y sigue desde ahi.
//!
//! Para generar el instalador: tools/build_installer.ps1.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod install;
mod payload;
mod system;

use std::borrow::Cow;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};
use wry::http::header::{ACCEPT_RANGES, CACHE_CONTROL, CONTENT_RANGE, CONTENT_SECURITY_POLICY, CONTENT_TYPE, RANGE};
use wry::http::{Request, Response};
use wry::{WebContext, WebView, WebViewBuilder};

use install::{Options, EXE};
use payload::Payload;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const WIDTH: f64 = 880.0;
const HEIGHT: f64 = 560.0;
/// Sin procesos de consola visibles (cmd para borrar los temporales al salir).
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const NO_PAYLOAD: &str = "Este instalador no trae los archivos del navegador. Generalo con tools/build_installer.ps1.";

/// Fondos para elegir (los mismos ids que protocol::THEMES del navegador),
/// en su version chica y difuminada: sirven de vista previa y de fondo del
/// propio instalador.
struct Theme {
    id: &'static str,
    label: &'static str,
    bytes: &'static [u8],
    mime: &'static str,
}

const THEMES: &[Theme] = &[
    Theme {
        id: "sleepy-rainy-evening",
        label: "Lluvia nocturna",
        bytes: include_bytes!("../../assets/wallpapers/optimized/sleepy-rainy-evening.blur.mp4"),
        mime: "video/mp4",
    },
    Theme {
        id: "torii-carmesi",
        label: "Torii carmesi",
        bytes: include_bytes!("../../assets/wallpapers/optimized/torii-carmesi.blur.jpg"),
        mime: "image/jpeg",
    },
    Theme {
        id: "blindfolded-girl",
        label: "Petalos y luna",
        bytes: include_bytes!("../../assets/wallpapers/optimized/blindfolded-girl.blur.mp4"),
        mime: "video/mp4",
    },
    Theme {
        id: "wuthering-waves-chisa",
        label: "Wuthering Waves",
        bytes: include_bytes!("../../assets/wallpapers/optimized/wuthering-waves-chisa.blur.mp4"),
        mime: "video/mp4",
    },
];

/// Los mismos ids que storage::SEARCH_ENGINES del navegador.
const ENGINES: &[(&str, &str)] = &[
    ("google", "Google"),
    ("bing", "Bing"),
    ("duckduckgo", "DuckDuckGo"),
    ("brave", "Brave Search"),
    ("ecosia", "Ecosia"),
];

enum UserEvent {
    /// Mensaje de la pagina (llega por el bucle de eventos, fuera del
    /// callback de WebView2).
    Ipc(String),
    /// JS para la pagina (avance de un trabajo en otro hilo).
    Js(String),
    /// Termino la instalacion/desinstalacion: JS con el resultado.
    Finished(String),
}

#[derive(Clone)]
enum Mode {
    Install,
    Uninstall { dir: Option<PathBuf>, temp_copy: bool },
}

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Command {
    Ready,
    PickDir,
    Install(Options),
    Uninstall {
        #[serde(rename = "deleteData")]
        delete_data: bool,
    },
    /// "Cerrar Michi y continuar" (estaba abierto).
    CloseRunning,
    Launch,
    DefaultApps,
    Drag,
    Minimize,
    Close,
}

/// Trabajo esperando a que se cierre el navegador.
enum Job {
    Install(Options),
    Uninstall(PathBuf, bool),
}

impl Job {
    fn dir(&self) -> PathBuf {
        match self {
            Job::Install(opt) => opt.dir.clone(),
            Job::Uninstall(dir, _) => dir.clone(),
        }
    }
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    mode: Mode,
    payload: Option<Arc<Payload>>,
    context: WebContext,
    window: Option<Window>,
    webview: Option<WebView>,
    busy: bool,
    waiting: Option<Job>,
    /// Donde quedo instalado (para "Abrir Michi").
    installed_dir: Option<PathBuf>,
}

impl App {
    fn js(&self, script: &str) {
        if let Some(webview) = &self.webview {
            let _ = webview.evaluate_script(script);
        }
    }

    fn init_data(&self) -> serde_json::Value {
        let existing = system::installed();
        let current = install::current_settings();
        let default_dir = existing.as_ref().map(|(_, d)| d.clone()).unwrap_or_else(install::default_dir);
        let (mode, uninstall_dir) = match &self.mode {
            Mode::Install => ("install", None),
            Mode::Uninstall { dir, .. } => ("uninstall", dir.clone()),
        };
        json!({
            "mode": mode,
            "version": self.payload.as_ref().map_or(VERSION.to_string(), |p| p.index.version.clone()),
            "hasPayload": self.payload.is_some(),
            "sizeMb": self.payload.as_ref().map_or(0.0, |p| (p.total_bytes() as f64 / 1_048_576.0).round()),
            "defaultDir": default_dir,
            "existing": existing.map(|(version, dir)| json!({ "version": version, "dir": dir })),
            // Fondo y buscador que el usuario ya tiene: quedan marcados (y el
            // desinstalador usa ese fondo).
            "currentTheme": current.get("theme").and_then(|v| v.as_str()),
            "currentEngine": current.get("searchEngine").and_then(|v| v.as_str()),
            "themes": THEMES.iter().map(|t| json!({ "id": t.id, "label": t.label, "video": t.mime == "video/mp4" })).collect::<Vec<_>>(),
            "engines": ENGINES.iter().map(|(id, label)| json!({ "id": id, "label": label })).collect::<Vec<_>>(),
            "webview2": wry::webview_version().unwrap_or_default(),
            "uninstallDir": uninstall_dir,
        })
    }

    fn handle(&mut self, event_loop: &ActiveEventLoop, raw: &str) {
        let Ok(cmd) = serde_json::from_str::<Command>(raw) else { return };
        match cmd {
            Command::Ready => self.js(&format!("app.init({})", self.init_data())),
            Command::PickDir => {
                let proxy = self.proxy.clone();
                std::thread::spawn(move || {
                    let picked = rfd::FileDialog::new().set_title("Donde instalar Michi").pick_folder();
                    if let Some(dir) = picked {
                        let dir = install::normalize_dir(&dir);
                        let _ = proxy.send_event(UserEvent::Js(format!("app.dir({})", json!(dir))));
                    }
                });
            }
            Command::Install(mut opt) => {
                opt.dir = install::normalize_dir(&opt.dir);
                self.start(Job::Install(opt));
            }
            Command::Uninstall { delete_data } => {
                let Mode::Uninstall { dir: Some(dir), .. } = &self.mode else { return };
                self.start(Job::Uninstall(dir.clone(), delete_data));
            }
            Command::CloseRunning => {
                if let Some(job) = self.waiting.take() {
                    system::close_processes(&system::running_in(&job.dir()));
                    self.start(job);
                }
            }
            Command::Launch => {
                if let Some(dir) = &self.installed_dir {
                    let _ = std::process::Command::new(dir.join(EXE)).current_dir(dir).spawn();
                }
                event_loop.exit();
            }
            Command::DefaultApps => {
                let uri = format!("ms-settings:defaultapps?registeredAppUser={}", system::REGISTERED_NAME);
                let _ = std::process::Command::new("explorer").arg(uri).spawn();
            }
            Command::Drag => {
                if let Some(w) = &self.window {
                    let _ = w.drag_window();
                }
            }
            Command::Minimize => {
                if let Some(w) = &self.window {
                    w.set_minimized(true);
                }
            }
            Command::Close => {
                if !self.busy {
                    event_loop.exit();
                }
            }
        }
    }

    /// Arranca un trabajo en otro hilo; si el navegador esta abierto desde
    /// esa carpeta, primero le pide al usuario cerrarlo.
    fn start(&mut self, job: Job) {
        if self.busy {
            return;
        }
        if !system::running_in(&job.dir()).is_empty() {
            self.waiting = Some(job);
            self.js("app.running()");
            return;
        }
        self.busy = true;
        if let Job::Install(opt) = &job {
            self.installed_dir = Some(opt.dir.clone());
        }
        let proxy = self.proxy.clone();
        let payload = self.payload.clone();
        std::thread::spawn(move || {
            let progress = |fraction: f64, step: &str, detail: &str| {
                let _ = proxy.send_event(UserEvent::Js(format!(
                    "app.progress({fraction:.4}, {}, {})",
                    json!(step),
                    json!(detail)
                )));
            };
            let result = match &job {
                Job::Install(opt) => match &payload {
                    Some(payload) => install::install(payload, opt, &progress),
                    None => Err(NO_PAYLOAD.into()),
                },
                Job::Uninstall(dir, delete_data) => install::uninstall(dir, *delete_data, &progress),
            };
            let js = match result {
                Ok(()) => "app.done()".to_string(),
                Err(err) => format!("app.error({})", json!(err)),
            };
            let _ = proxy.send_event(UserEvent::Finished(js));
        });
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let title = match self.mode {
            Mode::Install => "Instalar Michi",
            Mode::Uninstall { .. } => "Desinstalar Michi",
        };
        let mut attrs = Window::default_attributes()
            .with_title(title)
            .with_inner_size(LogicalSize::new(WIDTH, HEIGHT))
            .with_resizable(false)
            .with_decorations(false)
            .with_window_icon(app_icon());
        if let Some(monitor) = event_loop.primary_monitor() {
            let scale = monitor.scale_factor();
            let (size, pos) = (monitor.size(), monitor.position());
            attrs = attrs.with_position(PhysicalPosition::new(
                pos.x + ((size.width as f64 - WIDTH * scale) / 2.0).max(0.0) as i32,
                pos.y + ((size.height as f64 - HEIGHT * scale) / 2.0).max(0.0) as i32,
            ));
        }
        let window = event_loop.create_window(attrs).expect("no se pudo crear la ventana");
        round_corners(&window);

        let proxy = self.proxy.clone();
        let webview = WebViewBuilder::new_with_web_context(&mut self.context)
            .with_background_color((12, 11, 22, 255))
            .with_custom_protocol("app".into(), |_id, request| serve(&request))
            .with_url("app://localhost/")
            // Solo su propia pagina: nunca carga nada de afuera.
            .with_navigation_handler(|url| url.starts_with("http://app.localhost/"))
            .with_ipc_handler(move |req: Request<String>| {
                if req.uri().host() == Some("app.localhost") {
                    let _ = proxy.send_event(UserEvent::Ipc(req.into_body()));
                }
            })
            .with_devtools(cfg!(debug_assertions))
            .build(&window)
            .expect("no se pudo crear la interfaz del instalador");
        self.window = Some(window);
        self.webview = Some(webview);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let WindowEvent::CloseRequested = event {
            if !self.busy {
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Ipc(raw) => self.handle(event_loop, &raw),
            UserEvent::Js(js) => self.js(&js),
            UserEvent::Finished(js) => {
                self.busy = false;
                self.js(&js);
            }
        }
    }
}

/// Esquinas redondeadas (Windows 11) en la ventana sin bordes.
fn round_corners(window: &Window) {
    use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
    let pref = DWMWCP_ROUND;
    unsafe {
        DwmSetWindowAttribute(
            h.hwnd.get() as _,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &pref as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&pref) as u32,
        );
    }
}

fn app_icon() -> Option<winit::window::Icon> {
    use winit::platform::windows::IconExtWindows;
    winit::window::Icon::from_resource(1, None).ok()
}

/// Las paginas del instalador (todo viene dentro del .exe).
fn serve(request: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = request.uri().path();
    let found: Option<(&'static [u8], &str)> = match path {
        "/" => Some((include_bytes!("../ui/index.html"), "text/html; charset=utf-8")),
        "/icon.png" => Some((include_bytes!("../../assets/icon/michi-256.png"), "image/png")),
        _ => path
            .strip_prefix("/fondos/")
            .and_then(|id| THEMES.iter().find(|t| t.id == id))
            .map(|t| (t.bytes, t.mime)),
    };
    let Some((body, mime)) = found else {
        return Response::builder().status(404).body(Cow::Borrowed(&b""[..])).unwrap();
    };
    let builder = Response::builder()
        .header(CONTENT_TYPE, mime)
        .header("X-Content-Type-Options", "nosniff")
        .header(CACHE_CONTROL, "no-store")
        .header(
            CONTENT_SECURITY_POLICY,
            "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'self'; \
             media-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        );
    // Los <video> piden por rangos y Chromium no reproduce sin un 206.
    let range = request
        .headers()
        .get(RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| v.split_once('-'))
        .and_then(|(a, b)| Some((a.trim().parse::<usize>().ok()?, b.trim().parse::<usize>().ok())));
    match range {
        Some((start, end)) if start < body.len() => {
            let end = end.unwrap_or(body.len() - 1).min(body.len() - 1).max(start);
            builder
                .status(206)
                .header(ACCEPT_RANGES, "bytes")
                .header(CONTENT_RANGE, format!("bytes {start}-{end}/{}", body.len()))
                .body(Cow::Borrowed(&body[start..=end]))
                .unwrap()
        }
        _ => builder.status(200).header(ACCEPT_RANGES, "bytes").body(Cow::Borrowed(body)).unwrap(),
    }
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// Desinstalando desde la propia carpeta del programa: se copia a la carpeta
/// temporal y sigue desde ahi (si no, no podria borrar su propio .exe).
/// Devuelve true si la copia quedo a cargo.
fn relocate(dir: &PathBuf) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    if !exe.starts_with(dir) {
        return false;
    }
    let temp = std::env::temp_dir().join(format!("michi-desinstalar-{}.exe", std::process::id()));
    if std::fs::copy(&exe, &temp).is_err() {
        return false;
    }
    std::process::Command::new(&temp)
        .arg("--uninstall")
        .arg("--dir")
        .arg(dir)
        .arg("--temp")
        .spawn()
        .is_ok()
}

/// Al salir: borra la carpeta temporal de WebView2 del instalador y, si es la
/// copia temporal del desinstalador, a si misma (cuando ya termino).
fn clean_up_later(webview_dir: &std::path::Path, delete_self: bool) {
    let mut cmd = format!("/C ping 127.0.0.1 -n 3 >nul & rmdir /s /q \"{}\"", webview_dir.display());
    if delete_self {
        // Solo si de verdad es la copia temporal que hizo relocate (nunca,
        // por ejemplo, el instalador original lanzado con --temp a mano).
        let copy = std::env::current_exe().ok().filter(|exe| {
            exe.parent() == Some(std::env::temp_dir().as_path())
                && exe.file_name().is_some_and(|n| n.to_string_lossy().starts_with("michi-desinstalar-"))
        });
        if let Some(exe) = copy {
            cmd.push_str(&format!(" & del /f /q \"{}\"", exe.display()));
        }
    }
    let _ = std::process::Command::new("cmd").raw_arg(cmd).creation_flags(CREATE_NO_WINDOW).spawn();
}

fn main() {
    system::harden_process();
    let args: Vec<String> = std::env::args().collect();
    let mode = if args.iter().any(|a| a == "--uninstall") {
        let dir = arg_value(&args, "--dir")
            .map(PathBuf::from)
            .or_else(|| system::installed().map(|(_, dir)| dir))
            .or_else(|| std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)));
        let temp_copy = args.iter().any(|a| a == "--temp");
        if let (false, Some(dir)) = (temp_copy, &dir) {
            if relocate(dir) {
                return;
            }
        }
        Mode::Uninstall { dir, temp_copy }
    } else {
        Mode::Install
    };

    if wry::webview_version().is_err() {
        let open = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Falta Microsoft Edge WebView2")
            .set_description(
                "Michi necesita Microsoft Edge WebView2 Runtime (viene con Windows 11 y con casi todos los \
                 Windows 10). Abrir la pagina de descarga de Microsoft?",
            )
            .set_buttons(rfd::MessageButtons::YesNo)
            .show()
            == rfd::MessageDialogResult::Yes;
        if open {
            let _ = std::process::Command::new("explorer")
                .arg("https://go.microsoft.com/fwlink/p/?LinkId=2124703")
                .spawn();
        }
        return;
    }

    // Carpeta de datos de WebView2 del instalador: en la temporal, no junto
    // al .exe (que suele estar en Descargas). Se borra al salir.
    let webview_dir = std::env::temp_dir().join(format!("MichiSetup-{}", std::process::id()));
    let delete_self = matches!(mode, Mode::Uninstall { temp_copy: true, .. });

    let event_loop = EventLoop::<UserEvent>::with_user_event().build().expect("no se pudo crear el event loop");
    let mut app = App {
        proxy: event_loop.create_proxy(),
        mode: mode.clone(),
        payload: matches!(mode, Mode::Install).then(Payload::open).flatten().map(Arc::new),
        context: WebContext::new(Some(webview_dir.clone())),
        window: None,
        webview: None,
        busy: false,
        waiting: None,
        installed_dir: None,
    };
    let _ = event_loop.run_app(&mut app);
    drop(app);
    clean_up_later(&webview_dir, delete_self);
}
