//! The pill's top-level window: creation, placement, DPI helpers and the click-through region.

use crate::ui::{self, WIN_H, WIN_W, pill, pill_rect};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

pub const CLASS_NAME: PCWSTR = w!("NotchPillWindow");

/// Creates the (initially hidden) pill window.
///
/// - `WS_EX_NOREDIRECTIONBITMAP`: the window is composed purely by DirectComposition.
/// - `WS_EX_NOACTIVATE`: clicking the pill never steals focus from the app you are using.
/// - `WS_EX_TOOLWINDOW`: no taskbar button, not in Alt-Tab.
pub fn create() -> Result<HWND> {
    unsafe {
        let hmod = GetModuleHandleW(None)?;
        let hinst = HINSTANCE(hmod.0);
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS, // double-click a note tab to rename it
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(Error::from_thread());
        }
        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            CLASS_NAME,
            w!("Notch"),
            WS_POPUP,
            0,
            0,
            200,
            100,
            None,
            None,
            Some(hinst),
            None,
        )
    }
}

pub fn dpi_for(hwnd: HWND) -> u32 {
    let d = unsafe { GetDpiForWindow(hwnd) };
    if d == 0 { 96 } else { d }
}

/// DIPs -> physical pixels.
pub fn px(v: f32, dpi: u32) -> i32 {
    (v * dpi as f32 / 96.0).round() as i32
}

/// Centres the window at the top of the primary monitor. Returns the window origin (px).
pub fn place(hwnd: HWND, dpi: u32) -> (i32, i32) {
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let ww = px(WIN_W, dpi);
        let wh = px(WIN_H, dpi);
        let x = mi.rcMonitor.left + ((mi.rcMonitor.right - mi.rcMonitor.left) - ww) / 2;
        let y = mi.rcMonitor.top;
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, ww, wh, SWP_NOACTIVATE);
        (x, y)
    }
}

/// Restricts the window to the visible pill (plus a hair of margin) so mouse clicks outside
/// the pill fall through to whatever is underneath. The region reaches the very top edge of
/// the screen so the pill can be hit by slamming the cursor to the top.
pub fn set_region(hwnd: HWND, dpi: u32, w: f32, h: f32) {
    let s = dpi as f32 / 96.0;
    let p = pill_rect(w, h);
    let rad = pill::radius(h);
    let l = ((p.l - 2.0) * s).floor() as i32;
    let r = ((p.r + 2.0) * s).ceil() as i32 + 1;
    let b = ((p.b + 2.0) * s).ceil() as i32 + 1;
    let e = ((rad + 2.0) * 2.0 * s) as i32;
    unsafe {
        let rgn = CreateRoundRectRgn(l, 0, r, b, e, e);
        // On success the system owns the region.
        if SetWindowRgn(hwnd, Some(rgn), false) == 0 {
            let _ = DeleteObject(rgn.into());
        }
    }
}

pub fn mouse_xy(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match crate::app::with_app(|app| app.handle(hwnd, msg, wp, lp)) {
        Some(r) => r,
        None => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

#[allow(unused)]
fn _keep(_: ui::Tab) {}
