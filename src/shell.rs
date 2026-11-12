//! Windows shell integration: tray icon + menu, global hotkeys, autostart, and the
//! event-driven full-screen detection.

use crate::app::WM_APP_FGCHANGE;
use crate::settings::parse_hotkey;
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

pub const TRAY_ID: u32 = 1;
pub const ID_OPEN: u32 = 1;
pub const ID_PAUSE: u32 = 2;
pub const ID_AUTOSTART: u32 = 3;
pub const ID_QUIT: u32 = 4;
pub const ID_VISIBILITY: u32 = 5;

pub const HK_TOGGLE: i32 = 1;
pub const HK_CLIPBOARD: i32 = 2;
pub const HK_VISIBILITY: i32 = 3;

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("Notch");

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);

// ------------------------------------------------------------------ tray

/// Icon resource ids embedded by build.rs.
const ICON_TRAY_DARK: usize = 2;
const ICON_TRAY_LIGHT: usize = 3;

/// True when the taskbar (and so the notification area) uses Windows' light theme.
fn taskbar_is_light() -> bool {
    unsafe {
        let mut key = HKEY::default();
        let sub = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
        if RegOpenKeyExW(HKEY_CURRENT_USER, sub, None, KEY_READ, &mut key).is_err() {
            return false; // value absent on very old builds: dark
        }
        let mut data = 0u32;
        let mut len = size_of::<u32>() as u32;
        let r = RegQueryValueExW(
            key,
            w!("SystemUsesLightTheme"),
            None,
            None,
            Some(&mut data as *mut u32 as *mut u8),
            Some(&mut len),
        );
        let _ = RegCloseKey(key);
        r.is_ok() && data != 0
    }
}

/// The tray icon that reads well on the current taskbar: the bright mark on dark, the
/// deeper tones on light. The caller owns the handle.
unsafe fn load_tray_icon() -> HICON {
    unsafe {
        let Ok(hmod) = GetModuleHandleW(None) else { return HICON::default() };
        let id = if taskbar_is_light() { ICON_TRAY_LIGHT } else { ICON_TRAY_DARK };
        let cx = GetSystemMetrics(SM_CXSMICON);
        let cy = GetSystemMetrics(SM_CYSMICON);
        LoadImageW(Some(HINSTANCE(hmod.0)), PCWSTR(id as *const u16), IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR)
            .map(|h| HICON(h.0))
            .unwrap_or_default()
    }
}

pub fn add_tray(hwnd: HWND, callback_msg: u32, tooltip: &str) -> bool {
    unsafe {
        let icon = load_tray_icon();
        let mut nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: callback_msg,
            hIcon: icon,
            ..Default::default()
        };
        // szTip holds 128 UTF-16 units including the terminating NUL.
        for (i, c) in tooltip.encode_utf16().take(127).enumerate() {
            nid.szTip[i] = c;
        }
        let ok = Shell_NotifyIconW(NIM_ADD, &nid).as_bool();
        keep_tray_icon(icon);
        ok
    }
}

/// The icon currently shown in the tray. Kept alive for as long as it is shown, and freed
/// when it is replaced (theme switch, Explorer restart) or removed.
static TRAY_ICON: AtomicIsize = AtomicIsize::new(0);

fn keep_tray_icon(icon: HICON) {
    let old = TRAY_ICON.swap(icon.0 as isize, Ordering::Relaxed);
    if old != 0 {
        unsafe {
            let _ = DestroyIcon(HICON(old as *mut core::ffi::c_void));
        }
    }
}

/// Swaps the tray icon to match the taskbar theme (called when Windows' colour mode changes).
pub fn refresh_tray_icon(hwnd: HWND) {
    unsafe {
        let icon = load_tray_icon();
        let nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            uFlags: NIF_ICON,
            hIcon: icon,
            ..Default::default()
        };
        if Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool() {
            keep_tray_icon(icon);
        } else if !icon.is_invalid() {
            let _ = DestroyIcon(icon);
        }
    }
}

/// True for the `WM_SETTINGCHANGE` that Windows sends when light/dark mode is switched.
pub fn is_theme_change(lp: LPARAM) -> bool {
    if lp.0 == 0 {
        return false;
    }
    unsafe { PCWSTR(lp.0 as *const u16).to_string().map(|s| s == "ImmersiveColorSet").unwrap_or(false) }
}

pub fn remove_tray(hwnd: HWND) {
    unsafe {
        let nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            ..Default::default()
        };
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
    keep_tray_icon(HICON::default()); // frees the last icon
}

/// Shows the tray context menu at the cursor and returns the chosen command id (0 = none).
pub fn tray_menu(hwnd: HWND, paused: bool, autostart: bool, pill_hidden: bool) -> u32 {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(menu, MF_STRING, ID_OPEN as usize, w!("Open"));
        let toggle = if pill_hidden { w!("Show pill") } else { w!("Hide pill") };
        let _ = AppendMenuW(menu, MF_STRING, ID_VISIBILITY as usize, toggle);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING | check(paused), ID_PAUSE as usize, w!("Pause clipboard capture"));
        let _ = AppendMenuW(menu, MF_STRING | check(autostart), ID_AUTOSTART as usize, w!("Start with Windows"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_QUIT as usize, w!("Quit"));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_NONOTIFY,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        cmd.0 as u32
    }
}

/// Shows a small popup menu at the cursor and returns the chosen id (0 = dismissed).
pub fn popup_menu(hwnd: HWND, items: &[(u32, &str)]) -> u32 {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        for (id, label) in items {
            let wide: Vec<u16> = label.encode_utf16().chain(Some(0)).collect();
            let _ = AppendMenuW(menu, MF_STRING, *id as usize, PCWSTR(wide.as_ptr()));
        }
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        cmd.0 as u32
    }
}

// --------------------------------------------------------------- hotkeys

/// Registers a hotkey described by a settings string. Returns false if it is invalid or
/// already taken by another application.
pub fn register_hotkey(hwnd: HWND, id: i32, spec: &str) -> bool {
    let Some((mods, vk)) = parse_hotkey(spec) else {
        crate::log(&format!("hotkey '{spec}' could not be parsed"));
        return false;
    };
    unsafe {
        match RegisterHotKey(Some(hwnd), id, HOT_KEY_MODIFIERS(mods | MOD_NOREPEAT.0), vk) {
            Ok(()) => true,
            Err(e) => {
                crate::log(&format!("RegisterHotKey('{spec}', mods={mods:#x}, vk={vk:#x}) failed: {e}"));
                false
            }
        }
    }
}

/// Registers `spec`, or `fallback` if that combination is taken. Returns the combination
/// that ended up registered, if any, so the UI can tell the user.
pub fn register_hotkey_or_fallback(hwnd: HWND, id: i32, spec: &str, fallback: &str) -> Option<String> {
    if register_hotkey(hwnd, id, spec) {
        Some(spec.to_string())
    } else if spec != fallback && register_hotkey(hwnd, id, fallback) {
        Some(fallback.to_string())
    } else {
        None
    }
}

pub fn unregister_hotkeys(hwnd: HWND) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), HK_TOGGLE);
        let _ = UnregisterHotKey(Some(hwnd), HK_CLIPBOARD);
        let _ = UnregisterHotKey(Some(hwnd), HK_VISIBILITY);
    }
}

// ------------------------------------------------------------- autostart

pub fn autostart_enabled() -> bool {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_READ, &mut key).is_err() {
            return false;
        }
        let r = RegQueryValueExW(key, RUN_VALUE, None, None, None, None);
        let _ = RegCloseKey(key);
        r.is_ok()
    }
}

pub fn set_autostart(on: bool) {
    unsafe {
        let mut key = HKEY::default();
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .is_err()
        {
            return;
        }
        if on {
            if let Ok(exe) = std::env::current_exe() {
                let cmd = format!("\"{}\"", exe.display());
                let bytes: Vec<u8> = cmd.encode_utf16().chain(Some(0)).flat_map(|c| c.to_le_bytes()).collect();
                let _ = RegSetValueExW(key, RUN_VALUE, None, REG_SZ, Some(&bytes));
            }
        } else {
            let _ = RegDeleteValueW(key, RUN_VALUE);
        }
        let _ = RegCloseKey(key);
    }
}

// ------------------------------------------------------- full-screen hide

unsafe extern "system" fn fg_callback(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    let h = MAIN_HWND.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut core::ffi::c_void)), WM_APP_FGCHANGE, WPARAM(0), LPARAM(0));
        }
    }
}

/// Subscribes to "foreground window changed" events (no polling).
pub fn install_foreground_hook(hwnd: HWND) -> HWINEVENTHOOK {
    MAIN_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
    unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(fg_callback),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    }
}

pub fn remove_foreground_hook(h: HWINEVENTHOOK) {
    unsafe {
        let _ = UnhookWinEvent(h);
    }
}

/// True while a full-screen app, game or presentation is in front.
pub fn fullscreen_app_active() -> bool {
    unsafe {
        SHQueryUserNotificationState()
            .map(|s| s == QUNS_BUSY || s == QUNS_RUNNING_D3D_FULL_SCREEN || s == QUNS_PRESENTATION_MODE)
            .unwrap_or(false)
    }
}
