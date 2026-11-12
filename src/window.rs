//! The pill's top-level window: creation, placement, DPI helpers and the click-through region.

use crate::ui::{self, WIN_H, WIN_W, pill, pill_rect};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
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

/// A connected display, in physical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    /// Device name (`\\.\DISPLAY2`); what the `monitor` setting stores.
    pub name: String,
    pub primary: bool,
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    handle: isize,
}

unsafe extern "system" fn enum_monitor(hmon: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
    unsafe {
        let list = &mut *(lp.0 as *mut Vec<Monitor>);
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hmon, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
            let r = mi.monitorInfo.rcMonitor;
            let end = mi.szDevice.iter().position(|&c| c == 0).unwrap_or(mi.szDevice.len());
            list.push(Monitor {
                name: String::from_utf16_lossy(&mi.szDevice[..end]),
                primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                left: r.left,
                top: r.top,
                width: r.right - r.left,
                height: r.bottom - r.top,
                handle: hmon.0 as isize,
            });
        }
        TRUE
    }
}

/// The connected displays, primary first.
pub fn monitors() -> Vec<Monitor> {
    let mut list: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(enum_monitor), LPARAM(&mut list as *mut Vec<Monitor> as isize));
    }
    list.sort_by_key(|m| !m.primary); // stable: primary first, the rest in system order
    list
}

/// Which display the pill goes on: the one named `want`, else the primary one (also when the named
/// display is not connected any more), else whatever is first.
pub fn pick_monitor<'a>(list: &'a [Monitor], want: &str) -> Option<&'a Monitor> {
    let want = want.trim();
    if !want.is_empty() {
        if let Some(m) = list.iter().find(|m| m.name.eq_ignore_ascii_case(want)) {
            return Some(m);
        }
    }
    list.iter().find(|m| m.primary).or_else(|| list.first())
}

/// Top-left of a window `win_w` px wide centred along the top edge of `m`.
pub fn top_centre(m: &Monitor, win_w: i32) -> (i32, i32) {
    (m.left + (m.width - win_w) / 2, m.top)
}

/// "Display 2" for `\\.\DISPLAY2`, for the tray menu.
pub fn monitor_label(m: &Monitor) -> String {
    let digits: String = m.name.chars().rev().take_while(char::is_ascii_digit).collect::<Vec<_>>().into_iter().rev().collect();
    if digits.is_empty() { m.name.clone() } else { format!("Display {digits}") }
}

/// DPI of the display the pill should be on (so the window is sized right before it moves there),
/// or `None` if it cannot be told.
pub fn target_dpi(want: &str) -> Option<u32> {
    let list = monitors();
    let m = pick_monitor(&list, want)?;
    let (mut x, mut y) = (0u32, 0u32);
    unsafe {
        GetDpiForMonitor(HMONITOR(m.handle as *mut core::ffi::c_void), MDT_EFFECTIVE_DPI, &mut x, &mut y).ok()?;
    }
    (x != 0).then_some(x)
}

/// Centres the window at the top of the chosen monitor (`want` = its device name, empty for the
/// primary one). Returns the window origin (px).
pub fn place(hwnd: HWND, dpi: u32, want: &str) -> (i32, i32) {
    let ww = px(WIN_W, dpi);
    let wh = px(WIN_H, dpi);
    let list = monitors();
    let (x, y) = match pick_monitor(&list, want) {
        Some(m) => top_centre(m, ww),
        None => (0, 0),
    };
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, ww, wh, SWP_NOACTIVATE);
    }
    (x, y)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(name: &str, primary: bool, left: i32, width: i32) -> Monitor {
        Monitor { name: name.into(), primary, left, top: 0, width, height: 1080, handle: 0 }
    }

    fn two() -> Vec<Monitor> {
        vec![mon(r"\\.\DISPLAY1", true, 0, 1920), mon(r"\\.\DISPLAY2", false, 1920, 2560)]
    }

    #[test]
    fn picks_the_named_display_else_the_primary_one() {
        let l = two();
        assert_eq!(pick_monitor(&l, r"\\.\DISPLAY2").map(|m| m.name.as_str()), Some(r"\\.\DISPLAY2"));
        assert_eq!(pick_monitor(&l, r"\\.\display2").map(|m| m.left), Some(1920), "case-insensitive");
        assert_eq!(pick_monitor(&l, "").map(|m| m.primary), Some(true));
        // a display that is not connected any more falls back to the primary one
        assert_eq!(pick_monitor(&l, r"\\.\DISPLAY9").map(|m| m.primary), Some(true));
        assert_eq!(pick_monitor(&[], "x"), None);
        // no display flagged primary: the first one
        let odd = vec![mon("a", false, 0, 800), mon("b", false, 800, 800)];
        assert_eq!(pick_monitor(&odd, "").map(|m| m.name.as_str()), Some("a"));
    }

    #[test]
    fn window_is_centred_along_the_top_of_its_display() {
        let l = two();
        assert_eq!(top_centre(&l[0], 720), (600, 0));
        // the second display starts at x = 1920
        assert_eq!(top_centre(&l[1], 720), (1920 + (2560 - 720) / 2, 0));
        // a display above or left of the primary has negative or non-zero origin
        let mut m = mon("c", false, -1280, 1280);
        m.top = -200;
        assert_eq!(top_centre(&m, 720), (-1280 + 280, -200));
    }

    #[test]
    fn labels_say_which_display() {
        assert_eq!(monitor_label(&mon(r"\\.\DISPLAY2", false, 0, 1)), "Display 2");
        assert_eq!(monitor_label(&mon(r"\\.\DISPLAY12", false, 0, 1)), "Display 12");
        assert_eq!(monitor_label(&mon("Generic", false, 0, 1)), "Generic");
    }
}
