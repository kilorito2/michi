//! El menu del clic derecho de las paginas: el de WebView2 (copiar, pegar,
//! guardar imagen, inspeccionar...) mas lo que trae cualquier navegador y
//! WebView2 no tiene:
//!
//! - Sobre un enlace: "Abrir enlace en una pestana nueva" (atras, junto a la
//!   actual, como en Chrome) y "...en una pestana de incognito". Reemplazan a
//!   "Abrir vinculo en una ventana nueva" de WebView2, que en Michi abria una
//!   pestana al frente.
//! - Sobre una imagen: "Abrir imagen en una pestana nueva".
//! - Con texto seleccionado: "Buscar "..." en Google" (el buscador de Ajustes).

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{ContextMenuRequestedEventHandler, CustomItemSelectedEventHandler};
use windows_core::{Interface, BOOL, HSTRING};
use wry::{WebView, WebViewExtWindows};

use crate::native::read_pwstr;
use crate::state::Shared;
use crate::{security, storage};

/// Lo que hace cada opcion agregada.
#[derive(Clone)]
enum Choice {
    /// Pestana nueva atras (la de incognito si la pagina lo es).
    Background(String),
    Incognito(String),
    Search(String),
}

/// Lo que se ve de la seleccion en "Buscar ...": corto y en una linea.
fn short(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(32) {
        Some((i, _)) => format!("{}…", flat[..i].trim_end()),
        None => flat,
    }
}

pub fn attach(state: &Shared, webview: &WebView, id: u64) {
    let core = webview.webview();
    let Ok(core11) = core.cast::<ICoreWebView2_11>() else { return };
    let env = unsafe { core.cast::<ICoreWebView2_2>().and_then(|c| c.Environment()) };
    let Ok(env9) = env.and_then(|e| e.cast::<ICoreWebView2Environment9>()) else { return };

    let s = state.clone();
    let handler = ContextMenuRequestedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let target = unsafe { args.ContextMenuTarget()? };
        let items = unsafe { args.MenuItems()? };
        let flag = |f: &dyn Fn(*mut BOOL) -> windows_core::Result<()>| {
            let mut b = BOOL(0);
            f(&mut b).is_ok() && b.as_bool()
        };
        let (incognito, engine, page) = {
            let st = s.borrow();
            let tab = st.tabs.iter().find(|t| t.id == id);
            let incognito = tab.is_some_and(|t| t.incognito);
            let page = tab.map(|t| t.url.clone()).unwrap_or_default();
            (incognito, storage::engine(&st.settings.search_engine).1, page)
        };

        let mut extra: Vec<(String, Choice)> = Vec::new();
        if flag(&|p| unsafe { target.HasLinkUri(p) }) {
            let link = unsafe { read_pwstr(|p| target.LinkUri(p)) };
            // El mismo filtro que window.open (ver security::popup_target_allowed):
            // la pestana la abrimos nosotros, y para el motor eso es como si la
            // hubiera escrito el usuario. Sin el, un sitio podia hacer abrir
            // archivos locales o de una carpeta de red (file://servidor/...,
            // que le manda a ese servidor la credencial de Windows), nuestras
            // paginas internas o las de cualquier extension.
            if security::popup_target_allowed(&link, &page) {
                remove_named(&items, "openLinkInNewWindow");
                extra.push(("Abrir enlace en una pestaña nueva".into(), Choice::Background(link.clone())));
                if !incognito {
                    extra.push(("Abrir enlace en una pestaña de incógnito".into(), Choice::Incognito(link)));
                }
            }
        }
        let mut kind = COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND::default();
        let _ = unsafe { target.Kind(&mut kind) };
        // HasSourceUri a veces dice que no aunque la imagen tenga direccion:
        // se lee igual.
        if kind == COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_IMAGE {
            let src = unsafe { read_pwstr(|p| target.SourceUri(p)) };
            if !src.is_empty() && security::popup_target_allowed(&src, &page) {
                extra.push(("Abrir imagen en una pestaña nueva".into(), Choice::Background(src)));
            }
        }
        if flag(&|p| unsafe { target.HasSelection(p) }) {
            let text = unsafe { read_pwstr(|p| target.SelectionText(p)) };
            if !text.trim().is_empty() {
                extra.push((format!("Buscar \"{}\" en {engine}", short(&text)), Choice::Search(text)));
            }
        }
        if extra.is_empty() {
            return Ok(());
        }

        let mut count = 0u32;
        let _ = unsafe { items.Count(&mut count) };
        let mut at = 0u32;
        for (label, choice) in extra {
            let label = HSTRING::from(label);
            let item = unsafe {
                env9.CreateContextMenuItem(&label, None::<&windows::Win32::System::Com::IStream>, COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND)
            };
            let Ok(item) = item else { continue };
            let s2 = s.clone();
            let selected = CustomItemSelectedEventHandler::create(Box::new(move |_, _| {
                run(&s2, choice.clone(), incognito);
                Ok(())
            }));
            let mut token = 0i64;
            unsafe {
                let _ = item.add_CustomItemSelected(&selected, &mut token);
                if items.InsertValueAtIndex(at, &item).is_ok() {
                    at += 1;
                }
            }
        }
        // Una linea entre lo nuestro y lo de WebView2 (si ahi no hay ya una).
        let separator_next = unsafe { items.GetValueAtIndex(at) }.is_ok_and(|next| {
            let mut k = COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND::default();
            unsafe { next.Kind(&mut k) }.is_ok() && k == COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR
        });
        if at > 0 && count > 0 && !separator_next {
            let empty = HSTRING::new();
            if let Ok(sep) = unsafe {
                env9.CreateContextMenuItem(&empty, None::<&windows::Win32::System::Com::IStream>, COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR)
            } {
                let _ = unsafe { items.InsertValueAtIndex(at, &sep) };
            }
        }
        Ok(())
    }));
    let mut token = 0i64;
    let _ = unsafe { core11.add_ContextMenuRequested(&handler, &mut token) };
}

/// Saca del menu la opcion de WebView2 con ese nombre (si esta).
fn remove_named(items: &ICoreWebView2ContextMenuItemCollection, name: &str) {
    let mut count = 0u32;
    let _ = unsafe { items.Count(&mut count) };
    for i in (0..count).rev() {
        let Ok(item) = (unsafe { items.GetValueAtIndex(i) }) else { continue };
        if unsafe { read_pwstr(|p| item.Name(p)) } == name {
            let _ = unsafe { items.RemoveValueAtIndex(i) };
        }
    }
}

fn run(state: &Shared, choice: Choice, from_incognito: bool) {
    let mut st = state.borrow_mut();
    match choice {
        Choice::Background(url) => st.request_background_tab(&url, from_incognito),
        Choice::Incognito(url) => st.request_tab_in(&url, true),
        Choice::Search(text) => {
            let url = storage::search_url(&st.settings.search_engine, text.trim());
            st.request_tab_in(&url, from_incognito);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_selections_are_shortened_to_one_line() {
        assert_eq!(short("  hola\n  mundo "), "hola mundo");
        let long = "a".repeat(50);
        assert_eq!(short(&long), format!("{}…", "a".repeat(32)));
    }
}
