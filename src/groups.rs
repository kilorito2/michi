//! Grupos de pestanas, como en Chrome: varias pestanas juntas bajo un nombre
//! y un color en el panel, que se pueden plegar para que ocupen una fila.
//!
//! Reglas que se mantienen siempre (ver `normalize`):
//! - Las pestanas de un grupo van juntas en `AppState::tabs`.
//! - Una pestana fijada no tiene grupo (fijarla la saca).
//! - Un grupo sin pestanas deja de existir.

use crate::state::{AppState, Shared, TabGroup};
use crate::{sync, tabs};

/// (id, color). El primero libre es el de un grupo nuevo.
pub const COLORS: &[(&str, &str)] = &[
    ("azul", "#6c9eff"),
    ("rojo", "#ff6b6b"),
    ("amarillo", "#ffd166"),
    ("verde", "#6fdc8c"),
    ("rosa", "#ff7eb6"),
    ("violeta", "#b08cff"),
    ("celeste", "#5ed6e8"),
    ("naranja", "#ff9f5a"),
    ("gris", "#9aa0a6"),
];

pub fn is_color(id: &str) -> bool {
    COLORS.iter().any(|(c, _)| *c == id)
}

/// El nombre de un grupo, prolijo y no muy largo ("" = sin nombre, solo el color).
fn group_name(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(40).collect()
}

/// Deja todo segun las reglas de arriba, sin cambiar la pestana activa.
pub fn normalize(st: &mut AppState) {
    for t in st.tabs.iter_mut().filter(|t| t.pinned) {
        t.group = None;
    }
    let active_id = st.active_tab().map(|t| t.id);
    // Cada grupo queda donde esta su primera pestana, con todas las suyas ahi.
    let mut order: Vec<usize> = Vec::with_capacity(st.tabs.len());
    let mut placed: Vec<u64> = Vec::new();
    for i in 0..st.tabs.len() {
        match st.tabs[i].group {
            None => order.push(i),
            Some(g) if !placed.contains(&g) => {
                placed.push(g);
                order.extend((i..st.tabs.len()).filter(|&j| st.tabs[j].group == Some(g)));
            }
            Some(_) => {}
        }
    }
    if order.iter().enumerate().any(|(k, &i)| k != i) {
        let mut old: Vec<Option<crate::state::Tab>> = std::mem::take(&mut st.tabs).into_iter().map(Some).collect();
        st.tabs = order.into_iter().filter_map(|i| old[i].take()).collect();
    }
    if let Some(i) = active_id.and_then(|a| st.tabs.iter().position(|t| t.id == a)) {
        st.active = i;
    }
    let used: Vec<u64> = st.tabs.iter().filter_map(|t| t.group).collect();
    st.groups.retain(|g| used.contains(&g.id));
}

/// Despues de mover la pestana `id` (ver tabs::move_tab): si quedo entre dos
/// pestanas de un mismo grupo, entra a ese grupo; si era de un grupo y quedo
/// en su borde, sigue en el; si no, sale.
pub fn after_move(st: &mut AppState, id: u64) {
    let Some(i) = st.tabs.iter().position(|t| t.id == id) else { return };
    let above = i.checked_sub(1).and_then(|j| st.tabs.get(j)).and_then(|t| t.group);
    let below = st.tabs.get(i + 1).and_then(|t| t.group);
    let own = st.tabs[i].group;
    st.tabs[i].group = match (above, below) {
        (Some(a), Some(b)) if a == b => Some(a),
        _ if own.is_some() && (above == own || below == own) => own,
        _ => None,
    };
    normalize(st);
}

/// Pone la pestana `id` en el grupo `group` (al final de el).
fn put_in(st: &mut AppState, id: u64, group: u64) {
    let Some(from) = st.tabs.iter().position(|t| t.id == id) else { return };
    if st.tabs[from].pinned {
        return;
    }
    let active_id = st.active_tab().map(|t| t.id);
    let mut tab = st.tabs.remove(from);
    tab.group = Some(group);
    let at = st.tabs.iter().rposition(|t| t.group == Some(group)).map_or(from.min(st.tabs.len()), |last| last + 1);
    st.tabs.insert(at, tab);
    if let Some(i) = active_id.and_then(|a| st.tabs.iter().position(|t| t.id == a)) {
        st.active = i;
    }
    normalize(st);
}

fn done(state: &Shared) {
    state.borrow().save_session();
    sync::push_tabs(state);
}

/// "Agregar a un grupo nuevo": el grupo nace con el primer color libre y sin
/// nombre (el panel pide uno en el momento). Devuelve su id.
pub fn create(state: &Shared, tab_id: u64) -> Option<u64> {
    let id = {
        let mut st = state.borrow_mut();
        if st.tabs.iter().find(|t| t.id == tab_id).is_none_or(|t| t.pinned) {
            return None;
        }
        let used: Vec<String> = st.groups.iter().map(|g| g.color.clone()).collect();
        let color = COLORS
            .iter()
            .map(|(c, _)| c.to_string())
            .find(|c| !used.contains(c))
            .unwrap_or_else(|| COLORS[st.groups.len() % COLORS.len()].0.to_string());
        let id = st.alloc_id();
        st.groups.push(TabGroup { id, name: String::new(), color, collapsed: false });
        put_in(&mut st, tab_id, id);
        id
    };
    done(state);
    Some(id)
}

pub fn add_to(state: &Shared, tab_id: u64, group: u64) {
    {
        let mut st = state.borrow_mut();
        if !st.groups.iter().any(|g| g.id == group) {
            return;
        }
        put_in(&mut st, tab_id, group);
    }
    done(state);
}

/// "Quitar del grupo": la pestana queda justo despues del grupo.
pub fn remove_from(state: &Shared, tab_id: u64) {
    {
        let mut st = state.borrow_mut();
        let Some(from) = st.tabs.iter().position(|t| t.id == tab_id) else { return };
        let Some(group) = st.tabs[from].group else { return };
        let active_id = st.active_tab().map(|t| t.id);
        let mut tab = st.tabs.remove(from);
        tab.group = None;
        let at = st.tabs.iter().rposition(|t| t.group == Some(group)).map_or(from, |last| last + 1);
        st.tabs.insert(at, tab);
        if let Some(i) = active_id.and_then(|a| st.tabs.iter().position(|t| t.id == a)) {
            st.active = i;
        }
        normalize(&mut st);
    }
    done(state);
}

/// Nombre, color o plegado de un grupo (lo que venga).
pub fn update(state: &Shared, group: u64, name: Option<String>, color: Option<String>, collapsed: Option<bool>) {
    let switched = {
        let mut st = state.borrow_mut();
        let active_group = st.active_tab().and_then(|t| t.group);
        // Plegar el grupo de la pestana que se esta viendo pasa a la mas
        // cercana fuera de el (como Chrome); si no hay ninguna, no se pliega.
        let mut switched = false;
        let mut collapsed = collapsed;
        if collapsed == Some(true) && active_group == Some(group) {
            let after = st.tabs.iter().rposition(|t| t.group == Some(group)).map(|i| i + 1);
            let outside = after
                .filter(|&i| i < st.tabs.len())
                .or_else(|| st.tabs.iter().position(|t| t.group == Some(group)).and_then(|i| i.checked_sub(1)));
            match outside {
                Some(i) => {
                    st.active = i;
                    switched = true;
                }
                None => collapsed = None,
            }
        }
        let Some(g) = st.groups.iter_mut().find(|g| g.id == group) else { return };
        if let Some(name) = name {
            g.name = group_name(&name);
        }
        if let Some(color) = color.filter(|c| is_color(c)) {
            g.color = color;
        }
        if let Some(c) = collapsed {
            g.collapsed = c;
        }
        switched
    };
    if switched {
        tabs::activated(state);
    }
    done(state);
}

/// Al pasar a una pestana de un grupo plegado, el grupo se despliega.
pub fn reveal_active(st: &mut AppState) {
    let Some(group) = st.active_tab().and_then(|t| t.group) else { return };
    if let Some(g) = st.groups.iter_mut().find(|g| g.id == group) {
        g.collapsed = false;
    }
}

/// "Desagrupar": las pestanas quedan donde estan, sueltas.
pub fn ungroup(state: &Shared, group: u64) {
    {
        let mut st = state.borrow_mut();
        for t in st.tabs.iter_mut().filter(|t| t.group == Some(group)) {
            t.group = None;
        }
        normalize(&mut st);
    }
    done(state);
}

/// "Cerrar el grupo": todas sus pestanas.
pub fn close(state: &Shared, group: u64) {
    let ids: Vec<u64> = state.borrow().tabs.iter().filter(|t| t.group == Some(group)).map(|t| t.id).collect();
    for id in ids {
        tabs::close_tab(state, id);
    }
}

/// Al restaurar la sesion: arma los grupos guardados y les pone sus pestanas
/// (`tab_ids` en el orden de `session.urls`).
pub fn restore(state: &Shared, session: &crate::storage::Session, tab_ids: &[u64]) {
    let mut st = state.borrow_mut();
    let ids: Vec<u64> = session
        .groups
        .iter()
        .map(|g| {
            let id = st.alloc_id();
            let color = if is_color(&g.color) { g.color.clone() } else { COLORS[0].0.to_string() };
            st.groups.push(TabGroup { id, name: group_name(&g.name), color, collapsed: g.collapsed });
            id
        })
        .collect();
    for (tab_id, group) in tab_ids.iter().zip(&session.tab_groups) {
        let group = group.and_then(|i| ids.get(i).copied());
        if let Some(t) = st.tabs.iter_mut().find(|t| t.id == *tab_id) {
            t.group = group;
        }
    }
    normalize(&mut st);
    reveal_active(&mut st);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_names_are_tidy() {
        assert_eq!(group_name("  Trabajo   de  hoy "), "Trabajo de hoy");
        assert_eq!(group_name(&"x".repeat(80)).chars().count(), 40);
        assert!(is_color("verde") && !is_color("#fff"));
    }
}
