use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2Controller, ICoreWebView2Deferral, ICoreWebView2Environment, ICoreWebView2NewWindowRequestedEventArgs,
};
use winit::event_loop::EventLoopProxy;
use winit::window::Window;
use wry::WebView;

use crate::security::TabSecurity;
use crate::storage::{self, Bookmark, Settings};

/// Alto de la barra superior (px logicos).
pub const TOPBAR_H: f64 = 44.0;
/// Ancho de la franja "inactiva" de los paneles laterales (el grip siempre visible).
pub const EDGE_IDLE: f64 = 10.0;
/// Ancho del WebView de cada panel lateral (siempre; ver layout::position_panels).
pub const PANEL_W: f64 = 300.0;
/// Tamano del menu "..." (ver menu.rs y ui/menu.html).
pub const MENU_W: f64 = 290.0;
pub const MENU_H: f64 = 440.0;

/// Tamano minimo de la ventana (ver with_min_inner_size en main.rs). Tambien
/// se usa en sync::push_wallpaper como umbral para descartar un tamano
/// reportado por winit que nunca podria ser real (ver el comentario ahi).
pub const MIN_WINDOW_W: f64 = 480.0;
pub const MIN_WINDOW_H: f64 = 360.0;

/// Pagina que abren las pestanas nuevas: nuestra propia "nueva pestana"
/// (fondo animado + barra de busqueda), servida via el protocolo
/// personalizado "app://" (ver src/protocol.rs).
pub const HOME_URL: &str = "app://localhost/";
/// Pagina de ajustes (y extensiones, en #extensions).
pub const SETTINGS_URL: &str = "app://localhost/settings";
/// Aviso "este sitio no admite conexiones seguras" (ver security::https_first).
/// Con la forma http://app.localhost y no app://: se abre con load_url en una
/// pestana que ya existe, y solo with_url (al crearla) traduce el esquema
/// app:// (ver is_own_page).
pub const INSECURE_URL: &str = "http://app.localhost/insecure";
/// Pestana nueva, para cargarla con load_url (ver INSECURE_URL).
pub const HOME_LOAD_URL: &str = "http://app.localhost/";

/// Si una pestana sigue mostrando nuestra propia pagina servida por el
/// protocolo "app://" (ver src/protocol.rs). Hace falta comparar contra dos
/// formas porque en Windows WebView2 no soporta esquemas de URL realmente
/// personalizados para el contenido de la pagina: internamente sirve el
/// protocolo como el host virtual "http://app.localhost/", asi que una vez
/// que la pestana termina de cargar, tab.url pasa a tener esa forma en vez
/// de "app://localhost/" (que solo se ve en el instante entre crear la
/// pestana y que WebView2 reporte la primera navegacion).
///
/// Compara el ORIGEN exacto, no "contiene app.localhost": de eso depende que
/// un sitio cualquiera (ej. "https://malo.com/?app.localhost") no pueda usar
/// los comandos de ajustes y extensiones (ver ipc::dispatch_from_content).
pub fn is_own_page(url: &str) -> bool {
    ["app://localhost", "http://app.localhost", "https://app.localhost"]
        .iter()
        .any(|origin| url.strip_prefix(origin).is_some_and(|rest| rest.is_empty() || rest.starts_with(['/', '?', '#'])))
}

/// Ruta de una pagina propia ("/" para la pestana nueva, "/settings"...), o
/// None si no es una pagina propia.
pub fn own_page_path(url: &str) -> Option<&str> {
    if !is_own_page(url) {
        return None;
    }
    let after_scheme = url.split_once("://")?.1;
    let path = after_scheme.find('/').map_or("/", |i| &after_scheme[i..]);
    Some(path.split(['?', '#']).next().unwrap_or("/"))
}

/// Una pestana = un WebView real e independiente (motor WebView2 completo, no un iframe).
pub struct Tab {
    pub id: u64,
    pub webview: WebView,
    pub title: String,
    pub url: String,
    pub loading: bool,
    /// Icono del sitio (favicon) como data URL para el panel de pestanas; None
    /// = todavia no lo tiene (el panel muestra un punto). Ver tabs::attach_favicon.
    pub favicon: Option<String>,
    /// HTTPS primero, lo bloqueado en la pagina actual... (ver security.rs).
    pub sec: TabSecurity,
    /// La pagina tiene un <video> reproduciendose (lo avisa ui/content_init.js,
    /// ver ipc::media_message) o suena (ver audio::attach). Con eso la
    /// interfaz congela su fondo animado (ver sync::push_media).
    pub playing: Playing,
}

/// Que esta reproduciendo una pestana.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub struct Playing {
    pub video: bool,
    pub audio: bool,
}

impl Playing {
    pub fn any(self) -> bool {
        self.video || self.audio
    }
}

/// Eventos propios que despiertan el bucle de winit. Hace falta porque los
/// comandos IPC de las paginas llegan por el bucle de mensajes de WebView2,
/// no como eventos de winit: sin un aviso explicito, el bucle seguiria
/// dormido (ControlFlow::Wait) y el vigilante del cursor de los paneles
/// (ver panels::tick) nunca arrancaria. Los demas traen el resultado de un
/// trabajo hecho en otro hilo (descargar una extension, un dialogo nativo).
#[derive(Debug, Clone)]
pub enum UserEvent {
    /// "Hay trabajo pendiente" (ver panels::tick y AppState::actions).
    Wake,
    /// Una extension ya descargada/desempaquetada en esta carpeta, lista para
    /// registrarla en WebView2 (ver extensions::install_dir).
    ExtensionUnpacked(PathBuf),
    /// Algo fallo en un trabajo de fondo; se muestra en ajustes/barra.
    ExtensionFailed(String),
    /// Carpeta elegida en el dialogo de ajustes de descargas.
    DownloadsDirPicked(PathBuf),
    /// Datos leidos de otro navegador, listos para volcar (ver import.rs). Van
    /// en un Box porque es un dato grande y UserEvent se mueve por el bucle.
    ImportReady(Box<crate::import::ImportData>),
    /// Fallo la importacion desde otro navegador.
    ImportFailed(String),
    /// Otro michi.exe (un enlace o un .html abierto desde Windows) le paso lo
    /// pedido a este, que es el principal: abrirlo en una pestana (None = solo
    /// traer la ventana al frente). Ver single_instance.rs.
    OpenExternal(Option<String>),
    /// Sugerencias del buscador para lo escrito en la barra (ver suggest.rs).
    SearchSuggestions { seq: u64, query: String, list: Vec<String> },
}

/// Algo que hay que hacer en la proxima vuelta del bucle de eventos (ver
/// about_to_wait en main.rs y actions.rs) y no en el momento: casi todo
/// llega desde callbacks de WebView2 (atajos de teclado, IPC), y ahi adentro
/// crear o destruir un WebView se cuelga o destruye el propio WebView que
/// esta ejecutando el callback (ver AppState::request_tab).
#[derive(Debug, Clone)]
pub enum Action {
    CloseActiveTab,
    ReopenClosedTab,
    /// +1 / -1: siguiente/anterior pestana (con vuelta).
    CycleTab(i32),
    /// Ir a la pestana N (0 = la primera; negativo = la ultima): Ctrl+1..9.
    GoToTab(i32),
    FocusAddressBar,
    ToggleBookmark,
    /// Abre el panel izquierdo (historial, descargas, marcadores).
    ShowLibrary,
    /// Abre (o reusa) la pestana de ajustes en esa seccion ("" = la primera).
    OpenSettings(String),
    Print,
    DevTools,
    /// Zoom de la pestana activa: +1 / -1 un paso, 0 = volver a 100%.
    Zoom(i32),
    ToggleMenu,
    CloseMenu,
    /// Abre/cierra el popup de una extension (id, borde derecho de su icono).
    /// Diferido como todo lo que crea un WebView (ver request_tab).
    TogglePopup(String, f64),
    /// Cierra el popup de extension (ver popup.rs).
    ClosePopup,
    /// Lleva una pestana (por id) a una URL: los desvios que decide
    /// security.rs (intentar por HTTPS, volver a HTTP, la pagina de aviso).
    LoadUrl(u64, String),
    /// Cierra una pestana por id: la propia pagina llamo a window.close().
    CloseTab(u64),
    /// La pagina de una pestana (id) entro o salio de pantalla completa
    /// (ver fullscreen.rs).
    Fullscreen(u64, bool),
}

/// Una ventana que abrio una pagina con window.open(), esperando su pestana.
/// La pagina queda en pausa (deferral) hasta que la pestana existe y se la
/// entregamos al motor como la ventana nueva (SetNewWindow): asi conserva su
/// window.opener, como en Chrome. Los inicios de sesion en ventana emergente
/// ("Continuar con Google" en claude.ai, por ejemplo) devuelven el resultado
/// a la pagina que los abrio por ahi; sin opener se quedaban en blanco.
/// La pestana se crea en la proxima vuelta del bucle (ver request_tab).
pub struct PendingWindow {
    pub url: String,
    pub args: ICoreWebView2NewWindowRequestedEventArgs,
    pub deferral: ICoreWebView2Deferral,
    /// El de la pagina que la abre: WebView2 exige el mismo.
    pub environment: ICoreWebView2Environment,
}

/// Popup de extension abierto (ver popup.rs).
pub struct ExtPopup {
    pub id: String,
    pub webview: WebView,
    /// Borde derecho del icono que lo abrio (px logicos): se alinea ahi.
    pub right: f64,
    /// Si ya llego el tamano de su contenido (hasta entonces, oculto).
    pub sized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Una animacion de apertura/cierre en curso de un panel lateral (ver panels.rs).
pub struct PanelAnim {
    pub from: f64,
    pub to: f64,
    pub start: Instant,
    pub duration: Duration,
}

/// Estado de un panel lateral (ver panels.rs): si deberia estar abierto
/// (`expanded`, el objetivo) y cuanto esta abierto AHORA (`progress`: 0 =
/// solo asoma la franja de EDGE_IDLE, 1 = todo adentro), que durante una
/// animacion va de un extremo al otro.
pub struct PanelState {
    pub expanded: bool,
    pub progress: f64,
    pub anim: Option<PanelAnim>,
    /// Desde cuando el cursor esta fuera del panel abierto (ver panels::watch_cursor).
    pub outside_since: Option<Instant>,
    /// Abierto desde un atajo o el menu, no acercando el cursor: no se cierra
    /// solo por tener el cursor afuera (ver panels::open_held).
    pub held: bool,
}

impl PanelState {
    fn new() -> Self {
        Self { expanded: false, progress: 0.0, anim: None, outside_since: None, held: false }
    }
}

#[derive(Clone, serde::Serialize)]
pub struct DownloadEntry {
    pub id: u64,
    pub file_name: String,
    /// "downloading" | "paused" | "completed" | "failed" | "cancelled" |
    /// "blocked" (SmartScreen u otro control de seguridad lo freno)
    pub status: String,
    pub path: Option<PathBuf>,
    pub received: u64,
    /// None si el servidor no dice el tamano.
    pub total: Option<u64>,
    /// Tipo de archivo peligroso (.exe, .bat...) esperando que el usuario
    /// decida conservarlo o descartarlo (ver downloads::on_security_check).
    pub dangerous: bool,
    /// Se descarga por HTTP (sin cifrar): alguien en la red pudo cambiarlo.
    pub insecure_source: bool,
}

pub use storage::HistoryEntry;

/// Estado central de toda la aplicacion. Vive detras de un Rc<RefCell<_>> para que
/// los closures de IPC/navegacion de cada WebView puedan compartirlo y mutarlo.
pub struct AppState {
    pub window: Option<Rc<Window>>,
    pub topbar: Option<WebView>,
    pub left_panel: Option<WebView>,
    pub right_panel: Option<WebView>,
    /// Menu "..." (ver menu.rs): WebView propio, oculto mientras esta cerrado.
    pub menu: Option<WebView>,
    pub menu_open: bool,
    /// Cuando se cerro el menu por ultima vez (ver menu::toggle).
    pub menu_closed_at: Option<Instant>,
    /// Desplegable de sugerencias de la barra de direcciones (ver suggest.rs).
    pub suggest: crate::suggest::Suggest,
    pub ext_popup: Option<ExtPopup>,
    /// Que popup se cerro y cuando (ver popup::toggle).
    pub ext_popup_closed: Option<(String, Instant)>,

    pub tabs: Vec<Tab>,
    pub active: usize,
    /// Pestana cuya pagina esta en pantalla completa (un video), si hay una.
    /// Ver fullscreen.rs.
    pub fullscreen: Option<u64>,

    pub left: PanelState,
    pub right: PanelState,

    /// Para despertar el bucle de eventos desde un callback de WebView2.
    pub proxy: Option<EventLoopProxy<UserEvent>>,
    /// Pestanas pedidas (URL) que todavia no se crearon. Ver request_tab.
    pub pending_tabs: Vec<String>,
    /// window.open() de una pagina, esperando su pestana (ver PendingWindow).
    pub pending_windows: Vec<PendingWindow>,
    /// Acciones diferidas a la proxima vuelta del bucle (ver Action).
    pub actions: Vec<Action>,

    pub downloads: Vec<DownloadEntry>,
    pub history: Vec<HistoryEntry>,
    pub bookmarks: Vec<Bookmark>,
    /// URLs de las pestanas cerradas, para Ctrl+Shift+T (la ultima al final).
    pub closed_tabs: Vec<String>,

    /// Ajustes del usuario (persistidos, ver storage.rs). El tema de fondo
    /// activo vive aca (settings.theme), compartido por la barra superior,
    /// los dos paneles y las paginas propias (ver sync::push_wallpaper).
    pub settings: Settings,

    /// Mensaje de estado de la ultima instalacion de extension ("" = nada),
    /// que muestran la barra y la pagina de extensiones.
    pub extension_status: String,

    /// Perfiles de otros navegadores detectados para importar (ver import.rs).
    /// Se llena al entrar a ajustes; se guarda para resolver `key` al confirmar.
    pub import_sources: Vec<crate::import::Source>,

    /// Hosts que en esta sesion se abren por HTTP: no tienen HTTPS, o el
    /// usuario acepto el aviso (ver security::https_first).
    pub http_allowed: HashSet<String>,
    /// Hosts cuya pagina mostro un certificado invalido en esta sesion: la
    /// barra los marca como no seguros aunque sean https://.
    pub cert_errors: HashSet<String>,
    /// Pieza de la interfaz (barra o panel) que tenia el foco del teclado, para
    /// devolvercelo al volver a la ventana (ver shortcuts::restore_focus).
    /// None = la pestana al frente.
    pub focus_owner: Option<ICoreWebView2Controller>,
    /// Hasta cuando ignorar los avisos de foco: al volver a la ventana wry le
    /// da el foco a TODOS los WebView, uno atras de otro (ver restore_focus).
    pub focus_settle_until: Option<Instant>,
    /// El fondo animado de la interfaz esta congelado porque la pestana al
    /// frente reproduce video o audio (ver sync::push_media).
    pub wallpaper_frozen: bool,
    /// Cerrando con "borrar datos al cerrar": hasta cuando esperar a que
    /// termine el borrado antes de salir igual (ver security::quit).
    pub quit_deadline: Option<Instant>,

    next_id: u64,
}

/// Tema por defecto al arrancar la app por primera vez.
pub const DEFAULT_THEME: &str = "sleepy-rainy-evening";

impl AppState {
    pub fn new() -> Self {
        Self {
            window: None,
            topbar: None,
            left_panel: None,
            right_panel: None,
            menu: None,
            menu_open: false,
            menu_closed_at: None,
            suggest: crate::suggest::Suggest::load(),
            ext_popup: None,
            ext_popup_closed: None,
            tabs: Vec::new(),
            active: 0,
            fullscreen: None,
            left: PanelState::new(),
            right: PanelState::new(),
            proxy: None,
            pending_tabs: Vec::new(),
            pending_windows: Vec::new(),
            actions: Vec::new(),
            downloads: Vec::new(),
            history: storage::load(storage::HISTORY_FILE),
            bookmarks: storage::load(storage::BOOKMARKS_FILE),
            closed_tabs: Vec::new(),
            settings: storage::load(storage::SETTINGS_FILE),
            extension_status: String::new(),
            import_sources: Vec::new(),
            http_allowed: HashSet::new(),
            cert_errors: HashSet::new(),
            focus_owner: None,
            focus_settle_until: None,
            wallpaper_frozen: false,
            quit_deadline: None,
            next_id: 1,
        }
    }

    pub fn panel(&self, side: Side) -> &PanelState {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }

    pub fn panel_mut(&mut self, side: Side) -> &mut PanelState {
        match side {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
        }
    }

    /// Despierta el bucle de eventos para que pase por panels::tick.
    pub fn wake(&self) {
        self.send(UserEvent::Wake);
    }

    pub fn send(&self, event: UserEvent) {
        if let Some(proxy) = &self.proxy {
            let _ = proxy.send_event(event);
        }
    }

    /// Pide abrir una pestana nueva; se crea en la proxima vuelta del bucle
    /// de eventos (ver about_to_wait en main.rs), nunca en el momento.
    ///
    /// Todos los pedidos de pestana nueva llegan desde un callback de
    /// WebView2 (el IPC del boton "+", un enlace target=_blank...), y crear
    /// un WebView ahi adentro se cuelga para siempre: wry espera a que el
    /// WebView nuevo este listo con un bucle de mensajes anidado, pero
    /// WebView2 no entrega ese aviso hasta que termina el callback en curso.
    /// La app no se congelaba del todo (seguia bombeando mensajes), pero la
    /// pestana nueva nunca terminaba de crearse y nada volvia a actualizarse.
    pub fn request_tab(&mut self, url: &str) {
        self.pending_tabs.push(url.to_string());
        self.wake();
    }

    /// Encola una accion para la proxima vuelta del bucle (ver Action).
    pub fn request(&mut self, action: Action) {
        self.actions.push(action);
        self.wake();
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub fn save_settings(&self) {
        storage::save(storage::SETTINGS_FILE, &self.settings);
    }

    pub fn save_bookmarks(&self) {
        storage::save(storage::BOOKMARKS_FILE, &self.bookmarks);
    }

    pub fn save_history(&self) {
        storage::save(storage::HISTORY_FILE, &self.history);
    }

    /// Guarda las pestanas abiertas (para "al iniciar: restaurar"). Nunca con
    /// "borrar datos al cerrar": la sesion es justamente lo que no tiene que
    /// quedar en el disco.
    pub fn save_session(&self) {
        if self.settings.clear_on_exit {
            return;
        }
        let session = storage::Session {
            urls: self.tabs.iter().map(|t| t.url.clone()).collect(),
            active: self.active,
        };
        storage::save(storage::SESSION_FILE, &session);
    }

    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

pub type Shared = Rc<RefCell<AppState>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_pages_match_the_exact_origin_only() {
        assert!(is_own_page("app://localhost/"));
        assert!(is_own_page("http://app.localhost/settings#extensions"));
        assert!(is_own_page("http://app.localhost"));
        assert!(!is_own_page("https://malo.com/?app.localhost"));
        assert!(!is_own_page("http://app.localhost.malo.com/"));
        assert!(!is_own_page("https://example.com/app://localhost/"));

        assert_eq!(own_page_path("http://app.localhost/"), Some("/"));
        assert_eq!(own_page_path("http://app.localhost"), Some("/"));
        assert_eq!(own_page_path("app://localhost/settings#about"), Some("/settings"));
        assert_eq!(own_page_path("https://google.com/settings"), None);
    }
}
