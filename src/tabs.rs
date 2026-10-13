//! Creacion y cierre de pestanas. Cada pestana es un WebView real e
//! independiente (WebView2), asi que funciona con cualquier sitio, incluidos
//! los que bloquean ser mostrados dentro de un iframe.

use wry::dpi::{LogicalPosition, LogicalSize};
use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2, ICoreWebView2Environment};
use wry::{PageLoadEvent, Rect, WebViewBuilderExtWindows, WebViewExtWindows};

use crate::state::{is_own_page, own_page_path, HistoryEntry, PendingTab, PendingWindow, Shared, Tab, EDGE_IDLE, HOME_URL, TOPBAR_H};
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

pub fn open_tab(state: &Shared, url: &str, incognito: bool) {
    let _ = create_tab(state, url, None, incognito, false);
}

/// Una pestana pedida con AppState::request_tab (o atras, con
/// request_background_tab).
pub fn open_pending(state: &Shared, p: PendingTab) {
    let created = create_tab(state, &p.url, None, p.incognito, p.background);
    if let (Some(_), Some(group)) = (created, p.group) {
        let id = state.borrow().tabs.iter().map(|t| t.id).max();
        if let Some(id) = id {
            crate::groups::add_to(state, id, group);
        }
    }
}

/// window.open() de una pagina (ver state::PendingWindow): la pestana nace
/// sin navegar, en el entorno de la pagina que la abre, y se la entregamos
/// al motor como la ventana nueva; el motor la lleva a `url`.
pub fn open_window(state: &Shared, w: PendingWindow) {
    let core = create_tab(state, &w.url, Some(w.environment), w.incognito, false);
    unsafe {
        // Sin pestana, window.open() devuelve null (como con un bloqueador).
        if let Some(core) = &core {
            let _ = w.args.SetNewWindow(core);
        }
        let _ = w.args.SetHandled(true);
        let _ = w.deferral.Complete();
    }
}

/// Crea la pestana y la deja al frente (o atras, junto a la activa, con
/// `background`). Con `environment` es una ventana de window.open(): no
/// navega (eso lo hace el motor despues de SetNewWindow).
/// `incognito`: perfil InPrivate, sin rastro en historial ni sesion.
fn create_tab(
    state: &Shared,
    url: &str,
    environment: Option<ICoreWebView2Environment>,
    incognito: bool,
    background: bool,
) -> Option<ICoreWebView2> {
    let id = state.borrow_mut().alloc_id();

    // Clonamos el Rc<Window> y soltamos el prestamo de `state` de inmediato:
    // construir el WebView puede disparar callbacks (on_page_load, etc.) de
    // forma sincronica, y esos callbacks tambien necesitan pedir prestado
    // `state`, asi que no podemos mantener un borrow() vivo durante el build.
    let window = state.borrow().window.clone()?;

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
            // Perfil InPrivate de WebView2: cookies, cache y permisos en
            // memoria, aparte del perfil normal (ver Tab::incognito).
            .with_incognito(incognito)
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
                    // actualizamos a la entrada de historial recien agregada
                    // (las de incognito no tienen).
                    let url = st.tabs.iter().find(|t| t.id == id && !t.incognito).map(|t| t.url.clone());
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
                let (title_snapshot, finished, error_page, incognito) = {
                    let mut st = state_load.borrow_mut();
                    if let Some(tab) = st.tabs.iter_mut().find(|t| t.id == id) {
                        tab.url = page_url.clone();
                        tab.loading = matches!(event, PageLoadEvent::Started);
                        // Lo de la pagina caida ya paso (salvo en la pagina que lo cuenta).
                        if own_page_path(&page_url) != Some("/crashed") {
                            tab.crashed = None;
                        }
                        // La pagina nueva avisa por su cuenta si reproduce algo.
                        if tab.loading {
                            tab.playing.video = false;
                        }
                    }
                    let tab = st.tabs.iter().find(|t| t.id == id);
                    let title = tab.map(|t| t.title.clone()).unwrap_or_default();
                    let error_page = tab.is_some_and(|t| t.sec.error_page);
                    (title, matches!(event, PageLoadEvent::Finished), error_page, tab.is_some_and(|t| t.incognito))
                };

                if finished {
                    let mut st = state_load.borrow_mut();
                    // Ni paginas propias, ni las internas de una extension
                    // (su popup u opciones abiertos en una pestana), ni las
                    // de error (un sitio que no se pudo abrir), ni nada de
                    // lo que se navega en incognito.
                    if !is_new_tab_url(&page_url)
                        && !error_page
                        && !incognito
                        && !page_url.starts_with("chrome-extension://")
                        && st.history.last().map(|h| &h.url) != Some(&page_url)
                    {
                        st.history.push(HistoryEntry {
                            title: title_snapshot,
                            url: page_url,
                            time: crate::storage::now_secs(),
                        });

                        let excess = st.history.len().saturating_sub(HISTORY_MAX);
                        st.history.drain(..excess);
                        st.save_history();
                    }
                    st.save_session();
                }
                // Cada sitio con su zoom (ver zoom.rs).
                crate::zoom::apply(&state_load, id);
                sync::push_media(&state_load);
                sync::push_history(&state_load);
                sync::push_active_tab(&state_load);
                sync::push_tabs(&state_load);
            })
            .build_as_child(window.as_ref())
    };
    // Que no se pueda crear una pestana (WebView2 sin memoria, o cerrandose)
    // nunca tira abajo el navegador con todas las demas: queda en el log y
    // el resto sigue andando.
    let webview = match webview {
        Ok(webview) => webview,
        Err(e) => {
            eprintln!("[pestanas] no se pudo crear la pestana ({url}): {e}");
            return None;
        }
    };

    security::attach_tab(state, &webview, id);
    on_source_changed(state, &webview, id);
    attach_favicon(state, &webview, id);
    crate::audio::attach(state, &webview, id);
    crate::fullscreen::attach(state, &webview, id);
    permissions::attach(state, &webview);
    shortcuts::attach(state, &webview, false);
    downloads::attach(state, &webview);
    crate::recovery::attach(state, &webview, Some(id));
    crate::zoom::attach(state, &webview, id);
    crate::context_menu::attach(state, &webview, id);

    let core = webview.webview();
    let tab = Tab {
        id,
        webview,
        title: String::new(),
        url: url.to_string(),
        loading: true,
        favicon: None,
        sec: Default::default(),
        playing: Default::default(),
        incognito,
        rest: Default::default(),
        crashed: None,
        pinned: false,
        muted: false,
        group: None,
    };
    {
        let mut st = state.borrow_mut();
        if background {
            // Junto a la activa, despues de las que ya se abrieron atras desde
            // ella (en orden, como en Chrome), y nunca entre las fijadas.
            let active_id = st.active_tab().map(|t| t.id);
            let after_chain = st
                .background_chain
                .filter(|(opener, _)| Some(*opener) == active_id)
                .and_then(|(_, last)| st.tabs.iter().position(|t| t.id == last));
            let at = after_chain.unwrap_or(st.active).saturating_add(1).max(st.pinned_count()).min(st.tabs.len());
            // Abierta desde una pestana de un grupo: queda en ese grupo.
            let mut tab = tab;
            tab.group = st.active_tab().and_then(|t| t.group);
            st.tabs.insert(at, tab);
            if at <= st.active {
                st.active += 1;
            }
            st.background_chain = active_id.map(|opener| (opener, id));
            crate::groups::normalize(&mut st);
        } else {
            st.tabs.push(tab);
            st.active = st.tabs.len() - 1;
        }
    }
    crate::zoom::apply(state, id);

    if background {
        layout::relayout(state);
        sync::push_tabs(state);
        state.borrow().save_session();
        crate::perf::on_activated(state);
    } else {
        activated(state);
    }
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
            crate::zoom::apply(&s, id);
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
                Some((tab.url.clone(), tab.incognito))
            }
            _ => None,
        }
    };
    if let Some((url, incognito)) = changed {
        // Recordar el logo de un sitio dice que se lo visito: en incognito no.
        if !incognito {
            crate::suggest::remember_favicon(state, &url, &data_url);
        }
        sync::push_tabs(state);
    }
}

/// Todo lo que hay que actualizar despues de abrir, cerrar o cambiar de
/// pestana activa.
pub fn activated(state: &Shared) {
    // El popup de una extension actua sobre la pestana que estaba al frente.
    crate::popup::close(state);
    crate::fullscreen::on_tab_switch(state);
    crate::groups::reveal_active(&mut state.borrow_mut());
    layout::relayout(state);
    sync::push_media(state);
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
    // Las que quedan atras descansan segun el modo de rendimiento.
    crate::perf::on_activated(state);
}

pub fn close_tab(state: &Shared, id: u64) {
    let should_reopen;
    {
        let mut st = state.borrow_mut();
        let Some(i) = st.tabs.iter().position(|t| t.id == id) else { return };
        let was_active = st.active;
        let closed = st.tabs.remove(i);
        // Una de incognito no se puede reabrir: seria un rastro de lo que se vio.
        if !is_new_tab_url(&closed.url) && !closed.incognito {
            st.closed_tabs.push(closed.url.clone());
            let excess = st.closed_tabs.len().saturating_sub(25);
            st.closed_tabs.drain(..excess);
        }
        should_reopen = st.tabs.is_empty();
        // Un grupo que se quedo sin pestanas deja de existir.
        crate::groups::normalize(&mut st);
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

/// Lleva la pestana `id` justo antes de la pestana `before` (None = al
/// final): lo que se arrastro en el panel. Por id y no por posicion, porque
/// el panel no muestra las pestanas de los grupos plegados. Las fijadas solo
/// se mueven entre fijadas, y las demas, debajo de ellas; y al soltarla entre
/// dos pestanas de un grupo (o justo debajo de su encabezado, `into_group`),
/// entra a ese grupo (ver groups::after_move).
pub fn move_tab(state: &Shared, id: u64, before: Option<u64>, into_group: Option<u64>) {
    {
        let mut st = state.borrow_mut();
        let Some(from) = st.tabs.iter().position(|t| t.id == id) else { return };
        let active_id = st.active_tab().map(|t| t.id);
        let tab = st.tabs.remove(from);
        let pinned = st.pinned_count();
        let at = before.filter(|b| *b != id).and_then(|b| st.tabs.iter().position(|t| t.id == b)).unwrap_or(st.tabs.len());
        let at = if tab.pinned { at.min(pinned) } else { at.max(pinned) };
        st.tabs.insert(at, tab);
        if let Some(i) = active_id.and_then(|a| st.tabs.iter().position(|t| t.id == a)) {
            st.active = i;
        }
        let joins = into_group.filter(|g| st.groups.iter().any(|x| x.id == *g));
        match (joins, st.tabs.iter_mut().find(|t| t.id == id && !t.pinned)) {
            (Some(g), Some(tab)) => {
                tab.group = Some(g);
                crate::groups::normalize(&mut st);
            }
            _ => crate::groups::after_move(&mut st, id),
        }
        st.save_session();
    }
    sync::push_tabs(state);
}

/// Fija (o desfija) una pestana: pasa al final de las fijadas, o justo
/// debajo de ellas.
pub fn set_pinned(state: &Shared, id: u64, pinned: bool) {
    {
        let mut st = state.borrow_mut();
        let Some(from) = st.tabs.iter().position(|t| t.id == id) else { return };
        if st.tabs[from].pinned == pinned {
            return;
        }
        let active_id = st.active_tab().map(|t| t.id);
        let mut tab = st.tabs.remove(from);
        tab.pinned = pinned;
        // Una fijada no tiene grupo.
        if pinned {
            tab.group = None;
        }
        let at = st.pinned_count();
        st.tabs.insert(at, tab);
        crate::groups::normalize(&mut st);
        if let Some(i) = active_id.and_then(|a| st.tabs.iter().position(|t| t.id == a)) {
            st.active = i;
        }
        st.save_session();
    }
    sync::push_tabs(state);
}

/// Silencia (o vuelve a dejar sonar) una pestana.
pub fn set_muted(state: &Shared, id: u64, muted: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;
    use windows_core::Interface;
    {
        let mut st = state.borrow_mut();
        let Some(tab) = st.tabs.iter_mut().find(|t| t.id == id) else { return };
        let Ok(core) = tab.webview.webview().cast::<ICoreWebView2_8>() else { return };
        if unsafe { core.SetIsMuted(muted) }.is_err() {
            return;
        }
        tab.muted = muted;
    }
    sync::push_tabs(state);
}

/// Otra pestana con la misma pagina (en el mismo grupo, si tiene).
pub fn duplicate(state: &Shared, id: u64) {
    let mut st = state.borrow_mut();
    let Some(tab) = st.tabs.iter().find(|t| t.id == id) else { return };
    let (url, incognito, group) = (tab.real_url().to_string(), tab.incognito, tab.group);
    st.request_tab_in(&url, incognito);
    if let Some(p) = st.pending_tabs.last_mut() {
        p.group = group;
    }
}

/// Cierra todas menos `id` y las fijadas.
pub fn close_others(state: &Shared, id: u64) {
    let others: Vec<u64> = state.borrow().tabs.iter().filter(|t| t.id != id && !t.pinned).map(|t| t.id).collect();
    for other in others {
        close_tab(state, other);
    }
    let i = state.borrow().tabs.iter().position(|t| t.id == id);
    if let Some(i) = i {
        state.borrow_mut().active = i;
        activated(state);
    }
}

/// Vuelve a cargar una pestana. En la pagina de "dejo de funcionar", lo que
/// se vuelve a cargar es el sitio que se cayo, no el aviso.
pub fn reload(state: &Shared, id: u64) {
    let crashed = {
        let st = state.borrow();
        let Some(tab) = st.tabs.iter().find(|t| t.id == id) else { return };
        if own_page_path(&tab.url) == Some("/crashed") {
            true
        } else {
            let _ = tab.webview.reload();
            false
        }
    };
    if crashed {
        crate::recovery::reload_crashed(state, id);
    }
}
