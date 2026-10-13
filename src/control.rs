//! Control de Michi desde otra aplicacion del usuario (May, la asistente de
//! voz): listar, abrir, cambiar y cerrar pestanas, navegar, traer la ventana
//! al frente y mandarle comandos del protocolo DevTools (CDP) a una pestana
//! puntual, que es lo que permite leer la pagina, hacer clic, escribir o
//! sacar una captura.
//!
//! Por un pipe con nombre (nunca desde la red), un pedido por conexion:
//! el cliente escribe una linea JSON {"cmd": ...} y recibe una linea JSON
//! {"ok": true, ...} o {"ok": false, "error": "..."}. Los comandos CDP van
//! por ICoreWebView2::CallDevToolsProtocolMethod de la pestana pedida: no se
//! abre ningun puerto de depuracion y cada comando cae en la pestana justa
//! (incognito incluida).
//!
//! Para encontrarlo, el principal deja `control.json` en la carpeta de datos
//! con el nombre del pipe, su pid y la ruta del .exe. Se apaga con el ajuste
//! "Permitir que May controle Michi" (Privacidad y seguridad).

use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use serde_json::{json, Value};
use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
use windows_core::HSTRING;
use winit::event_loop::EventLoopProxy;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{FlushFileBuffers, ReadFile, WriteFile, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use wry::WebViewExtWindows;

use crate::state::{is_own_page, Shared, UserEvent, HOME_LOAD_URL, HOME_URL};
use crate::{storage, tabs, win};

/// Archivo (en la carpeta de datos) por el que los clientes encuentran el pipe.
pub const DISCOVERY_FILE: &str = "control.json";
/// Lo maximo que se acepta en un pedido (un script o un CDP grande entra).
const MAX_REQUEST: usize = 4 * 1024 * 1024;
/// Cuanto se espera la respuesta del bucle de eventos (una captura o una
/// pagina pesada pueden tardar).
const REPLY_TIMEOUT: Duration = Duration::from_secs(60);

/// Un pedido que llego por el pipe, para resolverlo en el hilo de la interfaz.
#[derive(Debug, Clone)]
pub struct Request {
    pub body: Value,
    pub reply: Sender<Value>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn pipe_name() -> String {
    format!("\\\\.\\pipe\\Michi-control-{:016x}", crate::single_instance::install_id())
}

/// Arranca el pipe (en su propio hilo) y deja control.json. Solo el principal.
pub fn listen(proxy: EventLoopProxy<UserEvent>) {
    let name = pipe_name();
    write_discovery(&name);
    std::thread::spawn(move || {
        let wname = wide(&name);
        loop {
            let pipe = unsafe {
                CreateNamedPipeW(
                    wname.as_ptr(),
                    PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    PIPE_UNLIMITED_INSTANCES,
                    MAX_REQUEST as u32,
                    MAX_REQUEST as u32,
                    0,
                    std::ptr::null(),
                )
            };
            if pipe == INVALID_HANDLE_VALUE {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            let connected =
                unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            if !connected {
                unsafe { CloseHandle(pipe) };
                continue;
            }
            // Cada conexion en su hilo: un CDP lento no frena a los demas.
            let proxy = proxy.clone();
            let pipe = pipe as usize;
            std::thread::spawn(move || serve(pipe as HANDLE, &proxy));
        }
    });
}

fn write_discovery(pipe: &str) {
    let info = json!({
        "pipe": pipe,
        "pid": std::process::id(),
        "exe": std::env::current_exe().ok(),
        "version": env!("CARGO_PKG_VERSION"),
    });
    let dir = storage::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(DISCOVERY_FILE), info.to_string());
}

fn serve(pipe: HANDLE, proxy: &EventLoopProxy<UserEvent>) {
    let response = match read_line(pipe).and_then(|raw| serde_json::from_slice::<Value>(&raw).ok()) {
        None => error("pedido inválido"),
        Some(body) => {
            let (tx, rx) = mpsc::channel();
            if proxy.send_event(UserEvent::Control(Request { body, reply: tx })).is_err() {
                error("Michi se esta cerrando")
            } else {
                rx.recv_timeout(REPLY_TIMEOUT).unwrap_or_else(|_| error("Michi no respondió a tiempo"))
            }
        }
    };
    let mut out = response.to_string();
    out.push('\n');
    let mut written = 0u32;
    unsafe {
        WriteFile(pipe, out.as_ptr(), out.len() as u32, &mut written, std::ptr::null_mut());
        FlushFileBuffers(pipe);
        DisconnectNamedPipe(pipe);
        CloseHandle(pipe);
    }
}

/// Lee hasta el primer salto de linea (o hasta que el otro lado cierre).
fn read_line(pipe: HANDLE) -> Option<Vec<u8>> {
    let mut data = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let mut read = 0u32;
        let ok = unsafe { ReadFile(pipe, buf.as_mut_ptr(), buf.len() as u32, &mut read, std::ptr::null_mut()) } != 0;
        data.extend_from_slice(&buf[..read as usize]);
        if let Some(end) = data.iter().position(|b| *b == b'\n') {
            data.truncate(end);
            return Some(data);
        }
        if !ok || read == 0 {
            return (!data.is_empty()).then_some(data);
        }
        if data.len() > MAX_REQUEST {
            return None;
        }
    }
}

fn error(message: &str) -> Value {
    json!({ "ok": false, "error": message })
}

fn ok(mut extra: Value) -> Value {
    if let Some(map) = extra.as_object_mut() {
        map.insert("ok".into(), Value::Bool(true));
    }
    extra
}

/// En el hilo de la interfaz (UserEvent::Control), fuera de cualquier
/// callback de WebView2: aca si se pueden crear y cerrar pestanas.
pub fn handle(state: &Shared, req: Request) {
    if !state.borrow().settings.allow_assistant {
        let _ = req.reply.send(error("Michi tiene apagado el control desde May (Ajustes > Privacidad y seguridad)."));
        return;
    }
    if state.borrow().window.is_none() {
        let _ = req.reply.send(error("Michi todavía está arrancando"));
        return;
    }
    let body = &req.body;
    let cmd = body.get("cmd").and_then(Value::as_str).unwrap_or("");
    let tab = body.get("tab").and_then(Value::as_u64);
    let response = match cmd {
        "hello" => ok(json!({ "version": env!("CARGO_PKG_VERSION"), "pid": std::process::id() })),
        "tabs" => ok(tabs_json(state)),
        "focus" => {
            win::force_to_front();
            ok(json!({}))
        }
        "open" => {
            let incognito = body.get("incognito").and_then(Value::as_bool).unwrap_or(false);
            match resolve(state, body) {
                None => error("dirección inválida"),
                Some(url) => {
                    tabs::open_tab(state, &url, incognito);
                    if body.get("focus").and_then(Value::as_bool).unwrap_or(true) {
                        win::force_to_front();
                    }
                    let id = state.borrow().active_tab().map(|t| t.id);
                    ok(json!({ "id": id, "url": url }))
                }
            }
        }
        "navigate" => {
            match (resolve(state, body), core_of(state, tab)) {
                (None, _) => error("dirección inválida"),
                (_, None) => error("no existe esa pestaña"),
                (Some(url), Some((id, core))) => {
                    // En una pestana que ya existe, la propia va con su forma http.
                    let url = if url == HOME_URL { HOME_LOAD_URL.to_string() } else { url };
                    let _ = unsafe { core.Navigate(&HSTRING::from(url.as_str())) };
                    ok(json!({ "id": id, "url": url }))
                }
            }
        }
        "activate" => match find(state, tab) {
            None => error("no existe esa pestaña"),
            Some(i) => {
                let changed = {
                    let mut st = state.borrow_mut();
                    let changed = st.active != i;
                    st.active = i;
                    changed
                };
                if changed {
                    tabs::activated(state);
                }
                if body.get("focus").and_then(Value::as_bool).unwrap_or(true) {
                    win::force_to_front();
                }
                ok(json!({}))
            }
        },
        "close" => match find(state, tab).map(|i| state.borrow().tabs[i].id) {
            None => error("no existe esa pestaña"),
            Some(id) => {
                tabs::close_tab(state, id);
                ok(json!({}))
            }
        },
        "back" | "forward" | "reload" | "stop" => match core_of(state, tab) {
            None => error("no existe esa pestaña"),
            Some((_, core)) => {
                let _ = unsafe {
                    match cmd {
                        "back" => core.GoBack(),
                        "forward" => core.GoForward(),
                        "reload" => core.Reload(),
                        _ => core.Stop(),
                    }
                };
                ok(json!({}))
            }
        },
        "cdp" => {
            let method = body.get("method").and_then(Value::as_str).unwrap_or("").to_string();
            let params = body.get("params").cloned().unwrap_or_else(|| json!({}));
            match core_of(state, tab) {
                None => error("no existe esa pestaña"),
                Some(_) if method.is_empty() => error("falta el método"),
                Some((_, core)) => {
                    // La respuesta llega despues, en el callback.
                    call_cdp(&core, &method, &params, req.reply);
                    return;
                }
            }
        }
        _ => error("comando desconocido"),
    };
    let _ = req.reply.send(response);
}

/// "search": lo que se busca con el buscador elegido. "url": texto escrito
/// o URL, como la barra de direcciones (dominios con https://, lo demas al
/// buscador). La pestana nueva propia se pide con "" o "about:newtab".
fn resolve(state: &Shared, body: &Value) -> Option<String> {
    let engine = state.borrow().settings.search_engine.clone();
    if let Some(query) = body.get("search").and_then(Value::as_str).map(str::trim).filter(|q| !q.is_empty()) {
        return Some(storage::search_url(&engine, query));
    }
    let input = body.get("url").and_then(Value::as_str).unwrap_or("").trim();
    if input.is_empty() || input == "about:newtab" {
        return Some(HOME_URL.to_string());
    }
    let url = storage::resolve_input(input, &engine)?;
    let lower = url.to_ascii_lowercase();
    // Nada que ejecute codigo en la pagina por la barra.
    (!lower.starts_with("javascript:") && !lower.starts_with("data:")).then_some(url)
}

fn tabs_json(state: &Shared) -> Value {
    let st = state.borrow();
    let list: Vec<Value> = st
        .tabs
        .iter()
        .enumerate()
        .map(|(i, t)| {
            json!({
                "id": t.id,
                "title": t.title,
                "url": t.url,
                "active": i == st.active,
                "incognito": t.incognito,
                "loading": t.loading,
                "playing": t.playing.any(),
                "own": is_own_page(&t.url),
            })
        })
        .collect();
    json!({ "tabs": list, "active": st.active_tab().map(|t| t.id) })
}

/// Indice de la pestana (None = la del frente).
fn find(state: &Shared, tab: Option<u64>) -> Option<usize> {
    let st = state.borrow();
    match tab {
        None => (!st.tabs.is_empty()).then_some(st.active),
        Some(id) => st.tabs.iter().position(|t| t.id == id),
    }
}

/// El puntero COM de la pestana: el prestamo de `state` se suelta antes de
/// usarlo (navegar dispara eventos que lo vuelven a pedir).
fn core_of(state: &Shared, tab: Option<u64>) -> Option<(u64, webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2)> {
    let i = find(state, tab)?;
    let st = state.borrow();
    let t = &st.tabs[i];
    Some((t.id, t.webview.webview()))
}

fn call_cdp(
    core: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    method: &str,
    params: &Value,
    reply: Sender<Value>,
) {
    let sender = reply.clone();
    let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, text| {
        // Con error, WebView2 devuelve igual el JSON del error de CDP.
        let parsed = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
        let response = match result {
            Ok(()) => json!({ "ok": true, "result": parsed }),
            Err(e) => {
                let message = parsed
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| e.message().to_string());
                json!({ "ok": false, "error": message })
            }
        };
        let _ = sender.send(response);
        Ok(())
    }));
    let started = unsafe {
        core.CallDevToolsProtocolMethod(&HSTRING::from(method), &HSTRING::from(params.to_string()), &handler)
    };
    if let Err(e) = started {
        let _ = reply.send(error(&e.message()));
    }
}
