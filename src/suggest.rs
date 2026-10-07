//! Sugerencias de la barra de direcciones, como en Firefox.
//!
//! - Al entrar a la barra (vacia o con la direccion actual): los sitios mas
//!   visitados.
//! - Al escribir: completa en linea el sitio mas visitado que empiece con lo
//!   escrito ("y" -> "youtube.com/", con lo agregado seleccionado), y abajo las
//!   sugerencias del buscador y lo que coincide del historial, los marcadores y
//!   las pestanas abiertas, cada uno con el logo de su sitio.
//!
//! El desplegable es un WebView aparte (ui/suggest.html), por lo mismo que el
//! menu "..." (ver menu.rs): la barra mide TOPBAR_H de alto. El foco se queda en
//! la barra: las flechas, Enter y Escape los maneja ella y le avisa a Rust, que
//! es quien sabe que hay en la lista y cual esta elegida.
//!
//! Lo del buscador viaja por la red (lo escrito se le envia al buscador, sin
//! cookies); se puede apagar en ajustes. Nunca se envia algo con forma de
//! direccion (un sitio, localhost, una IP, un archivo). El historial, los
//! marcadores y las pestanas se buscan aca, sin enviar nada.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::http::Request;
use wry::{Rect, WebView};

use crate::state::{AppState, Shared, UserEvent, TOPBAR_H};
use crate::storage::{self, HistoryEntry};
use crate::{layout, sync};

// Medidas de las filas: las mismas que usa ui/suggest.html (alto fijo), asi el
// alto del desplegable se calcula aca sin esperar a que la pagina lo mida.
const PAD: f64 = 6.0;
const ROW_TWO: f64 = 48.0;
const ROW_ONE: f64 = 36.0;
const HEADER: f64 = 26.0;
/// Hueco entre el borde de abajo de la barra de direcciones y el desplegable.
const TOP: f64 = TOPBAR_H - 4.0;

const MAX_ENGINE: usize = 5;
const MAX_LOCAL: usize = 8;
const MAX_LOCAL_WITH_ENGINE: usize = 5;
const MAX_TOP_SITES: usize = 8;
const MAX_FAVICONS: usize = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Primera fila: el sitio que se completo en linea.
    Autofill,
    /// Primera fila: ir a lo escrito (tiene forma de direccion).
    Go,
    /// Primera fila: buscar lo escrito.
    Search,
    /// Sugerencia del buscador.
    Suggestion,
    History,
    Bookmark,
    /// Pestana abierta: elegirla cambia a esa pestana.
    Tab,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub kind: Kind,
    pub title: String,
    pub detail: String,
    /// Logo del sitio (data URL) si se conoce.
    pub icon: Option<String>,
    /// Titulo de seccion que va antes de esta fila.
    pub header: Option<&'static str>,
    #[serde(skip)]
    url: String,
    /// Lo que se pone en la barra al elegirla con las flechas.
    #[serde(skip)]
    fill: String,
    #[serde(skip)]
    tab_id: Option<u64>,
}

impl Item {
    fn height(&self) -> f64 {
        let row = if self.kind == Kind::Suggestion { ROW_ONE } else { ROW_TWO };
        row + if self.header.is_some() { HEADER } else { 0.0 }
    }
}

#[derive(Clone, Debug)]
struct Autofill {
    /// Lo que queda escrito en la barra ("youtube.com/").
    text: String,
    url: String,
    title: String,
}

/// Logo de cada sitio visitado (sin "www."), para las sugerencias. Se guarda
/// en favicons.json; con un tope, se van los que hace mas que no se ven.
#[derive(Default, Serialize, Deserialize)]
pub struct Favicons {
    icons: HashMap<String, (u64, String)>,
    next: u64,
}

impl Favicons {
    fn get(&self, url: &str) -> Option<String> {
        let (_, host) = split_url(url)?;
        self.icons.get(bare(&host)).map(|(_, icon)| icon.clone())
    }

    /// Devuelve si cambio algo (para guardar solo entonces).
    fn put(&mut self, host: String, icon: String) -> bool {
        self.next += 1;
        let stamp = self.next;
        let changed = self.icons.get(&host).is_none_or(|(_, old)| *old != icon);
        self.icons.insert(host, (stamp, icon));
        if self.icons.len() > MAX_FAVICONS {
            if let Some(oldest) = self.icons.iter().min_by_key(|(_, (s, _))| *s).map(|(h, _)| h.clone()) {
                self.icons.remove(&oldest);
            }
        }
        changed
    }
}

/// Estado del desplegable (vive en AppState::suggest).
#[derive(Default)]
pub struct Suggest {
    pub view: Option<WebView>,
    pub open: bool,
    /// Donde esta ahora (para el recorte del fondo animado).
    pub pos: (f64, f64),
    /// Lo escrito, tal cual (sin lo completado en linea).
    query: String,
    items: Vec<Item>,
    selected: Option<usize>,
    /// Sugerencias del buscador para `query` (llegan despues, de la red).
    engine: Vec<String>,
    autofill: Option<Autofill>,
    /// x y ancho de la barra de direcciones (lo manda la barra).
    rect: (f64, f64),
    /// Numero del ultimo pedido: una respuesta del buscador vieja se descarta.
    seq: Arc<AtomicU64>,
    favicons: Favicons,
}

impl Suggest {
    pub fn load() -> Self {
        Self { favicons: storage::load(storage::FAVICONS_FILE), ..Default::default() }
    }
}

// ---------------------------------------------------------------------------
// El WebView del desplegable
// ---------------------------------------------------------------------------

pub fn build(state: &Shared) -> WebView {
    let window = state.borrow().window.clone().expect("la ventana debe existir antes de las sugerencias");
    let s = state.clone();
    // Solo puede mostrar su propia pagina (ver security::chrome_builder).
    crate::security::chrome_builder()
        // Opaco a proposito (ver chrome::PANEL_BG): se superpone a la pestana.
        .with_background_color(crate::chrome::PANEL_BG)
        .with_visible(false)
        .with_url("app://localhost/suggest")
        .with_bounds(rect(0.0, TOP, 400.0, 200.0))
        .with_ipc_handler(move |req: Request<String>| crate::ipc::dispatch(&s, &req.uri().to_string(), req.body()))
        .build_as_child(window.as_ref())
        .expect("no se pudo crear el desplegable de sugerencias")
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { position: LogicalPosition::new(x, y).into(), size: LogicalSize::new(w, h).into() }
}

/// Ubica el desplegable debajo de la barra con el alto justo para su lista, y
/// lo muestra (o lo oculta si no hay nada que sugerir).
fn place(state: &Shared) {
    let showing = {
        let mut st = state.borrow_mut();
        let Some(window) = st.window.clone() else { return };
        let size = window.inner_size().to_logical::<f64>(window.scale_factor());
        let empty = st.suggest.items.is_empty();
        let sg = &mut st.suggest;
        let Some(view) = &sg.view else { return };
        if empty {
            if sg.open {
                sg.open = false;
                let _ = view.set_visible(false);
            }
            false
        } else {
            let h = (PAD * 2.0 + sg.items.iter().map(Item::height).sum::<f64>()).min((size.height - TOP - 12.0).max(80.0));
            let (x, w) = sg.rect;
            let w = w.clamp(260.0, size.width.max(260.0));
            let x = x.clamp(0.0, (size.width - w).max(0.0));
            sg.pos = (x, TOP);
            let _ = view.set_bounds(rect(x, TOP, w, h));
            if !sg.open {
                sg.open = true;
                let _ = view.set_visible(true);
            }
            true
        }
    };
    topbar(state, &format!("window.onSuggestOpen && window.onSuggestOpen({showing})"));
    if showing {
        layout::raise_chrome(state);
        sync::push_wallpaper(state);
    }
}

pub fn close(state: &Shared) {
    {
        let mut st = state.borrow_mut();
        let sg = &mut st.suggest;
        // Se cancela un pedido al buscador en curso.
        sg.seq.fetch_add(1, Ordering::SeqCst);
        if !sg.open {
            return;
        }
        sg.open = false;
        sg.selected = None;
        // La lista NO se borra: al hacer clic en una fila, la barra pierde el
        // foco (y pide cerrar) a la vez que llega el clic; los dos mensajes
        // pueden llegar en cualquier orden, y el clic tiene que encontrar su fila.
        if let Some(view) = &sg.view {
            let _ = view.set_visible(false);
        }
    }
    topbar(state, "window.onSuggestOpen && window.onSuggestOpen(false)");
}

/// La lista a la pagina del desplegable (tambien cuando la pagina avisa que cargo).
pub fn push(state: &Shared) {
    let st = state.borrow();
    let Some(view) = &st.suggest.view else { return };
    let data = json!({
        "items": st.suggest.items,
        "selected": st.suggest.selected,
        "query": st.suggest.query,
    });
    let _ = view.evaluate_script(&format!("window.onSuggest && window.onSuggest({data})"));
}

fn topbar(state: &Shared, js: &str) {
    if let Some(tb) = &state.borrow().topbar {
        let _ = tb.evaluate_script(js);
    }
}

// ---------------------------------------------------------------------------
// Lo que manda la barra
// ---------------------------------------------------------------------------

/// Cambio el texto de la barra (o se entro a ella: texto vacio = los sitios
/// mas visitados). `deleting`: se estaba borrando, no se completa en linea
/// (si no, borrar la parte completada la volveria a poner).
pub fn on_input(state: &Shared, text: String, deleting: bool, x: f64, w: f64) {
    let seq = {
        let mut st = state.borrow_mut();
        let visits = aggregate(&st.history);
        let sg = &mut st.suggest;
        sg.rect = (x, w);
        let seq = sg.seq.fetch_add(1, Ordering::SeqCst) + 1;
        if sg.query != text {
            // Mientras llegan las nuevas, quedan las del buscador que todavia
            // sirven (escribiendo de corrido, casi todas): sin parpadeos.
            let q = fold(text.trim());
            sg.engine.retain(|s| !q.is_empty() && fold(s).starts_with(&q));
        }
        sg.query = text.clone();
        sg.autofill = if deleting { None } else { autofill(&visits, &text) };
        rebuild(&mut st, &visits);
        st.suggest.selected = if text.trim().is_empty() { None } else { Some(0) };
        seq
    };
    place(state);
    push(state);
    let fill = state.borrow().suggest.autofill.as_ref().map(|a| a.text.clone());
    if let Some(fill) = fill {
        let data = json!({ "query": text, "fill": fill });
        topbar(state, &format!("window.onSuggestAutofill && window.onSuggestAutofill({data})"));
    }
    fetch_engine(state, seq, text);
}

/// Flecha abajo (+1) o arriba (-1): cambia la fila elegida y pone su texto en
/// la barra.
pub fn on_key(state: &Shared, delta: i32) {
    let (index, fill) = {
        let mut st = state.borrow_mut();
        let sg = &mut st.suggest;
        let n = sg.items.len() as i64;
        if !sg.open || n == 0 {
            return;
        }
        let next = match sg.selected {
            None if delta > 0 => 0,
            None => n - 1,
            Some(i) => (i as i64 + delta as i64).rem_euclid(n),
        } as usize;
        sg.selected = Some(next);
        (next, sg.items[next].fill.clone())
    };
    if let Some(view) = &state.borrow().suggest.view {
        let _ = view.evaluate_script(&format!("window.onSuggestSelected && window.onSuggestSelected({index})"));
    }
    let fill = serde_json::to_string(&fill).unwrap_or_default();
    topbar(state, &format!("window.onSuggestFill && window.onSuggestFill({fill})"));
}

/// Enter en la barra: la fila elegida, o lo escrito (direccion o busqueda).
pub fn on_enter(state: &Shared, input: String) {
    let pick = {
        let st = state.borrow();
        let sg = &st.suggest;
        sg.selected.filter(|_| sg.open).and_then(|i| sg.items.get(i).cloned())
    };
    close(state);
    match pick {
        // Lo completado en linea, si no se lo cambio despues.
        Some(item) if item.kind == Kind::Autofill && input.trim().eq_ignore_ascii_case(item.fill.trim()) => {
            open_url(state, &item.url)
        }
        Some(item) if !matches!(item.kind, Kind::Autofill | Kind::Go | Kind::Search) => activate(state, &item),
        _ => crate::ipc::navigate_input(state, &input),
    }
}

/// Clic en una fila del desplegable.
pub fn on_pick(state: &Shared, index: usize) {
    let (item, query) = {
        let st = state.borrow();
        (st.suggest.items.get(index).cloned(), st.suggest.query.clone())
    };
    close(state);
    let Some(item) = item else { return };
    match item.kind {
        Kind::Autofill => open_url(state, &item.url),
        Kind::Go | Kind::Search => crate::ipc::navigate_input(state, &query),
        _ => activate(state, &item),
    }
    // El clic se llevo el foco al desplegable (ya oculto): a la pagina.
    if let Some(tab) = state.borrow().active_tab() {
        let _ = tab.webview.focus();
    }
    topbar(state, "window.onSuggestDone && window.onSuggestDone()");
}

fn activate(state: &Shared, item: &Item) {
    match (item.kind, item.tab_id) {
        (Kind::Tab, Some(id)) => {
            {
                let mut st = state.borrow_mut();
                match st.tabs.iter().position(|t| t.id == id) {
                    Some(i) => st.active = i,
                    None => {
                        drop(st);
                        return open_url(state, &item.url);
                    }
                }
            }
            crate::tabs::activated(state);
        }
        _ => open_url(state, &item.url),
    }
}

fn open_url(state: &Shared, url: &str) {
    if !crate::security::is_safe_to_open(url) {
        return;
    }
    if let Some(tab) = state.borrow().active_tab() {
        let _ = tab.webview.load_url(url);
    }
}

// ---------------------------------------------------------------------------
// Armar la lista
// ---------------------------------------------------------------------------

fn rebuild(st: &mut AppState, visits: &[Visit]) {
    let query = st.suggest.query.trim().to_string();
    let engine_id = st.settings.search_engine.clone();
    if query.is_empty() {
        st.suggest.items = top_sites(st, visits);
        return;
    }

    let mut items = Vec::new();
    let mut skip = HashSet::new();
    match &st.suggest.autofill {
        Some(af) => {
            skip.insert(dedupe_key(&af.url));
            items.push(Item {
                kind: Kind::Autofill,
                title: af.title.clone(),
                detail: display_url(&af.url),
                icon: st.suggest.favicons.get(&af.url),
                header: None,
                url: af.url.clone(),
                fill: af.text.clone(),
                tab_id: None,
            });
        }
        None => {
            let search = is_search(&query, &engine_id);
            let url = storage::resolve_input(&query, &engine_id).unwrap_or_default();
            items.push(Item {
                kind: if search { Kind::Search } else { Kind::Go },
                title: query.clone(),
                detail: if search {
                    format!("Buscar con {}", storage::engine(&engine_id).1)
                } else {
                    "Ir a esta direccion".into()
                },
                icon: None,
                header: None,
                url,
                fill: query.clone(),
                tab_id: None,
            });
        }
    }

    let engine: Vec<String> = st
        .suggest
        .engine
        .iter()
        .filter(|s| !s.trim().eq_ignore_ascii_case(&query))
        .take(MAX_ENGINE)
        .cloned()
        .collect();
    for s in &engine {
        items.push(Item {
            kind: Kind::Suggestion,
            title: s.clone(),
            detail: String::new(),
            icon: None,
            header: None,
            url: storage::search_url(&engine_id, s),
            fill: s.clone(),
            tab_id: None,
        });
    }

    let limit = if engine.is_empty() { MAX_LOCAL } else { MAX_LOCAL_WITH_ENGINE };
    let mut local = local_matches(st, visits, &query, &skip, limit);
    if let Some(first) = local.first_mut() {
        first.header = Some("Sugerencias de Michi");
    }
    items.extend(local);
    st.suggest.items = items;
}

/// Llegaron las sugerencias del buscador para `query`.
pub fn on_engine(state: &Shared, seq: u64, query: String, list: Vec<String>) {
    {
        let mut st = state.borrow_mut();
        let sg = &st.suggest;
        if !sg.open || sg.seq.load(Ordering::SeqCst) != seq || sg.query != query {
            return;
        }
        // La fila elegida sigue elegida aunque cambien las de arriba.
        let chosen = sg.selected.and_then(|i| sg.items.get(i)).map(|it| (it.kind, it.url.clone()));
        st.suggest.engine = list;
        let visits = aggregate(&st.history);
        rebuild(&mut st, &visits);
        let sg = &mut st.suggest;
        sg.selected = match chosen {
            Some((kind, url)) => sg.items.iter().position(|it| it.kind == kind && it.url == url).or(Some(0)),
            None => None,
        };
    }
    place(state);
    push(state);
}

/// Una URL del historial, con cuantas veces y que tan hace poco se visito.
struct Visit {
    url: String,
    title: String,
    score: f64,
}

/// Junta el historial por direccion: visitas mas recientes y frecuentes, mas
/// puntaje (la "frecencia" de Firefox, simplificada).
fn aggregate(history: &[HistoryEntry]) -> Vec<Visit> {
    let n = history.len().max(1) as f64;
    let mut by_url: HashMap<&str, (u32, usize, &str)> = HashMap::new();
    for (i, h) in history.iter().enumerate() {
        if !is_web(&h.url) {
            continue;
        }
        let e = by_url.entry(h.url.as_str()).or_insert((0, 0, ""));
        e.0 += 1;
        e.1 = i;
        if !h.title.is_empty() {
            e.2 = h.title.as_str();
        }
    }
    by_url
        .into_iter()
        .map(|(url, (visits, last, title))| Visit {
            url: url.to_string(),
            title: title.to_string(),
            score: (visits as f64).ln_1p() * 2.0 + 3.0 * (last as f64 + 1.0) / n,
        })
        .collect()
}

/// El sitio mas visitado cuyo dominio empieza con lo escrito.
fn autofill(visits: &[Visit], typed: &str) -> Option<Autofill> {
    let q = typed.trim().to_ascii_lowercase();
    if q.is_empty() || q.contains(char::is_whitespace) || q.contains('/') || q.contains("://") {
        return None;
    }
    let (www, q) = match q.strip_prefix("www.") {
        Some(rest) => (true, rest.to_string()),
        None => (false, q),
    };
    if q.is_empty() {
        return None;
    }
    // dominio sin www -> (puntaje total, mejor visita)
    let mut hosts: HashMap<String, (f64, &Visit)> = HashMap::new();
    for v in visits {
        let Some((_, host)) = split_url(&v.url) else { continue };
        let b = bare(&host);
        if !b.starts_with(&q) {
            continue;
        }
        let e = hosts.entry(b.to_string()).or_insert((0.0, v));
        e.0 += v.score;
        if v.score > e.1.score {
            e.1 = v;
        }
    }
    let (host, (_, best)) = hosts.into_iter().max_by(|a, b| a.1 .0.total_cmp(&b.1 .0))?;
    let (scheme, full_host) = split_url(&best.url)?;
    let url = format!("{scheme}://{full_host}/");
    // El titulo de la portada del sitio si se la visito; si no, el de la mas visitada.
    let title = visits
        .iter()
        .find(|v| v.url.eq_ignore_ascii_case(&url) && !v.title.is_empty())
        .map(|v| v.title.as_str())
        .unwrap_or(&best.title);
    let text = format!("{}{host}/", if www { "www." } else { "" });
    Some(Autofill { text, url: url.clone(), title: if title.is_empty() { display_url(&url) } else { title.to_string() } })
}

/// Lo del historial, los marcadores y las pestanas que coincide con todas las
/// palabras escritas (en el titulo o la direccion, sin importar acentos).
fn local_matches(st: &AppState, visits: &[Visit], query: &str, skip: &HashSet<String>, limit: usize) -> Vec<Item> {
    let tokens: Vec<String> = fold(query).split_whitespace().map(str::to_string).collect();
    if tokens.is_empty() {
        return Vec::new();
    }
    // clave -> (fila, puntaje)
    let mut found: HashMap<String, (Item, f64)> = HashMap::new();
    let mut add = |kind: Kind, url: &str, title: &str, score: f64, tab_id: Option<u64>| {
        let key = dedupe_key(url);
        match found.get_mut(&key) {
            Some((item, s)) => {
                // Pestana > marcador > historial; el puntaje se suma.
                *s += score;
                if kind == Kind::Tab || (kind == Kind::Bookmark && item.kind == Kind::History) {
                    item.kind = kind;
                    item.tab_id = tab_id;
                    if !title.is_empty() {
                        item.title = title.to_string();
                    }
                }
            }
            None => {
                found.insert(key, (row(st, kind, url, title, tab_id), score));
            }
        }
    };
    for v in visits {
        add(Kind::History, &v.url, &v.title, v.score, None);
    }
    for b in &st.bookmarks {
        if is_web(&b.url) {
            add(Kind::Bookmark, &b.url, &b.title, 2.0, None);
        }
    }
    for (i, t) in st.tabs.iter().enumerate() {
        // Las de incognito solo se sugieren desde otra de incognito.
        if i != st.active && is_web(&t.url) && (!t.incognito || st.active_is_incognito()) {
            add(Kind::Tab, &t.url, &t.title, 3.0, Some(t.id));
        }
    }

    let mut ranked: Vec<(Item, f64)> = found
        .into_iter()
        .filter(|(key, _)| !skip.contains(key))
        .filter_map(|(_, (item, score))| {
            let title = fold(&item.title);
            let address = fold(item.url.split_once("://").map_or(item.url.as_str(), |(_, r)| r));
            let hay = format!("{title} {address}");
            if !tokens.iter().all(|t| hay.contains(t.as_str())) {
                return None;
            }
            let first = &tokens[0];
            let host = split_url(&item.url).map(|(_, h)| bare(&h).to_string()).unwrap_or_default();
            let mut bonus = 0.0;
            if host.starts_with(first.as_str()) {
                bonus += 6.0;
            }
            if title.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(first.as_str())) {
                bonus += 2.0;
            }
            Some((item, score + bonus))
        })
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    ranked.into_iter().take(limit).map(|(item, _)| item).collect()
}

/// Los sitios mas visitados (uno por dominio), para la barra vacia.
fn top_sites(st: &AppState, visits: &[Visit]) -> Vec<Item> {
    let mut sorted: Vec<&Visit> = visits.iter().collect();
    sorted.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut hosts = HashSet::new();
    let mut items = Vec::new();
    for v in sorted {
        let Some((_, host)) = split_url(&v.url) else { continue };
        if hosts.insert(bare(&host).to_string()) {
            items.push(row(st, Kind::History, &v.url, &v.title, None));
            if items.len() == MAX_TOP_SITES {
                break;
            }
        }
    }
    items
}

fn row(st: &AppState, kind: Kind, url: &str, title: &str, tab_id: Option<u64>) -> Item {
    let detail = display_url(url);
    Item {
        kind,
        title: if title.is_empty() { detail.clone() } else { title.to_string() },
        detail,
        icon: st.suggest.favicons.get(url),
        header: None,
        url: url.to_string(),
        fill: url.to_string(),
        tab_id,
    }
}

// ---------------------------------------------------------------------------
// Sugerencias del buscador (red)
// ---------------------------------------------------------------------------

/// Pide al buscador sugerencias para `query`, en otro hilo y un instante
/// despues (si se sigue escribiendo, el pedido se descarta antes de salir).
fn fetch_engine(state: &Shared, seq: u64, query: String) {
    let (enabled, engine_id, proxy, latest) = {
        let st = state.borrow();
        (st.settings.search_suggestions, st.settings.search_engine.clone(), st.proxy.clone(), st.suggest.seq.clone())
    };
    let q = query.trim();
    if !enabled || q.is_empty() || q.chars().count() > 100 || !is_search(q, &engine_id) {
        return;
    }
    let (Some(url), Some(proxy)) = (endpoint(&engine_id, q), proxy) else { return };
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(120));
        if latest.load(Ordering::SeqCst) != seq {
            return;
        }
        let list = request(&url);
        if latest.load(Ordering::SeqCst) != seq || list.is_empty() {
            return;
        }
        let _ = proxy.send_event(UserEvent::SearchSuggestions { seq, query, list });
    });
}

/// Direccion de sugerencias (formato OpenSearch) de cada buscador.
fn endpoint(engine_id: &str, query: &str) -> Option<String> {
    let q = storage::percent_encode(query);
    Some(match engine_id {
        "google" => format!("https://suggestqueries.google.com/complete/search?client=firefox&ie=utf-8&oe=utf-8&q={q}"),
        "bing" => format!("https://api.bing.com/osjson.aspx?query={q}"),
        "duckduckgo" => format!("https://duckduckgo.com/ac/?type=list&q={q}"),
        "brave" => format!("https://search.brave.com/api/suggest?q={q}"),
        "ecosia" => format!("https://ac.ecosia.org/autocomplete?type=list&q={q}"),
        _ => return None,
    })
}

fn request(url: &str) -> Vec<String> {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    let agent = AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout(Duration::from_millis(2500))
            .https_only(true)
            .user_agent("Michi")
            .build()
    });
    let Ok(response) = agent.get(url).call() else { return Vec::new() };
    let mut body = Vec::new();
    if response.into_reader().take(64 * 1024).read_to_end(&mut body).is_err() {
        return Vec::new();
    }
    parse_suggestions(&body)
}

/// ["consulta", ["sugerencia 1", ...]] (OpenSearch), o una lista de
/// {"phrase": ...} (DuckDuckGo sin type=list), o {"suggestions": [...]}.
fn parse_suggestions(body: &[u8]) -> Vec<String> {
    let Ok(v) = serde_json::from_slice::<Value>(body) else { return Vec::new() };
    let list = v
        .get(1)
        .and_then(Value::as_array)
        .or_else(|| v.get("suggestions").and_then(Value::as_array))
        .or_else(|| v.as_array().filter(|a| a.first().is_some_and(Value::is_object)));
    list.map(|items| {
        items
            .iter()
            .filter_map(|s| s.as_str().or_else(|| s.get("phrase").and_then(Value::as_str)))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s.chars().count() <= 200)
            .take(10)
            .collect()
    })
    .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Logos de los sitios
// ---------------------------------------------------------------------------

/// Guarda el logo de la pagina `url` (ver tabs::attach_favicon).
pub fn remember_favicon(state: &Shared, url: &str, icon: &Option<String>) {
    let (Some(icon), true) = (icon, is_web(url)) else { return };
    let Some((_, host)) = split_url(url) else { return };
    let mut st = state.borrow_mut();
    let changed = st.suggest.favicons.put(bare(&host).to_string(), icon.clone());
    // Con "borrar datos al cerrar" no queda en el disco.
    if changed && !st.settings.clear_on_exit {
        storage::save(storage::FAVICONS_FILE, &st.suggest.favicons);
    }
}

/// Se borra el historial: los logos dicen que sitios se visitaron.
pub fn forget_favicons(state: &Shared) {
    state.borrow_mut().suggest.favicons = Favicons::default();
    storage::remove(storage::FAVICONS_FILE);
}

// ---------------------------------------------------------------------------
// Funciones puras
// ---------------------------------------------------------------------------

fn is_web(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// Lo escrito es una busqueda (y no algo con forma de direccion).
fn is_search(query: &str, engine_id: &str) -> bool {
    let q = query.trim();
    storage::resolve_input(q, engine_id).is_none_or(|u| u == storage::search_url(engine_id, q))
}

/// ("https", "www.youtube.com") de una URL; el dominio en minusculas, con
/// puerto y sin "usuario@".
fn split_url(url: &str) -> Option<(&str, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    (!host.is_empty()).then(|| (scheme, host.to_ascii_lowercase()))
}

fn bare(host: &str) -> &str {
    host.strip_prefix("www.").unwrap_or(host)
}

/// Como la muestra Firefox: sin https:// ni www., sin la "/" final de una
/// portada; http:// si se ve (no es seguro).
pub fn display_url(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let rest = match rest.strip_suffix('/') {
        Some(r) if !r.contains('/') => r,
        _ => rest,
    };
    if scheme.is_empty() || scheme.eq_ignore_ascii_case("https") {
        rest.to_string()
    } else {
        format!("{scheme}://{rest}")
    }
}

/// Misma pagina aunque cambie http/https, "www." o la "/" final.
fn dedupe_key(url: &str) -> String {
    let d = display_url(url).to_lowercase();
    let d = d.strip_prefix("http://").unwrap_or(&d);
    d.trim_end_matches('/').to_string()
}

/// Minusculas y sin acentos, para comparar ("Cancion" encuentra "Canción").
fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            c => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(title: &str, url: &str) -> HistoryEntry {
        HistoryEntry { title: title.into(), url: url.into() }
    }

    #[test]
    fn urls_display_like_firefox() {
        assert_eq!(display_url("https://www.youtube.com/"), "youtube.com");
        assert_eq!(display_url("https://claude.ai/new"), "claude.ai/new");
        assert_eq!(display_url("http://localhost:5173/"), "http://localhost:5173");
        assert_eq!(display_url("https://github.com/a/b/"), "github.com/a/b/");
        assert_eq!(dedupe_key("http://www.example.com/"), dedupe_key("https://example.com"));
    }

    #[test]
    fn autofill_picks_the_most_visited_site() {
        let history = vec![
            h("YouTube", "https://www.youtube.com/"),
            h("Video", "https://www.youtube.com/watch?v=1"),
            h("YouTube", "https://www.youtube.com/"),
            h("Yahoo", "https://yahoo.com/"),
        ];
        let visits = aggregate(&history);
        let af = autofill(&visits, "y").unwrap();
        assert_eq!(af.text, "youtube.com/");
        assert_eq!(af.url, "https://www.youtube.com/");
        assert_eq!(af.title, "YouTube");
        assert_eq!(autofill(&visits, "yah").unwrap().text, "yahoo.com/");
        // Lo que no es el comienzo de un dominio no se completa.
        assert!(autofill(&visits, "tube").is_none());
        assert!(autofill(&visits, "you tube").is_none());
        assert!(autofill(&visits, "youtube.com/wat").is_none());
        // Con puerto, y respetando el "www." escrito.
        let local = vec![h("App", "http://localhost:5173/")];
        assert_eq!(autofill(&aggregate(&local), "loc").unwrap().text, "localhost:5173/");
        assert_eq!(autofill(&visits, "www.you").unwrap().text, "www.youtube.com/");
    }

    #[test]
    fn only_searches_go_to_the_search_engine() {
        assert!(is_search("yout", "google"));
        assert!(is_search("como hacer pan", "google"));
        assert!(!is_search("youtube.com", "google"));
        assert!(!is_search("localhost:5173", "google"));
        assert!(!is_search("192.168.0.1", "google"));
        assert!(!is_search("https://ejemplo.com/x", "google"));
    }

    #[test]
    fn parses_every_engine_format() {
        assert_eq!(parse_suggestions(br#"["yout",["youtube","youtube music"]]"#), vec!["youtube", "youtube music"]);
        assert_eq!(parse_suggestions(br#"[{"phrase":"youtube"},{"phrase":"y2mate"}]"#), vec!["youtube", "y2mate"]);
        assert_eq!(parse_suggestions(br#"{"query":"y","suggestions":["yahoo"]}"#), vec!["yahoo"]);
        assert!(parse_suggestions(b"<html>no</html>").is_empty());
        assert!(parse_suggestions(br#"["q",[1,2]]"#).is_empty());
    }

    #[test]
    fn folding_ignores_case_and_accents() {
        assert_eq!(fold("Canción ÁRBOL Ñandú"), "cancion arbol nandu");
    }

    #[test]
    fn favicons_keep_a_bounded_cache() {
        let mut f = Favicons::default();
        assert!(f.put("a.com".into(), "x".into()));
        assert!(!f.put("a.com".into(), "x".into()));
        assert!(f.put("a.com".into(), "y".into()));
        for i in 0..MAX_FAVICONS + 10 {
            f.put(format!("s{i}.com"), "z".into());
        }
        assert_eq!(f.icons.len(), MAX_FAVICONS);
        // Se fueron los mas viejos; quedan los ultimos.
        assert!(f.get("https://a.com/").is_none());
        assert_eq!(f.get("https://www.s405.com/x").as_deref(), Some("z"));
    }
}
