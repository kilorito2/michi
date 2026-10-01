// Sin ventana de consola en release (la que se instala); en debug queda,
// para ver los mensajes de [seguridad], [ipc]... (los errores graves van
// igual a crash.log).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod audio;
mod chrome;
mod crx;
mod downloads;
mod extensions;
mod import;
mod ipc;
mod layout;
mod menu;
mod native;
mod panels;
mod permissions;
mod popup;
mod protocol;
mod security;
mod shortcuts;
mod single_instance;
mod state;
mod storage;
mod suggest;
mod sync;
mod tabs;
mod win;

use std::cell::RefCell;
use std::rc::Rc;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use state::{AppState, Shared, UserEvent, HOME_URL, MIN_WINDOW_H, MIN_WINDOW_W};

struct App {
    state: Shared,
    /// Ctrl/Mayus/Alt apretados, segun winit (ver shortcuts::handle_winit_key).
    modifiers: winit::keyboard::ModifiersState,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let mut st = AppState::new();
        st.proxy = Some(proxy);
        Self { state: Rc::new(RefCell::new(st)), modifiers: Default::default() }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Wait);

        if self.state.borrow().window.is_some() {
            return; // ya inicializado
        }

        let attrs = Window::default_attributes()
            .with_title("Michi")
            .with_inner_size({
                let (w, h) = win::restored_size(event_loop);
                LogicalSize::new(w, h)
            })
            // Centrada en el monitor cuando se la restaura. Tiene que ir en los
            // atributos: moverla despues de crearla le sacaba el maximizado.
            .with_position(win::restored_position(event_loop))
            .with_min_inner_size(LogicalSize::new(MIN_WINDOW_W, MIN_WINDOW_H))
            .with_maximized(true)
            .with_decorations(false)
            .with_transparent(true)
            .with_blur(true);
        let attrs = with_app_icon(attrs);

        let window = event_loop.create_window(attrs).expect("no se pudo crear la ventana");
        win::fit_maximize_to_work_area(&window);
        win::remember_main_window(&window);
        self.state.borrow_mut().window = Some(Rc::new(window));

        // Orden importante: primero el contenido (pestanas iniciales), luego
        // el "chrome" (barra + paneles + menu), que asi queda por encima en el
        // z-order y puede taparlo cuando un panel lateral se expande.
        open_startup_tabs(&self.state);

        let topbar = chrome::build_topbar(&self.state);
        let left = chrome::build_left_panel(&self.state);
        let right = chrome::build_right_panel(&self.state);
        let menu = menu::build(&self.state);
        let suggestions = suggest::build(&self.state);
        for wv in [&topbar, &left, &right, &menu, &suggestions] {
            security::harden(&self.state, wv);
            shortcuts::attach(&self.state, wv);
        }
        {
            let mut st = self.state.borrow_mut();
            st.topbar = Some(topbar);
            st.left_panel = Some(left);
            st.right_panel = Some(right);
            st.menu = Some(menu);
            st.suggest.view = Some(suggestions);
        }
        // Ajustes del perfil (necesitan algun WebView ya creado).
        security::apply_tracking_prevention(&self.state);
        security::clear_leftovers(&self.state);

        layout::relayout(&self.state);
        sync::push_tabs(&self.state);
        sync::push_active_tab(&self.state);
        sync::push_downloads(&self.state);
        sync::push_history(&self.state);
        sync::push_bookmarks(&self.state);
        sync::push_maximized(&self.state);
        sync::push_wallpaper(&self.state);
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => security::quit(&self.state),
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                shortcuts::handle_winit_key(&self.state, &event, self.modifiers)
            }
            WindowEvent::Resized(_) => {
                popup::close(&self.state);
                suggest::close(&self.state);
                layout::relayout(&self.state);
                sync::push_maximized(&self.state);
                sync::push_wallpaper(&self.state);
            }
            _ => {}
        }
    }

    // Wake no trae datos: solo sirve para que el bucle se despierte y pase
    // por about_to_wait, que es donde se hace el trabajo. Los demas traen el
    // resultado de un trabajo hecho en otro hilo.
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Wake => {}
            UserEvent::ExtensionUnpacked(dir) => extensions::install_dir(&self.state, dir),
            UserEvent::ExtensionFailed(message) => {
                self.state.borrow_mut().extension_status = message;
                sync::push_extension_status(&self.state);
            }
            UserEvent::DownloadsDirPicked(dir) => ipc::downloads_dir_picked(&self.state, dir),
            UserEvent::ImportReady(data) => import::apply(&self.state, *data),
            UserEvent::ImportFailed(message) => sync::push_toast(&self.state, &message),
            UserEvent::SearchSuggestions { seq, query, list } => suggest::on_engine(&self.state, seq, query, list),
            UserEvent::OpenExternal(url) => {
                win::bring_to_front();
                if let Some(url) = url.filter(|u| single_instance::is_external_target(u)) {
                    self.state.borrow_mut().request_tab(&url);
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Pestanas pedidas desde callbacks de WebView2 (ver AppState::request_tab).
        // De a UNA por vuelta: crear un WebView bloquea el hilo ~100-150ms, y
        // con varios pedidos juntos (clics seguidos en "+") crearlos todos de
        // una congelaba la ventana sin repintar. Entre una y otra el bucle
        // vuelve a procesar eventos (y a dibujar).
        let next_tab = {
            let mut st = self.state.borrow_mut();
            (!st.pending_tabs.is_empty()).then(|| st.pending_tabs.remove(0))
        };
        if let Some(url) = next_tab {
            tabs::open_tab(&self.state, &url);
            if !self.state.borrow().pending_tabs.is_empty() {
                self.state.borrow().wake();
            }
        }
        // Intentos por HTTPS demorados (ver security::tick): antes de las
        // acciones, asi su vuelta a HTTP sale en esta misma pasada.
        let https = security::tick(&self.state);
        // Atajos de teclado y menu (ver state::Action).
        actions::run_pending(&self.state);

        // Animacion de los paneles laterales + vigilantes del cursor (paneles
        // y menu): mientras haya algo de eso en curso, pedimos que nos
        // despierten a tiempo para el proximo frame; si no, a dormir hasta el
        // proximo evento. Lo mismo para los plazos de seguridad (HTTPS,
        // borrar datos al cerrar).
        let panels = panels::tick(&self.state);
        let menu = menu::watch(&self.state);
        let popup = popup::watch(&self.state);
        let quit = security::quit_deadline(&self.state);
        match panels.into_iter().chain(menu).chain(popup).chain(https).chain(quit).min() {
            Some(deadline) => event_loop.set_control_flow(ControlFlow::WaitUntil(deadline)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

/// Icono de la ventana (titulo, Alt+Tab) y de la barra de tareas: el mismo
/// que lleva incrustado el .exe (recurso 1, ver build.rs).
#[cfg(windows)]
fn with_app_icon(attrs: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    use winit::platform::windows::{IconExtWindows, WindowAttributesExtWindows};
    use winit::window::Icon;
    let small = Icon::from_resource(1, None).ok();
    let big = Icon::from_resource(1, Some(winit::dpi::PhysicalSize::new(256, 256))).ok();
    attrs.with_window_icon(small).with_taskbar_icon(big)
}

#[cfg(not(windows))]
fn with_app_icon(attrs: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    attrs
}

/// "Al iniciar" (ajustes): una pestana nueva, o las de la ultima sesion
/// (nunca con "borrar datos al cerrar": no deberia haber sesion guardada).
fn open_startup_tabs(state: &Shared) {
    let restore = {
        let st = state.borrow();
        st.settings.startup == "restore" && !st.settings.clear_on_exit
    };
    let session: storage::Session = if restore { storage::load(storage::SESSION_FILE) } else { Default::default() };
    let requested = url_from_args();
    if session.urls.is_empty() {
        tabs::open_tab(state, requested.as_deref().unwrap_or(HOME_URL));
        return;
    }
    for url in &session.urls {
        tabs::open_tab(state, url);
    }
    // Lo pedido al abrir (un enlace de otra aplicacion) queda al frente.
    if let Some(url) = requested {
        tabs::open_tab(state, &url);
        return;
    }
    let last = session.urls.len() - 1;
    state.borrow_mut().active = session.active.min(last);
    tabs::activated(state);
}

/// Direccion o archivo pasado al abrir el navegador: es lo que hace Windows
/// cuando Michi es el navegador predeterminado y se toca un enlace o se
/// abre un .html (ver el registro que hace el instalador). Solo web y
/// archivos locales: nunca javascript:, data: ni nada que no sea navegable.
fn url_from_args() -> Option<String> {
    let arg = std::env::args().nth(1)?;
    let lower = arg.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("file:///") {
        return Some(arg);
    }
    let path = std::path::Path::new(&arg);
    if !path.is_absolute() || !path.is_file() {
        return None;
    }
    let mut url = String::from("file:///");
    for c in path.to_string_lossy().chars() {
        match c {
            '\\' => url.push('/'),
            ' ' => url.push_str("%20"),
            '#' => url.push_str("%23"),
            '%' => url.push_str("%25"),
            '?' => url.push_str("%3F"),
            c => url.push(c),
        }
    }
    Some(url)
}

/// Si algo entra en panico (un bug), deja el detalle en crash.log dentro de
/// la carpeta de datos antes de cerrarse: sin consola, si no, el navegador
/// solo "desaparece" sin decir por que.
fn install_crash_log() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        let dir = storage::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("crash.log")) {
            let when = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let _ = writeln!(f, "[{when}] {info}\n{}\n", std::backtrace::Backtrace::force_capture());
        }
        default_hook(info);
    }));
}

fn main() {
    // Antes que nada: que ninguna DLL se cargue desde un lugar inseguro.
    security::harden_process();
    // Si ya hay un Michi abierto, el enlace o archivo pedido va a una pestana
    // de ese y este proceso termina aca, sin abrir otra ventana.
    if !single_instance::claim(url_from_args().as_deref()) {
        return;
    }
    // Antes de leer ningun archivo ni crear ningun WebView.
    storage::migrate_old_data();
    storage::migrate_old_profile();
    install_crash_log();
    std::thread::spawn(extensions::clean_staging);
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("no se pudo crear el event loop");
    single_instance::listen(event_loop.create_proxy());
    let mut app = App::new(event_loop.create_proxy());
    event_loop.run_app(&mut app).expect("fallo el bucle de eventos");
}
