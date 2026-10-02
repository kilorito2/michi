//! Creacion y cierre de pestanas. Cada pestana es un WebView real e
//! independiente (WebView2), asi que funciona con cualquier sitio, incluidos
//! los que bloquean ser mostrados dentro de un iframe.

use wry::dpi::{LogicalPosition, LogicalSize};
use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2, ICoreWebView2Environment};
use wry::{PageLoadEvent, Rect, WebViewBuilderExtWindows, WebViewExtWindows};

use crate::state::{is_own_page, HistoryEntry, PendingWindow, Shared, Tab, EDGE_IDLE, HOME_URL, TOPBAR_H};
use crate::storage::HISTORY_MAX;
use crate::{downloads, layout, permissions, security, shortcuts, sync};

/// Una pagina se considera "nueva pestana" (no un sitio real que el usuario
/// visito) si su URL corresponde a nuestro protocolo "app://" (ver
/// state::is_own_page) o todavia no navego a ningun lado. No se guarda en el
/// historial ni se muestra su URL interna en la barra de direcciones.
fn is_new_tab_url(url: &str) -> bool {
    is_own_page(url) || url == "about:blank"
}

/// Si tiene sentido guardar la pagina como marcador (no nuestras paginas
/// internas ni about:blank).
pub fn is_bookmarkable(url: &str) -> bool {
    !is_new_tab_url(url) && !url.is_empty()
}

pub fn open_tab(state: &Shared, url: &str) {
    let _ = create_tab(state, url, None);
}

/// window.open() de una pagina (ver state::PendingWindow): la pestana nace
/// sin navegar, en el entorno de la pagina que la abre, y se la entregamos
/// al motor como la ventana nueva; el motor la lleva a `url`.
pub fn open_window(state: &Shared, w: PendingWindow) {
    let core = create_tab(state, &w.url, Some(w.environment));
    unsafe {
        // Sin pestana, window.open() devuelve null (como con un bloqueador).
        if let Some(core) = &core {
            let _ = w.args.SetNewWindow(core);
        }
        let _ = w.args.SetHandled(true);
        let _ = w.deferral.Complete();
    }
}

/// Crea la pestana y la deja al frente. Con `environment` es una ventana de
/// window.open(): no navega (eso lo hace el motor despues de SetNewWindow).
fn create_tab(state: &Shared, url: &str, environment: Option<ICoreWebView2Environment>) -> Option<ICoreWebView2> {
    let id = state.borrow_mut().alloc_id();
    let is_window = environment.is_some();

    // Clonamos el Rc<Window> y soltamos el prestamo de `state` de inmediato:
    // construir el WebView puede disparar callbacks (on_page_load, etc.) de
    // forma sincronica, y esos callbacks tambien necesitan pedir prestado
    // `state`, asi que no podemos mantener un borrow() vivo durante el build.
    let (window, zoom) = {
        let st = state.borrow();
        (
            st.window.clone().expect("la ventana debe existir antes de crear pestanas"),
            st.settings.default_zoom,
        )
    };

    let webview = {
        // Bounds provisorios: layout::relayout(state), llamado al final de esta
        // funcion, los recalcula de inmediato segun el estado real de los
        // paneles laterales (expandidos o no). Estos solo importan por el
        // instante entre la creacion del WebView y esa primera llamada.
        let size = window.inner_size().to_logical::<f64>(window.scale_factor());

        let state_title = state.clone();
        let state_load = state.clone();
        let state_ipc = state.clone();

        // Navegacion, ventanas nuevas, certificados, permisos...: ver
        // security::attach_tab y permissions::attach, que se enganchan
        // apenas se crea (antes del primer evento de la navegacion inicial).
        let builder = match environment {
            Some(env) => security::app_builder().with_environment(env),
            None => security::app_builder().with_url(url),
        };
        builder
            // Nace oculta: un WebView nuevo queda arriba de todo en el orden de
            // apilamiento, y hasta que relayout (en `activated`) lo ubica y
            // vuelve a subir la interfaz, tapaba la barra y los paneles — con
            // varios "+" seguidos, la barra "desaparecia" un momento.
            .with_visible(false)
            // F12 / Ctrl+Shift+I y "Herramientas de desarrollador" del menu.
            .with_devtools(true)
            .with_initialization_script(include_str!("../ui/content_init.js"))
            // Restringido a nuestras propias paginas app:// (pestana nueva y
            // ajustes): ver ipc::dispatch_from_content. Nunca los comandos de
            // ipc::dispatch (ese es solo para la barra, los paneles y el menu).
            .with_ipc_handler(move |req: wry::http::Request<String>| {
                crate::ipc::dispatch_from_content(&state_ipc, id, &req.uri().to_string(), req.body());
            })
            .with_bounds(Rect {
                position: LogicalPosition::new(EDGE_IDLE, TOPBAR_H).into(),
                size: LogicalSize::new(
                    (size.width - 2.0 * EDGE_IDLE).max(0.0),
                    (size.height - TOPBAR_H).max(0.0),
                )
                .into(),
            })
            .with_document_title_changed_handler(move |title| {
                {
                    let mut st = state_title.borrow_mut();
                    if let Some(tab) = st.tabs.iter_mut().find(|t| t.id == id) {
                        tab.title = title.clone();
                    }
                    // El titulo suele llegar despues de "cargo": se lo
                    // actualizamos a la entrada de historial recien agregada.
                    let url = st.tabs.iter().find(|t| t.id == id).map(|t| t.url.clone());
                    if let (Some(url), Some(last)) = (url, st.history.last_mut()) {
                        if last.url == url && last.title != title {
                            last.title = title;
                            st.save_history();
                        }
                    }
                }
                sync::push_tabs(&state_title);
                sync::push_active_tab(&state_title);
                sync::push_history(&state_title);
            })
            .with_on_page_load_handler(move |event, page_url| {
                let (title_snapshot, finished, error_page) = {
                    let mut st = state_load.borrow_mut();
                    if let Some(tab) = st.tabs.iter_mut().find(|t| t.id == id) {
                        tab.url = page_url.clone();
                        tab.loading = matches!(event, PageLoadEvent::Started);
                    }
                    let tab = st.tabs.iter().find(|t| t.id == id);
                    let title = tab.map(|t| t.title.clone()).unwrap_or_default();
                    let error_page = tab.is_some_and(|t| t.sec.error_page);
                    (title, matches!(event, PageLoadEvent::Finished), error_page)
                };

                if finished {
                    let mut st = state_load.borrow_mut();
                    // Ni paginas propias, ni las internas de una extension
                    // (su popup u opciones abiertos en una pestana), ni las
                    // de error (un sitio que no se pudo abrir).
                    if !is_new_tab_url(&page_url)
                        && !error_page
                        && !page_url.starts_with("chrome-extension://")
                        && st.history.last().map(|h| &h.url) != Some(&page_url)
                    {
                        st.history.push(HistoryEntry { title: title_snapshot, url: page_url });
                        let excess = st.history.len().saturating_sub(HISTORY_MAX);
                        st.history.drain(..excess);
                        st.save_history();
                    }
                    st.save_session();
                }
                sync::push_history(&state_load);
                sync::push_active_tab(&state_load);
                sync::push_tabs(&state_load);
            })
            .build_as_child(window.as_ref())
    };
    let webview = match webview {
        Ok(webview) => webview,
        // Una pagina que abre una ventana no puede tirar abajo el navegador.
        Err(e) if is_window => {
            eprintln!("[pestanas] no se pudo crear la ventana de window.open: {e}");
            return None;
        }
        Err(e) => panic!("no se pudo crear el webview de la pestana: {e}"),
    };

    security::attach_tab(state, &webview, id);
    on_source_changed(state, &webview, id);
    attach_favicon(state, &webview, id);
    crate::audio::attach(&webview);
    crate::fullscreen::attach(state, &webview, id);
    permissions::attach(state, &webview);
    shortcuts::attach(state, &webview);
    downloads::attach(state, &webview);
    if (zoom - 1.0).abs() > f64::EPSILON {
        let _ = webview.zoom(zoom);
    }

    let core = webview.webview();
    {
        let mut st = state.borrow_mut();
        st.tabs.push(Tab {
            id,
            webview,
            title: String::new(),
            url: url.to_string(),
            loading: true,
            favicon: None,
            sec: Default::default(),
        });
        st.active = st.tabs.len() - 1;
    }

    activated(state);
    Some(core)
}

/// La direccion tambien cambia SIN cargar una pagina nueva: los sitios que
/// son una sola aplicacion (Chrome Web Store, YouTube, GitHub...) navegan
/// con history.pushState. wry solo avisa las cargas completas, asi que la
/// barra seguia mostrando la direccion vieja, y en la tienda de extensiones
/// no aparecia "Agregar a Michi" (la pestana creia seguir en la portada).
fn on_source_changed(state: &Shared, webview: &wry::WebView, id: u64) {
    use webview2_com::SourceChangedEventHandler;
    use wry::WebViewExtWindows;

    let s = state.clone();
    let handler = SourceChangedEventHandler::create(Box::new(move |sender, _| {
        let Some(core) = sender else { return Ok(()) };
        let url = unsafe { crate::native::read_pwstr(|p| core.Source(p)) };
        let changed = {
            let mut st = s.borrow_mut();
            match st.tabs.iter_mut().find(|t| t.id == id) {
                Some(tab) if !url.is_empty() && tab.url != url => {
                    tab.url = url;
                    true
                }
                _ => false,
            }
        };
        if changed {
            sync::push_tabs(&s);
            sync::push_active_tab(&s);
            s.borrow().save_session();
        }
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { webview.webview().add_SourceChanged(&handler, &mut token) };
}

/// El icono del sitio (favicon). WebView2 avisa cada vez que cambia (al
/// navegar, o cuando el sitio lo carga mas tarde); ahi le pedimos la imagen en
/// PNG y la guardamos como data URL para el panel. Todo ocurre en el hilo de la
/// interfaz (los callbacks de WebView2), asi que se puede tocar el estado.
fn attach_favicon(state: &Shared, webview: &wry::WebView, id: u64) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_15, COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG,
    };
    use webview2_com::{FaviconChangedEventHandler, GetFaviconCompletedHandler};
    use windows_core::Interface;
    use wry::WebViewExtWindows;

    // ICoreWebView2_15 (favicon) existe desde Edge WebView2 114; si el runtime
    // es mas viejo, simplemente no hay iconos (se queda el punto).
    let Ok(core) = webview.webview().cast::<ICoreWebView2_15>() else { return };

    let s = state.clone();
    let handler = FaviconChangedEventHandler::create(Box::new(move |sender, _| {
        let Some(core15) = sender.and_then(|c| c.cast::<ICoreWebView2_15>().ok()) else {
            return Ok(());
        };
        let s = s.clone();
        let done = GetFaviconCompletedHandler::create(Box::new(move |result, stream| {
            if result.is_ok() {
                set_favicon(&s, id, stream.and_then(crate::native::stream_to_data_url));
            }
            Ok(())
        }));
        let _ = unsafe { core15.GetFavicon(COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG, &done) };
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { core.add_FaviconChanged(&handler, &mut token) };
}

/// Guarda el favicon de una pestana (si cambio) y refresca el panel. Tambien
/// queda recordado por sitio, para las sugerencias de la barra (suggest.rs).
fn set_favicon(state: &Shared, id: u64, data_url: Option<String>) {
    let changed = {
        let mut st = state.borrow_mut();
        match st.tabs.iter_mut().find(|t| t.id == id) {
            Some(tab) if tab.favicon != data_url => {
                tab.favicon = data_url.clone();
                Some(tab.url.clone())
            }
            _ => None,
        }
    };
    if let Some(url) = changed {
        crate::suggest::remember_favicon(state, &url, &data_url);
        sync::push_tabs(state);
    }
}

/// Todo lo que hay que actualizar despues de abrir, cerrar o cambiar de
/// pestana activa.
pub fn activated(state: &Shared) {
    // El popup de una extension actua sobre la pestana que estaba al frente.
    crate::popup::close(state);
    crate::fullscreen::on_tab_switch(state);
    layout::relayout(state);
    // Como cualquier navegador: la pestana que queda al frente recibe el
    // foco del teclado (en la pestana nueva, su buscador ya queda listo para
    // escribir; y los atajos de teclado llegan por su WebView).
    if let Some(t) = state.borrow().active_tab() {
        let _ = t.webview.focus();
    }
    sync::push_tabs(state);
    sync::push_active_tab(state);
    sync::push_wallpaper(state);
    sync::push_own_pages(state);
    state.borrow().save_session();
}

pub fn close_tab(state: &Shared, id: u64) {
    let should_reopen;
    {
        let mut st = state.borrow_mut();
        let Some(i) = st.tabs.iter().position(|t| t.id == id) else { return };
        let was_active = st.active;
        let closed = st.tabs.remove(i);
        if !is_new_tab_url(&closed.url) {
            st.closed_tabs.push(closed.url.clone());
            let excess = st.closed_tabs.len().saturating_sub(25);
            st.closed_tabs.drain(..excess);
        }
        should_reopen = st.tabs.is_empty();
        if !should_reopen {
            st.active = if i < was_active { was_active - 1 } else { was_active.min(st.tabs.len() - 1) };
        }
    }

    if should_reopen {
        state.borrow_mut().request_tab(HOME_URL);
        return;
    }

    activated(state);
}
