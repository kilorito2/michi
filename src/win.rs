//! Ajustes de la ventana principal que winit no ofrece.
//!
//! Maximizar una ventana SIN bordes (with_decorations(false)) la hace ocupar
//! el monitor ENTERO, incluida la franja de la barra de tareas: la barra de
//! tareas quedaba tapando el borde de abajo de las paginas (o Windows la
//! escondia, tratando al navegador como un juego a pantalla completa). Windows
//! pregunta con WM_GETMINMAXINFO cuanto medir maximizada; contestamos con el
//! area de trabajo del monitor (el monitor menos la barra de tareas).

use std::sync::atomic::{AtomicIsize, Ordering};

use winit::raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle, WindowHandle,
    WindowsDisplayHandle, Win32WindowHandle,
};

/// HWND de la ventana principal (0 = todavia no hay). Ver Owner.
static MAIN_WINDOW: AtomicIsize = AtomicIsize::new(0);

pub fn remember_main_window(window: &winit::window::Window) {
    if let Ok(handle) = window.window_handle() {
        if let RawWindowHandle::Win32(h) = handle.as_raw() {
            MAIN_WINDOW.store(h.hwnd.get(), Ordering::Relaxed);
        }
    }
}

/// Trae la ventana principal al frente (y la restaura si estaba minimizada):
/// otro Michi le acaba de pasar un enlace o un archivo (ver single_instance).
/// SetForegroundWindow funciona porque ese otro proceso, que Windows abrio en
/// primer plano, nos cedio el permiso (AllowSetForegroundWindow) antes de avisar.
#[cfg(windows)]
pub fn bring_to_front() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE};
    let hwnd = MAIN_WINDOW.load(Ordering::Relaxed) as windows_sys::Win32::Foundation::HWND;
    if hwnd.is_null() {
        return;
    }
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd);
    }
}

#[cfg(not(windows))]
pub fn bring_to_front() {}

/// La ventana principal como "duena" de un dialogo (`set_parent` de rfd),
/// usable desde cualquier hilo. Los dialogos se abren en otros hilos para no
/// trabar el bucle de eventos, y sin duena Windows podia dejarlos DETRAS del
/// navegador: parecia que "no pasaba nada" (por ejemplo al confirmar una
/// extension). Con duena aparecen encima y la bloquean mientras estan abiertos.
pub struct Owner;

impl HasWindowHandle for Owner {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = std::num::NonZeroIsize::new(MAIN_WINDOW.load(Ordering::Relaxed)).ok_or(HandleError::Unavailable)?;
        // La ventana principal vive hasta que termina el proceso.
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(hwnd))) })
    }
}

impl HasDisplayHandle for Owner {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(unsafe { DisplayHandle::borrow_raw(RawDisplayHandle::Windows(WindowsDisplayHandle::new())) })
    }
}

#[cfg(windows)]
pub fn fit_maximize_to_work_area(window: &winit::window::Window) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::Shell::SetWindowSubclass;

    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
    unsafe {
        SetWindowSubclass(h.hwnd.get() as _, Some(subclass_proc), 1, 0);
    }
}

#[cfg(windows)]
unsafe extern "system" fn subclass_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
    _id: usize,
    _data: usize,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows_sys::Win32::UI::Shell::DefSubclassProc;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MINMAXINFO, WM_GETMINMAXINFO};

    // Primero winit (que completa el tamano minimo); despues corregimos el maximo.
    let result = DefSubclassProc(hwnd, msg, wparam, lparam);
    if msg == WM_GETMINMAXINFO && lparam != 0 {
        let mmi = &mut *(lparam as *mut MINMAXINFO);
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) != 0 {
            let (work, mon) = (info.rcWork, info.rcMonitor);
            mmi.ptMaxPosition = POINT { x: work.left - mon.left, y: work.top - mon.top };
            mmi.ptMaxSize = POINT { x: work.right - work.left, y: work.bottom - work.top };
        }
    }
    result
}

#[cfg(not(windows))]
pub fn fit_maximize_to_work_area(_window: &winit::window::Window) {}

/// Donde queda la ventana al restaurarla: centrada en el monitor principal.
pub fn restored_position(event_loop: &winit::event_loop::ActiveEventLoop) -> winit::dpi::PhysicalPosition<i32> {
    let Some(monitor) = event_loop.primary_monitor() else { return Default::default() };
    let (w, h) = restored_size(event_loop);
    let scale = monitor.scale_factor();
    let (size, pos) = (monitor.size(), monitor.position());
    winit::dpi::PhysicalPosition::new(
        pos.x + ((size.width as f64 - w * scale) / 2.0).max(0.0) as i32,
        pos.y + ((size.height as f64 - h * scale) / 2.0).max(0.0) as i32,
    )
}

/// Tamano (logico) de la ventana al restaurarla: 1280x800 como mucho, pero
/// nunca mas grande que el 90% del monitor. Antes era 1280x800 fijo, y en
/// una pantalla de 1920x1080 con escala 150% (1280x720 logicos) la ventana
/// restaurada se salia por abajo y por la derecha (el boton cerrar quedaba
/// fuera de la pantalla).
pub fn restored_size(event_loop: &winit::event_loop::ActiveEventLoop) -> (f64, f64) {
    let Some(monitor) = event_loop.primary_monitor() else { return (1280.0, 800.0) };
    let size = monitor.size().to_logical::<f64>(monitor.scale_factor());
    ((size.width * 0.9).min(1280.0), (size.height * 0.9).min(800.0))
}
