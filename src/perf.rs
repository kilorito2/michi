//! Modos de rendimiento (Ajustes > Rendimiento): cuanto del navegador se
//! sacrifica para gastar menos memoria, procesador y GPU.
//!
//! - Normal: todo como siempre.
//! - Optimizado: el fondo animado se pausa cuando Michi no esta en primer
//!   plano (otra ventana al frente, o minimizado); las pestanas de atras
//!   usan menos memoria y, tras un rato sin usarlas, se duermen.
//! - Super optimizado: lo anterior pero mas rapido (las pestanas se duermen
//!   antes), el fondo es una imagen fija (no se decodifica ningun video) y la
//!   interfaz no anima nada (ver ui/wallpaper.js y panels.rs).
//!
//! Todo se cambia en caliente: no hace falta reiniciar. Lo que es de las
//! paginas del navegador (el fondo, las animaciones) lo decide
//! ui/wallpaper.js, que recibe el modo con window.__setPerf (ver
//! sync::push_perf); lo de las pestanas, este modulo.
//!
//! Una pestana "dormida" (ICoreWebView2_3::TrySuspend) deja de ejecutar la
//! pagina y libera casi toda su memoria; al volver a ella se reanuda sola. No
//! se duerme la que suena, reproduce video, esta cargando, es una pagina
//! propia o esta en pantalla completa.

use std::time::{Duration, Instant};

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2_19, ICoreWebView2_3, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
};
use webview2_com::TrySuspendCompletedHandler;
use windows_core::Interface;
use wry::WebViewExtWindows;

use crate::state::{is_own_page, AppState, Shared};
use crate::sync;

/// Cada cuanto se mira si Michi sigue en primer plano y si hay pestanas para
/// dormir (solo con un modo de rendimiento activo: en Normal no se mira nada).
const POLL: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Normal,
    Optimized,
    Super,
}

/// Los valores validos del ajuste `performance`.
pub const IDS: &[&str] = &["normal", "optimized", "super"];

impl Mode {
    /// Un valor desconocido (un ajuste viejo o roto) es Normal.
    pub fn parse(id: &str) -> Mode {
        match id {
            "optimized" => Mode::Optimized,
            "super" => Mode::Super,
            _ => Mode::Normal,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Mode::Normal => "normal",
            Mode::Optimized => "optimized",
            Mode::Super => "super",
        }
    }

    /// Cuanto tiempo en segundo plano antes de dormir una pestana (None = nunca).
    pub fn sleep_after(self) -> Option<Duration> {
        match self {
            Mode::Normal => None,
            Mode::Optimized => Some(Duration::from_secs(10 * 60)),
            Mode::Super => Some(Duration::from_secs(2 * 60)),
        }
    }

    /// Las pestanas de atras piden usar la menos memoria posible.
    pub fn background_low_memory(self) -> bool {
        self != Mode::Normal
    }

    /// El fondo animado se pausa con Michi fuera de primer plano.
    pub fn pause_in_background(self) -> bool {
        self != Mode::Normal
    }

    /// Los paneles laterales se abren y cierran de golpe.
    pub fn instant_panels(self) -> bool {
        self == Mode::Super
    }
}

pub fn mode(st: &AppState) -> Mode {
    Mode::parse(&st.settings.performance)
}

/// Lo que cada pestana recuerda para este modulo.
#[derive(Default)]
pub struct Rest {
    /// Desde cuando esta en segundo plano (None = es la que esta al frente).
    hidden_since: Option<Instant>,
    /// Dormida (TrySuspend) y todavia sin reanudar.
    suspended: bool,
    /// Ya se le pidio usar poca memoria.
    low_memory: bool,
}

/// Cambio la pestana al frente (o se creo una): las demas empiezan a contar
/// su descanso, la que vuelve al frente se despierta, y cada una recibe el
/// nivel de memoria que le toca.
pub fn on_activated(state: &Shared) {
    let mut wake_up = Vec::new();
    {
        let mut st = state.borrow_mut();
        let (active, now) = (st.active, Instant::now());
        for (i, tab) in st.tabs.iter_mut().enumerate() {
            if i == active {
                tab.rest.hidden_since = None;
                if std::mem::take(&mut tab.rest.suspended) {
                    eprintln!("[rendimiento] pestana {} reanudada", tab.id);
                    wake_up.push(tab.webview.webview());
                }
            } else if tab.rest.hidden_since.is_none() {
                tab.rest.hidden_since = Some(now);
            }
        }
    }
    // Sin prestamos vivos: reanudar una pagina dispara eventos del motor.
    resume(wake_up.into_iter());
    apply_memory(state);
    // Para que tick() haga la cuenta de nuevo.
    state.borrow().wake();
}

/// Ajustes > Rendimiento: cambio el modo. El fondo y las animaciones los
/// resuelve cada pagina (sync::push_perf); aca la memoria de las pestanas.
pub fn on_mode_changed(state: &Shared) {
    let (normal, wake_up) = {
        let mut st = state.borrow_mut();
        let normal = mode(&st) == Mode::Normal;
        if normal {
            // Nada que vigilar: el fondo vuelve a andar aunque Michi no este al frente.
            st.foreground = true;
        }
        // Sin modo que las duerma, ninguna pestana tiene por que seguir dormida
        // (un chat abierto de fondo tiene que volver a avisar).
        let sleeps = mode(&st).sleep_after().is_some();
        let mut wake_up = Vec::new();
        for tab in st.tabs.iter_mut().filter(|t| !sleeps && t.rest.suspended) {
            tab.rest.suspended = false;
            eprintln!("[rendimiento] pestana {} reanudada (cambio de modo)", tab.id);
            wake_up.push(tab.webview.webview());
        }
        (normal, wake_up)
    };
    resume(wake_up.into_iter());
    apply_memory(state);
    sync::push_perf(state);
    sync::push_media(state);
    if !normal {
        state.borrow().wake();
    }
}

fn resume(cores: impl Iterator<Item = webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2>) {
    for core in cores {
        if let Ok(core3) = core.cast::<ICoreWebView2_3>() {
            let _ = unsafe { core3.Resume() };
        }
    }
}

/// Memoria baja para las pestanas de atras (y normal para la de adelante),
/// segun el modo. Solo avisa al motor cuando una pestana cambia de nivel.
fn apply_memory(state: &Shared) {
    let mut changes = Vec::new();
    {
        let mut st = state.borrow_mut();
        let low_in_background = mode(&st).background_low_memory();
        let active = st.active;
        for (i, tab) in st.tabs.iter_mut().enumerate() {
            let low = low_in_background && i != active;
            if tab.rest.low_memory != low {
                tab.rest.low_memory = low;
                changes.push((tab.webview.webview(), low));
            }
        }
    }
    for (core, low) in changes {
        if let Ok(core19) = core.cast::<ICoreWebView2_19>() {
            let level = if low {
                COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
            } else {
                COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
            };
            let result = unsafe { core19.SetMemoryUsageTargetLevel(level) };
            eprintln!("[rendimiento] memoria {}: {result:?}", if low { "baja" } else { "normal" });
        }
    }
}

/// Se llama en cada vuelta del bucle de eventos (ver about_to_wait en
/// main.rs): vigila si Michi sigue en primer plano y duerme las pestanas que
/// llevan demasiado atras. Devuelve cuando volver a mirar (None en Normal).
pub fn tick(state: &Shared) -> Option<Instant> {
    let mode = mode(&state.borrow());
    if mode == Mode::Normal {
        return None;
    }
    refresh_foreground(state);

    let mut due = Vec::new();
    if let Some(after) = mode.sleep_after() {
        let mut st = state.borrow_mut();
        let (active, fullscreen, now) = (st.active, st.fullscreen, Instant::now());
        for (i, tab) in st.tabs.iter_mut().enumerate() {
            let Some(since) = tab.rest.hidden_since else { continue };
            if i == active || tab.rest.suspended || now < since + after {
                continue;
            }
            let busy = tab.playing.any() || tab.loading || is_own_page(&tab.url) || fullscreen == Some(tab.id);
            if busy {
                continue;
            }
            // Si no se deja dormir (el motor se niega), se vuelve a intentar
            // despues de otro tanto, no en cada vuelta.
            tab.rest.hidden_since = Some(now);
            due.push((tab.id, tab.webview.webview()));
        }
    }
    for (id, core) in due {
        suspend(state, id, core);
    }
    Some(Instant::now() + POLL)
}

fn suspend(state: &Shared, id: u64, core: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2) {
    let Ok(core3) = core.cast::<ICoreWebView2_3>() else { return };
    let s = state.clone();
    let handler = TrySuspendCompletedHandler::create(Box::new(move |result, suspended| {
        if result.is_ok() && suspended {
            let mut st = s.borrow_mut();
            if let Some(tab) = st.tabs.iter_mut().find(|t| t.id == id) {
                // Pudo volver al frente mientras se dormia: ya no hay nada que hacer.
                tab.rest.suspended = tab.rest.hidden_since.is_some();
                eprintln!("[rendimiento] pestana {id} dormida");
            }
        }
        Ok(())
    }));
    // TrySuspend exige que la pagina no sea visible (layout::relayout oculta
    // las de atras). Si falla, queda para el proximo intento.
    let _ = unsafe { core3.TrySuspend(&handler) };
}

/// Mira si Michi esta en primer plano; si cambio, la interfaz pausa o
/// reanuda su fondo animado (ver sync::push_media).
pub fn refresh_foreground(state: &Shared) {
    let now = crate::win::is_foreground();
    let changed = {
        let mut st = state.borrow_mut();
        let changed = st.foreground != now;
        st.foreground = now;
        changed
    };
    if changed {
        eprintln!("[rendimiento] primer plano: {now}");
        sync::push_media(state);
    }
}

/// Si el fondo animado de la interfaz tiene que estar congelado por el modo:
/// con Michi fuera de primer plano.
pub fn freeze_wallpaper(st: &AppState) -> bool {
    mode(st).pause_in_background() && !st.foreground
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_values_fall_back_to_normal() {
        assert_eq!(Mode::parse("super"), Mode::Super);
        assert_eq!(Mode::parse("optimized"), Mode::Optimized);
        assert_eq!(Mode::parse("normal"), Mode::Normal);
        assert_eq!(Mode::parse(""), Mode::Normal);
        assert_eq!(Mode::parse("turbo"), Mode::Normal);
        for id in IDS {
            assert_eq!(Mode::parse(id).id(), *id);
        }
    }

    #[test]
    fn super_sleeps_sooner_than_optimized_and_normal_never() {
        let (opt, sup) = (Mode::Optimized.sleep_after().unwrap(), Mode::Super.sleep_after().unwrap());
        assert!(sup < opt);
        assert_eq!(Mode::Normal.sleep_after(), None);
        assert!(!Mode::Normal.background_low_memory() && Mode::Optimized.background_low_memory());
        assert!(!Mode::Optimized.instant_panels() && Mode::Super.instant_panels());
    }
}
