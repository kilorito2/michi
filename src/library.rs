//! El panel izquierdo como biblioteca: marcadores en carpetas (y editarlos),
//! y el historial con buscador, por dia, y borrando de a una pagina.
//!
//! Las carpetas son de un solo nivel, como las del panel: cada marcador dice
//! en cual esta (Bookmark::folder, "" = suelto) y la lista de carpetas se
//! guarda aparte (para que existan aunque esten vacias, y en su orden).

use serde_json::json;

use crate::state::Shared;
use crate::storage::Bookmark;

use crate::{security, suggest, sync};

/// Cuantas paginas del historial muestra el panel (las ultimas, o las que
/// coinciden con lo buscado).
const HISTORY_SHOWN: usize = 150;

/// El nombre de una carpeta, prolijo: sin espacios de mas y no muy largo.
pub fn folder_name(raw: &str) -> String {
    let flat = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.chars().take(60).collect()
}

/// La direccion que se escribio al editar un marcador: con https:// si no
/// tiene esquema; nada que no se pueda abrir con seguridad (javascript:...).
fn bookmark_url(raw: &str) -> Option<String> {
    let v = raw.trim();
    if v.is_empty() || v.contains(char::is_whitespace) {
        return None;
    }
    // Con esquema ("https://...", "javascript:...") tal cual, y que decida
    // is_safe_to_open; "localhost:3000" no es un esquema.
    let scheme = v.split_once(':').filter(|(s, rest)| {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_alphabetic()) && !rest.starts_with(|c: char| c.is_ascii_digit())
    });
    let url = if scheme.is_some() { v.to_string() } else { format!("https://{v}") };
    security::is_safe_to_open(&url).then_some(url)
}

/// Las carpetas en orden: las guardadas y, por las dudas, las que nombre
/// algun marcador y no esten en la lista.
fn folders(st: &crate::state::AppState) -> Vec<String> {
    let mut list = st.bookmark_folders.clone();
    for b in &st.bookmarks {
        if !b.folder.is_empty() && !list.contains(&b.folder) {
            list.push(b.folder.clone());
        }
    }
    list
}

pub fn push_bookmarks(state: &Shared) {
    let st = state.borrow();
    let Some(lp) = &st.left_panel else { return };
    let items: Vec<_> = st
        .bookmarks
        .iter()
        .map(|b| json!({ "title": b.title, "url": b.url, "folder": b.folder, "icon": st.suggest.favicons.get(&b.url) }))
        .collect();
    let data = json!({ "items": items, "folders": folders(&st) });
    let _ = lp.evaluate_script(&format!("window.onBookmarks && window.onBookmarks({data})"));
}

/// Cambia el nombre, la direccion o la carpeta del marcador de `url`.
pub fn update_bookmark(state: &Shared, url: &str, title: &str, new_url: &str, folder: &str) {
    let ok = {
        let mut st = state.borrow_mut();
        let new_url = bookmark_url(new_url);
        let folder = folder_name(folder);
        match (st.bookmarks.iter().position(|b| b.url == url), new_url) {
            (Some(i), Some(new_url)) => {
                // Si la direccion nueva ya era otro marcador, queda uno solo.
                if new_url != url {
                    st.bookmarks.retain(|b| b.url != new_url);
                }
                let i = st.bookmarks.iter().position(|b| b.url == url).unwrap_or(i);
                let title = title.trim();
                st.bookmarks[i] = Bookmark {
                    title: if title.is_empty() { new_url.clone() } else { title.to_string() },
                    url: new_url,
                    folder: folder.clone(),
                };
                if !folder.is_empty() && !st.bookmark_folders.contains(&folder) {
                    st.bookmark_folders.push(folder);
                    st.save_folders();
                }
                st.save_bookmarks();
                true
            }
            _ => false,
        }
    };
    if ok {
        sync::push_bookmarks(state);
        sync::push_active_tab(state);
        sync::push_menu(state);
    }
}

pub fn add_folder(state: &Shared, name: &str) {
    {
        let mut st = state.borrow_mut();
        let name = folder_name(name);
        if name.is_empty() || folders(&st).contains(&name) {
            return;
        }
        st.bookmark_folders.push(name);
        st.save_folders();
    }
    sync::push_bookmarks(state);
}

pub fn rename_folder(state: &Shared, from: &str, to: &str) {
    {
        let mut st = state.borrow_mut();
        let to = folder_name(to);
        if to.is_empty() || to == from {
            return;
        }
        // Con el nombre de otra carpeta que ya existe: se juntan.
        let mut list = folders(&st);
        list.retain(|f| f != &to);
        for f in list.iter_mut() {
            if f == from {
                *f = to.clone();
            }
        }
        st.bookmark_folders = list;
        for b in st.bookmarks.iter_mut().filter(|b| b.folder == from) {
            b.folder = to.clone();
        }
        st.save_folders();
        st.save_bookmarks();
    }
    sync::push_bookmarks(state);
}

/// Quita la carpeta; sus marcadores no se borran: quedan sueltos.
pub fn remove_folder(state: &Shared, name: &str) {
    {
        let mut st = state.borrow_mut();
        st.bookmark_folders.retain(|f| f != name);
        for b in st.bookmarks.iter_mut().filter(|b| b.folder == name) {
            b.folder.clear();
        }
        st.save_folders();
        st.save_bookmarks();
    }
    sync::push_bookmarks(state);
}

/// Lo que muestra el panel del historial: las ultimas paginas, o las que
/// coinciden con lo buscado (en el titulo o la direccion, sin importar
/// mayusculas ni acentos), de la mas nueva a la mas vieja. Cada pagina una
/// sola vez, en su visita mas reciente: volver tres veces a un sitio no lo
/// repite tres veces en la lista.
pub fn push_history(state: &Shared) {
    let st = state.borrow();
    let Some(lp) = &st.left_panel else { return };
    let query = suggest::fold(st.history_query.trim());
    let words: Vec<&str> = query.split_whitespace().collect();
    let mut seen = std::collections::HashSet::new();
    let items: Vec<_> = st
        .history
        .iter()
        .rev()
        .filter(|h| seen.insert(h.url.as_str()))
        .filter(|h| {
            words.is_empty() || {
                let hay = suggest::fold(&format!("{} {}", h.title, h.url));
                words.iter().all(|w| hay.contains(w))
            }
        })
        .take(HISTORY_SHOWN)
        .map(|h| json!({ "title": h.title, "url": h.url, "time": h.time, "icon": st.suggest.favicons.get(&h.url) }))
        .collect();
    let data = json!({ "items": items, "query": st.history_query, "total": st.history.len() });
    let _ = lp.evaluate_script(&format!("window.onHistory && window.onHistory({data})"));
}

pub fn search_history(state: &Shared, query: &str) {
    state.borrow_mut().history_query = query.chars().take(200).collect();
    push_history(state);
}

/// Saca del historial todas las visitas a esa pagina.
pub fn remove_history(state: &Shared, url: &str) {
    {
        let mut st = state.borrow_mut();
        st.history.retain(|h| h.url != url);
        st.save_history();
    }
    push_history(state);
    sync::push_own_pages(state);
}

/// "Borrar" del historial: todo, y lo que dice que sitios se visitaron.
pub fn clear_history(state: &Shared) {
    {
        let mut st = state.borrow_mut();
        st.history.clear();
        st.closed_tabs.clear();
        st.save_history();
    }
    suggest::forget_favicons(state);
    crate::zoom::forget_all(state);
    push_history(state);
    sync::push_own_pages(state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_are_tidy() {
        assert_eq!(folder_name("  Trabajo   y  estudio "), "Trabajo y estudio");
        assert_eq!(folder_name(&"x".repeat(80)).len(), 60);
    }

    #[test]
    fn edited_bookmark_addresses_are_safe() {
        assert_eq!(bookmark_url("github.com").as_deref(), Some("https://github.com"));
        assert_eq!(bookmark_url(" https://a.com/x ").as_deref(), Some("https://a.com/x"));
        assert_eq!(bookmark_url("javascript:alert(1)"), None);
        assert_eq!(bookmark_url("con espacios"), None);
        assert_eq!(bookmark_url(""), None);
    }
}
