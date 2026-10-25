//! Taskbar usage widget — a small native surface in the Windows taskbar
//! showing the active Claude account's usage for Windows and one WSL distro.
//!
//! **Why native pixels, not a webview.** WebView2 transparency is a
//! composition feature; reparented into the taskbar's DirectComposition tree
//! it paints opaque and fights `WS_EX_NOACTIVATE` focus rules. So the widget
//! is a plain `WS_EX_LAYERED` window updated with `UpdateLayeredWindow`, and
//! the main window's webview renders the pixels on a `<canvas>` (same
//! pipeline as the taskbar badge) and pushes them here via
//! `usage_widget_paint`.
//!
//! **Placement.** The window is parented into `Shell_TrayWnd` (making it share
//! the taskbar's z-order and visibility instead of fighting it as a topmost
//! popup) and sits just left of `TrayNotifyWnd`. Geometry is re-read every
//! [`REPOSITION_MS`] with `GetWindowRect` only — never `SHAppBarMessage`,
//! which is a synchronous round-trip into Explorer and hangs if it does.
//! Explorer restarts destroy cross-process children, so the widget is
//! recreated on `TaskbarCreated` (and whenever the parent check fails). If the
//! taskbar can't be found or the reparent doesn't stick, it falls back to a
//! topmost popup over the same spot.
//!
//! Size, DPI scale and the **system** theme (the taskbar follows
//! `SystemUsesLightTheme`, not the apps theme) are published as
//! [`METRICS_EVENT`] whenever they change; the frontend repaints on that and
//! on every usage board.
//!
//! Settings (enabled, which WSL distro fills the second row) live in
//! `<data_dir>/usage-widget.json` so the widget comes up at boot before any
//! webview asks for it.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

pub const METRICS_EVENT: &str = "usage-widget://metrics";
pub const CLICKED_EVENT: &str = "usage-widget://clicked";
const SETTINGS_FILE: &str = "usage-widget.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WidgetSettings {
    pub enabled: bool,
    /// Distro shown on the second row; `None` = first monitored distro.
    #[serde(default)]
    pub wsl_distro: Option<String>,
}

impl Default for WidgetSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            wsl_distro: None,
        }
    }
}

/// What the frontend needs to render a frame that fits.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WidgetMetrics {
    pub width_px: u32,
    pub height_px: u32,
    pub scale: f64,
    /// System (taskbar) theme is light.
    pub light: bool,
    /// Parented into the taskbar (false = topmost-popup fallback).
    pub embedded: bool,
}

pub struct UsageWidget {
    settings_path: PathBuf,
    settings: Mutex<WidgetSettings>,
}

impl UsageWidget {
    pub fn new(data_dir: &std::path::Path) -> Self {
        let settings_path = data_dir.join(SETTINGS_FILE);
        let settings = std::fs::read_to_string(&settings_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            settings_path,
            settings: Mutex::new(settings),
        }
    }

    pub fn settings(&self) -> WidgetSettings {
        self.settings.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Start the native widget thread (Windows only; no-op elsewhere).
    pub fn start(&self, app: AppHandle) {
        #[cfg(windows)]
        native::start(app, self.settings().enabled);
        #[cfg(not(windows))]
        let _ = app;
    }

    pub fn set_settings(&self, next: WidgetSettings) -> std::io::Result<()> {
        std::fs::write(
            &self.settings_path,
            serde_json::to_string_pretty(&next).map_err(std::io::Error::other)?,
        )?;
        #[cfg(windows)]
        native::set_visible(next.enabled);
        if let Ok(mut s) = self.settings.lock() {
            *s = next;
        }
        Ok(())
    }

    pub fn metrics(&self) -> Option<WidgetMetrics> {
        #[cfg(windows)]
        {
            native::metrics()
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// Present a frame: row-major RGBA with straight alpha, as returned by a
    /// canvas `getImageData`. Dropped when it doesn't match the current size
    /// (the frontend repaints on the metrics event).
    pub fn paint(&self, rgba: Vec<u8>, width: u32, height: u32) -> Result<(), String> {
        if rgba.len() != (width as usize) * (height as usize) * 4 {
            return Err("frame size does not match width × height".into());
        }
        #[cfg(windows)]
        native::paint(premultiplied_bgra(rgba), width, height);
        Ok(())
    }
}

/// RGBA (straight) → BGRA (premultiplied), which `UpdateLayeredWindow`
/// with `AC_SRC_ALPHA` requires.
fn premultiplied_bgra(mut px: Vec<u8>) -> Vec<u8> {
    for p in px.as_chunks_mut::<4>().0 {
        let a = u16::from(p[3]);
        let premul = |c: u8| ((u16::from(c) * a + 127) / 255) as u8;
        let (r, g, b) = (premul(p[0]), premul(p[1]), premul(p[2]));
        p[0] = b;
        p[1] = g;
        p[2] = r;
    }
    px
}

#[cfg(windows)]
mod native {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use tauri::{AppHandle, Emitter, Manager};
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
        CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
        HGDIOBJ, ReleaseDC, SelectObject,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowExW,
        FindWindowW, GWL_EXSTYLE, GWL_STYLE, GetMessageW, GetParent, GetWindowLongPtrW,
        GetWindowRect, HWND_TOP, HWND_TOPMOST, IDC_HAND, IsWindow, LoadCursorW, MA_NOACTIVATE, MSG,
        PostMessageW, RegisterClassExW, RegisterWindowMessageW, SW_HIDE, SW_SHOWNOACTIVATE,
        SWP_NOACTIVATE, SetParent, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow,
        TranslateMessage, ULW_ALPHA, UpdateLayeredWindow, WM_APP, WM_DISPLAYCHANGE, WM_LBUTTONUP,
        WM_MOUSEACTIVATE, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSEXW, WS_CHILD,
        WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_POPUP,
    };
    use windows::core::{PCWSTR, w};

    use super::{CLICKED_EVENT, METRICS_EVENT, WidgetMetrics};

    const WM_APP_PAINT: u32 = WM_APP + 1;
    const WM_APP_VISIBILITY: u32 = WM_APP + 2;
    const TIMER_ID: usize = 1;
    const REPOSITION_MS: u32 = 1500;
    /// Logical width; height always matches the taskbar.
    const WIDTH_DIP: f64 = 150.0;
    const GAP_DIP: f64 = 4.0;

    struct Frame {
        width: u32,
        height: u32,
        bgra: Vec<u8>,
    }

    struct Shared {
        app: AppHandle,
        host: AtomicIsize,
        visible: AtomicBool,
        frame: Mutex<Option<Frame>>,
        metrics: Mutex<Option<WidgetMetrics>>,
    }

    static SHARED: OnceLock<Shared> = OnceLock::new();

    /// State owned by the widget thread. Only `host_proc` touches it; the
    /// widget's own proc stays stateless because `SetParent`/`SetWindowPos`
    /// re-enter it synchronously.
    #[derive(Default)]
    struct Ui {
        widget: Option<HWND>,
        taskbar: Option<HWND>,
        embedded: bool,
        taskbar_created: u32,
        /// Last `(x, y, width, height)` applied to an embedded widget, so the
        /// timer doesn't push redundant moves into the taskbar every tick.
        placed: Option<(i32, i32, i32, i32)>,
    }

    thread_local! {
        static UI: RefCell<Ui> = RefCell::new(Ui::default());
    }

    fn hwnd(v: isize) -> HWND {
        HWND(v as *mut core::ffi::c_void)
    }

    pub fn start(app: AppHandle, visible: bool) {
        if SHARED
            .set(Shared {
                app,
                host: AtomicIsize::new(0),
                visible: AtomicBool::new(visible),
                frame: Mutex::new(None),
                metrics: Mutex::new(None),
            })
            .is_err()
        {
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("usage-widget".into())
            .spawn(|| {
                // SAFETY: plain Win32 window creation + message pump, all on
                // this thread, which owns every window it creates.
                if let Err(e) = unsafe { run() } {
                    tracing::warn!("usage widget unavailable: {e}");
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("usage widget thread: {e}");
        }
    }

    fn post(msg: u32) {
        if let Some(shared) = SHARED.get() {
            let host = shared.host.load(Ordering::Acquire);
            if host != 0 {
                // SAFETY: posting to a window owned by the widget thread;
                // a stale handle just fails.
                let _ = unsafe { PostMessageW(Some(hwnd(host)), msg, WPARAM(0), LPARAM(0)) };
            }
        }
    }

    pub fn set_visible(visible: bool) {
        if let Some(shared) = SHARED.get() {
            shared.visible.store(visible, Ordering::Release);
            post(WM_APP_VISIBILITY);
        }
    }

    pub fn metrics() -> Option<WidgetMetrics> {
        SHARED.get()?.metrics.lock().ok()?.clone()
    }

    pub fn paint(bgra: Vec<u8>, width: u32, height: u32) {
        if let Some(shared) = SHARED.get() {
            if let Ok(mut f) = shared.frame.lock() {
                *f = Some(Frame {
                    width,
                    height,
                    bgra,
                });
            }
            post(WM_APP_PAINT);
        }
    }

    unsafe fn run() -> windows::core::Result<()> {
        let shared = SHARED.get().expect("set before spawn");
        let instance = unsafe { GetModuleHandleW(None)? }.into();
        let cursor = unsafe { LoadCursorW(None, IDC_HAND) }.unwrap_or_default();

        let host_class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(host_proc),
            hInstance: instance,
            lpszClassName: w!("OxyrisUsageWidgetHost"),
            ..Default::default()
        };
        let widget_class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(widget_proc),
            hInstance: instance,
            hCursor: cursor,
            lpszClassName: w!("OxyrisUsageWidget"),
            ..Default::default()
        };
        unsafe {
            RegisterClassExW(&host_class);
            RegisterClassExW(&widget_class);
        }

        // Hidden *top-level* window: message-only windows don't receive the
        // `TaskbarCreated` broadcast or `WM_SETTINGCHANGE`.
        let host = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("OxyrisUsageWidgetHost"),
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                None,
            )?
        };
        UI.with(|ui| {
            ui.borrow_mut().taskbar_created =
                unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        });
        shared.host.store(host.0 as isize, Ordering::Release);
        unsafe { SetTimer(Some(host), TIMER_ID, REPOSITION_MS, None) };
        ensure_widget();

        let mut msg = MSG::default();
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        Ok(())
    }

    unsafe extern "system" fn host_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        let taskbar_created = UI.with(|ui| ui.try_borrow().map(|u| u.taskbar_created).unwrap_or(0));
        match msg {
            WM_TIMER => {
                ensure_widget();
                reposition();
            }
            WM_APP_PAINT => present(),
            WM_APP_VISIBILITY => apply_visibility(),
            WM_SETTINGCHANGE | WM_DISPLAYCHANGE => reposition(),
            m if m != 0 && m == taskbar_created => {
                // Explorer restarted: the old child died with the old taskbar.
                destroy_widget();
                ensure_widget();
            }
            _ => return unsafe { DefWindowProcW(h, msg, wp, lp) },
        }
        LRESULT(0)
    }

    unsafe extern "system" fn widget_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_LBUTTONUP | WM_RBUTTONUP => {
                on_click();
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(h, msg, wp, lp) },
        }
    }

    fn on_click() {
        let Some(shared) = SHARED.get() else { return };
        if let Some(main) = shared.app.get_webview_window("main") {
            let _ = main.unminimize();
            let _ = main.show();
            let _ = main.set_focus();
        }
        let _ = shared.app.emit(CLICKED_EVENT, ());
    }

    fn destroy_widget() {
        UI.with(|ui| {
            let Ok(mut ui) = ui.try_borrow_mut() else {
                return;
            };
            if let Some(w) = ui.widget.take()
                && unsafe { IsWindow(Some(w)) }.as_bool()
            {
                let _ = unsafe { DestroyWindow(w) };
            }
            ui.taskbar = None;
        });
    }

    fn find_taskbar() -> Option<HWND> {
        unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) }
            .ok()
            .filter(|h| !h.is_invalid())
    }

    /// (Re)create the widget when it's missing, dead, or parented to a
    /// taskbar that no longer exists.
    fn ensure_widget() {
        let taskbar = find_taskbar();
        let healthy = UI.with(|ui| {
            let ui = ui.borrow();
            match ui.widget {
                // A popup fallback stays put until `TaskbarCreated` gives the
                // embed another chance; an embedded one must still be
                // parented to the live taskbar.
                Some(w) if unsafe { IsWindow(Some(w)) }.as_bool() => {
                    !ui.embedded || unsafe { GetParent(w) }.ok() == taskbar
                }
                _ => false,
            }
        });
        if healthy {
            return;
        }
        destroy_widget();

        // SAFETY: window creation/styling on the owning thread.
        let created = unsafe { create_widget(taskbar) };
        let Some((widget, embedded)) = created else {
            return;
        };
        UI.with(|ui| {
            let mut ui = ui.borrow_mut();
            ui.widget = Some(widget);
            ui.taskbar = taskbar;
            ui.embedded = embedded;
            ui.placed = None;
        });
        // Force a metrics emit so the frontend repaints into the new window.
        if let Some(shared) = SHARED.get()
            && let Ok(mut m) = shared.metrics.lock()
        {
            *m = None;
        }
        reposition();
        present();
        apply_visibility();
    }

    unsafe fn create_widget(taskbar: Option<HWND>) -> Option<(HWND, bool)> {
        let instance = unsafe { GetModuleHandleW(None) }.ok()?.into();
        let widget = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                w!("OxyrisUsageWidget"),
                w!("Oxyris usage"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance),
                None,
            )
        }
        .ok()?;
        let Some(taskbar) = taskbar else {
            return Some((widget, false));
        };
        unsafe {
            let ex = GetWindowLongPtrW(widget, GWL_EXSTYLE) & !(WS_EX_TOPMOST.0 as isize);
            SetWindowLongPtrW(widget, GWL_EXSTYLE, ex);
            let style = (GetWindowLongPtrW(widget, GWL_STYLE) & !(WS_POPUP.0 as isize))
                | WS_CHILD.0 as isize
                | WS_CLIPSIBLINGS.0 as isize;
            SetWindowLongPtrW(widget, GWL_STYLE, style);
            let _ = SetParent(widget, Some(taskbar));
        }
        if unsafe { GetParent(widget) }.ok() == Some(taskbar) {
            return Some((widget, true));
        }
        // Reparent refused: back to a topmost popup.
        unsafe {
            let _ = SetParent(widget, None);
            let style = (GetWindowLongPtrW(widget, GWL_STYLE)
                & !(WS_CHILD.0 as isize)
                & !(WS_CLIPSIBLINGS.0 as isize))
                | WS_POPUP.0 as isize;
            SetWindowLongPtrW(widget, GWL_STYLE, style);
            let ex = GetWindowLongPtrW(widget, GWL_EXSTYLE) | WS_EX_TOPMOST.0 as isize;
            SetWindowLongPtrW(widget, GWL_EXSTYLE, ex);
        }
        tracing::info!("usage widget: taskbar reparent refused, using popup fallback");
        Some((widget, false))
    }

    fn rect_of(h: HWND) -> Option<RECT> {
        let mut r = RECT::default();
        unsafe { GetWindowRect(h, &mut r) }.ok().map(|_| r)
    }

    fn system_light_theme() -> bool {
        let mut value: u32 = 0;
        let mut size = size_of::<u32>() as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
                w!("SystemUsesLightTheme"),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut value as *mut u32 as *mut core::ffi::c_void),
                Some(&mut size),
            )
        };
        status.is_ok() && value != 0
    }

    fn reposition() {
        let Some((widget, taskbar, embedded)) = UI.with(|ui| {
            let ui = ui.borrow();
            ui.widget.map(|w| (w, ui.taskbar, ui.embedded))
        }) else {
            return;
        };
        let Some(tb_hwnd) = taskbar.or_else(find_taskbar) else {
            return;
        };
        let Some(tb) = rect_of(tb_hwnd) else { return };
        let (tb_w, tb_h) = (tb.right - tb.left, tb.bottom - tb.top);
        if tb_w <= tb_h {
            // Vertical taskbar: no room for a two-row strip; stay hidden.
            unsafe {
                let _ = ShowWindow(widget, SW_HIDE);
            }
            return;
        }
        let dpi = match unsafe { GetDpiForWindow(tb_hwnd) } {
            0 => 96,
            d => d,
        };
        let scale = f64::from(dpi) / 96.0;
        let width = (WIDTH_DIP * scale).round() as i32;
        let height = tb_h;
        let tray_left =
            unsafe { FindWindowExW(Some(tb_hwnd), None, w!("TrayNotifyWnd"), PCWSTR::null()) }
                .ok()
                .and_then(rect_of)
                .map(|r| r.left)
                .unwrap_or(tb.right);
        let x_screen = tray_left - width - (GAP_DIP * scale).round() as i32;
        let placement = (x_screen - tb.left, 0, width, height);
        let unchanged = embedded
            && UI.with(|ui| {
                let mut ui = ui.borrow_mut();
                let same = ui.placed == Some(placement);
                ui.placed = Some(placement);
                same
            });
        unsafe {
            let _ = if unchanged {
                Ok(())
            } else if embedded {
                SetWindowPos(
                    widget,
                    Some(HWND_TOP),
                    placement.0,
                    placement.1,
                    width,
                    height,
                    SWP_NOACTIVATE,
                )
            } else {
                // Popup fallback re-asserts topmost every tick: clicking the
                // taskbar raises it above us otherwise.
                SetWindowPos(
                    widget,
                    Some(HWND_TOPMOST),
                    x_screen,
                    tb.top,
                    width,
                    height,
                    SWP_NOACTIVATE,
                )
            };
        }

        let next = WidgetMetrics {
            width_px: width.max(1) as u32,
            height_px: height.max(1) as u32,
            scale,
            light: system_light_theme(),
            embedded,
        };
        let Some(shared) = SHARED.get() else { return };
        let changed = shared
            .metrics
            .lock()
            .map(|mut m| {
                if m.as_ref() == Some(&next) {
                    false
                } else {
                    *m = Some(next.clone());
                    true
                }
            })
            .unwrap_or(false);
        if changed {
            let _ = shared.app.emit(METRICS_EVENT, &next);
        }
    }

    fn apply_visibility() {
        let Some(shared) = SHARED.get() else { return };
        let Some(widget) = UI.with(|ui| ui.borrow().widget) else {
            return;
        };
        let show = shared.visible.load(Ordering::Acquire);
        unsafe {
            let _ = ShowWindow(widget, if show { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }

    fn present() {
        let Some(shared) = SHARED.get() else { return };
        let Some(widget) = UI.with(|ui| ui.borrow().widget) else {
            return;
        };
        let Some((mw, mh)) = shared
            .metrics
            .lock()
            .ok()
            .and_then(|m| m.as_ref().map(|m| (m.width_px, m.height_px)))
        else {
            return;
        };
        let Ok(frame) = shared.frame.lock() else {
            return;
        };
        let Some(frame) = frame.as_ref() else { return };
        if frame.width != mw || frame.height != mh {
            return;
        }
        // SAFETY: GDI objects created here are released on every path below.
        unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: frame.width as i32,
                    // Negative height = top-down rows, matching canvas order.
                    biHeight: -(frame.height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            if let Ok(bitmap) =
                CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            {
                if !bits.is_null() {
                    std::ptr::copy_nonoverlapping(
                        frame.bgra.as_ptr(),
                        bits as *mut u8,
                        frame.bgra.len(),
                    );
                    let old = SelectObject(mem, HGDIOBJ(bitmap.0));
                    let size = SIZE {
                        cx: frame.width as i32,
                        cy: frame.height as i32,
                    };
                    let origin = POINT::default();
                    let blend = BLENDFUNCTION {
                        BlendOp: AC_SRC_OVER as u8,
                        BlendFlags: 0,
                        SourceConstantAlpha: 255,
                        AlphaFormat: AC_SRC_ALPHA as u8,
                    };
                    if let Err(e) = UpdateLayeredWindow(
                        widget,
                        Some(screen),
                        None,
                        Some(&size),
                        Some(mem),
                        Some(&origin),
                        COLORREF(0),
                        Some(&blend),
                        ULW_ALPHA,
                    ) {
                        tracing::debug!("usage widget: UpdateLayeredWindow: {e}");
                    }
                    SelectObject(mem, old);
                }
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
            }
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplies_and_swaps_channels() {
        // Opaque red, half-transparent white, fully transparent green.
        let out = premultiplied_bgra(vec![255, 0, 0, 255, 255, 255, 255, 128, 0, 255, 0, 0]);
        assert_eq!(&out[0..4], &[0, 0, 255, 255]);
        assert_eq!(&out[4..8], &[128, 128, 128, 128]);
        assert_eq!(&out[8..12], &[0, 0, 0, 0]);
    }

    #[test]
    fn settings_round_trip_and_default_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let w = UsageWidget::new(dir.path());
        assert!(w.settings().enabled);
        let next = WidgetSettings {
            enabled: false,
            wsl_distro: Some("Ubuntu".into()),
        };
        w.set_settings(next.clone()).unwrap();
        assert_eq!(UsageWidget::new(dir.path()).settings(), next);
    }

    #[test]
    fn paint_rejects_mismatched_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let w = UsageWidget::new(dir.path());
        assert!(w.paint(vec![0; 10], 2, 2).is_err());
    }
}
