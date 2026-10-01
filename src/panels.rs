//! Apertura/cierre animado de los paneles laterales, y el "vigilante" del
//! cursor que decide cuando cerrarlos.
//!
//! Los paneles se superponen AL CONTENIDO (la pestana activa nunca cambia de
//! tamano, ver layout.rs). Su WebView mide siempre PANEL_W: cerrado queda
//! casi entero fuera de la ventana y solo asoma su franja de EDGE_IDLE (el
//! "grip"); al abrirse, se desliza hacia adentro. La animacion MUEVE el
//! WebView frame a frame (ver `tick`) sin cambiarle nunca el tamano: mover
//! una ventana hija no obliga a WebView2 a volver a pintar la pagina, asi
//! que el movimiento es fluido. (Se probo antes animar el ANCHO, pero
//! WebView2 no repinta mientras se lo sigue redimensionando y el panel
//! quedaba negro toda la animacion.) Es interrumpible: si se cierra a mitad
//! de abrirse (o al reves), sigue desde donde esta.
//!
//! Cierre: antes lo decidia la propia pagina con `mouseleave`, pero ese
//! evento no es fiable en un WebView2 hijo que se mueve debajo del cursor
//! — habia que ignorar los mouseleave de los primeros 400ms tras expandirse
//! (llegaban espurios), y si el cursor salia rapido justo en esa ventana el
//! unico mouseleave real se descartaba y el panel quedaba abierto para
//! siempre. Ahora la pagina solo avisa cuando el cursor ENTRA (eso si es
//! fiable), y es Rust quien mira la posicion real del cursor (GetCursorPos)
//! mientras haya un panel abierto, y lo cierra cuando lleva LEAVE_GRACE
//! fuera de su rectangulo.

use std::time::{Duration, Instant};

use crate::state::{PanelAnim, Shared, Side, PANEL_W, TOPBAR_H};
use crate::{layout, sync};

/// Duracion de un recorrido completo (cerrado <-> abierto). Un recorrido
/// parcial (animacion interrumpida) dura proporcionalmente menos.
const OPEN_MS: f64 = 300.0;
const CLOSE_MS: f64 = 230.0;
/// Cuanto tiempo tiene que estar el cursor fuera de un panel abierto para
/// cerrarlo. Evita cierres por un roce del cursor con el borde.
const LEAVE_GRACE: Duration = Duration::from_millis(200);
/// Intervalo entre frames mientras hay una animacion en curso (~120 Hz; la
/// interpolacion es por tiempo, asi que un frame tardio no la frena).
const FRAME: Duration = Duration::from_millis(8);
/// Intervalo del vigilante del cursor mientras un panel esta abierto y quieto.
const WATCH: Duration = Duration::from_millis(33);

const SIDES: [Side; 2] = [Side::Left, Side::Right];

/// Abre o cierra un panel (con animacion). Idempotente: si ya estaba en ese
/// estado solo le reconfirma el estado a la pagina del panel.
pub fn set_expanded(state: &Shared, side: Side, value: bool) {
    let changed = {
        let mut st = state.borrow_mut();
        let p = st.panel_mut(side);
        p.outside_since = None;
        if !value {
            p.held = false;
        }
        if p.expanded == value {
            false
        } else {
            p.expanded = value;
            let to = if value { 1.0 } else { 0.0 };
            let full_ms = if value { OPEN_MS } else { CLOSE_MS };
            let remaining = (to - p.progress).abs();
            p.anim = Some(PanelAnim {
                from: p.progress,
                to,
                start: Instant::now(),
                // Piso del 35% para que un recorrido cortito no sea un salto.
                duration: Duration::from_secs_f64(full_ms * remaining.max(0.35) / 1000.0),
            });
            true
        }
    };

    // Siempre: la pagina lleva su propia copia de "estoy abierto" (para no
    // mandar un IPC por cada mousemove, y para el fundido de su contenido),
    // y tiene que enterarse si fue Rust quien lo cerro.
    sync::push_expanded(state, side);
    if changed {
        state.borrow().wake();
    }
}

/// Abre un panel desde un atajo o el menu (Ctrl+H, "Historial"...), con el
/// cursor probablemente lejos: el vigilante lo cerraria a los LEAVE_GRACE.
/// Queda "sostenido" hasta que el cursor entre (desde ahi se comporta como
/// siempre) o haya un clic fuera de el.
pub fn open_held(state: &Shared, side: Side) {
    set_expanded(state, side, true);
    state.borrow_mut().panel_mut(side).held = true;
}

/// Avanza las animaciones y el vigilante del cursor. Se llama en cada vuelta
/// del bucle de eventos (ver about_to_wait en main.rs); devuelve cuando
/// quiere ser llamada de nuevo, o None si no hay nada pendiente (el bucle
/// puede dormir hasta el proximo evento).
pub fn tick(state: &Shared) -> Option<Instant> {
    watch_cursor(state);

    let now = Instant::now();
    let (moved, animating, any_open) = {
        let mut st = state.borrow_mut();
        let mut moved = false;
        for side in SIDES {
            let p = st.panel_mut(side);
            let Some(a) = &p.anim else { continue };
            let t = now.duration_since(a.start).as_secs_f64() / a.duration.as_secs_f64().max(1e-6);
            if t >= 1.0 {
                p.progress = a.to;
                p.anim = None;
            } else {
                p.progress = a.from + (a.to - a.from) * ease_out_cubic(t);
            }
            moved = true;
        }
        (
            moved,
            st.left.anim.is_some() || st.right.anim.is_some(),
            st.left.expanded || st.right.expanded,
        )
    };

    if moved {
        layout::position_panels(state);
    }

    if animating {
        Some(now + FRAME)
    } else if any_open {
        Some(now + WATCH)
    } else {
        None
    }
}

/// Arranca rapido y frena suave: se siente inmediato al acercar el cursor y
/// se asienta sin rebote. Al ser por tiempo, tambien sirve para retomar una
/// animacion interrumpida desde donde quedo.
fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

/// Cierra cualquier panel abierto cuyo rectangulo (el de abierto, PANEL_W de
/// ancho, aunque todavia se este deslizando) no contenga al cursor desde
/// hace LEAVE_GRACE. No cierra mientras el boton izquierdo este apretado,
/// para no cortar una seleccion o el arrastre de la barra de scroll que se
/// sale un poco del panel.
fn watch_cursor(state: &Shared) {
    let Some((x, y, win_w, win_h)) = cursor_in_window(state) else { return };
    let button_down = left_button_down();
    let now = Instant::now();

    let mut to_close = Vec::new();
    {
        let mut st = state.borrow_mut();
        for side in SIDES {
            let p = st.panel_mut(side);
            if !p.expanded {
                p.outside_since = None;
                continue;
            }
            let in_x = match side {
                Side::Left => x >= 0.0 && x < PANEL_W,
                Side::Right => x >= win_w - PANEL_W && x < win_w,
            };
            let inside = in_x && y >= TOPBAR_H && y < win_h;
            if p.held {
                if inside {
                    p.held = false;
                } else if button_down {
                    to_close.push(side);
                }
                continue;
            }
            if inside || button_down {
                p.outside_since = None;
                continue;
            }
            let since = *p.outside_since.get_or_insert(now);
            if now.duration_since(since) >= LEAVE_GRACE {
                to_close.push(side);
            }
        }
    }

    for side in to_close {
        set_expanded(state, side, false);
    }
}

/// Posicion del cursor relativa al area cliente de la ventana, y el tamano
/// de esa area, todo en px logicos (las mismas unidades que layout.rs).
#[cfg(windows)]
pub fn cursor_in_window(state: &Shared) -> Option<(f64, f64, f64, f64)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let st = state.borrow();
    let window = st.window.as_ref()?;
    let origin = window.inner_position().ok()?;
    let scale = window.scale_factor();
    let size = window.inner_size().to_logical::<f64>(scale);

    let mut pt = POINT { x: 0, y: 0 };
    // winit hace al proceso "per-monitor DPI aware", asi que GetCursorPos y
    // inner_position hablan los dos en px fisicos de pantalla.
    if unsafe { GetCursorPos(&mut pt) } == 0 {
        return None;
    }
    Some((
        (pt.x - origin.x) as f64 / scale,
        (pt.y - origin.y) as f64 / scale,
        size.width,
        size.height,
    ))
}

#[cfg(not(windows))]
pub fn cursor_in_window(_state: &Shared) -> Option<(f64, f64, f64, f64)> {
    None
}

/// Si algun boton del mouse esta apretado ahora, o se apreto desde la
/// ultima consulta (el bit bajo de GetAsyncKeyState: un clic rapido entre
/// dos consultas del vigilante no se pierde). Ver menu::watch.
#[cfg(windows)]
pub fn mouse_pressed() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON};
    [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON]
        .iter()
        .any(|vk| (unsafe { GetAsyncKeyState(*vk as i32) } as u16 & 0x8001) != 0)
}

#[cfg(not(windows))]
pub fn mouse_pressed() -> bool {
    false
}

#[cfg(windows)]
fn left_button_down() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    (unsafe { GetAsyncKeyState(VK_LBUTTON as i32) } as u16 & 0x8000) != 0
}

#[cfg(not(windows))]
fn left_button_down() -> bool {
    false
}
