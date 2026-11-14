//! Windows shell integration: tray icon + menu, global hotkeys, autostart, and the
//! event-driven full-screen detection.

use crate::app::WM_APP_FGCHANGE;
use crate::settings::{Accent, TextSize, Theme, parse_hotkey};
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{
    BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, HBITMAP, HGDIOBJ,
};
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
/// Menu ids for the choices (see `TrayMenu`): a base plus the index of the choice.
pub const ID_THEME: u32 = 100;
pub const ID_ACCENT: u32 = 120;
pub const ID_TEXT: u32 = 140;
pub const ID_MONITOR: u32 = 200;

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

/// Reads one of the light/dark flags under `...\Themes\Personalize` (non-zero = light).
fn personalize_flag(name: PCWSTR) -> bool {
    unsafe {
        let mut key = HKEY::default();
        let sub = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
        if RegOpenKeyExW(HKEY_CURRENT_USER, sub, None, KEY_READ, &mut key).is_err() {
            return false; // value absent on very old builds: dark
        }
        let mut data = 0u32;
        let mut len = size_of::<u32>() as u32;
        let r = RegQueryValueExW(key, name, None, None, Some(&mut data as *mut u32 as *mut u8), Some(&mut len));
        let _ = RegCloseKey(key);
        r.is_ok() && data != 0
    }
}

/// True when the taskbar (and so the notification area) uses Windows' light theme.
fn taskbar_is_light() -> bool {
    personalize_flag(w!("SystemUsesLightTheme"))
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

/// Windows animation effects are switched off (Settings > Accessibility > Visual effects), so
/// motion should be replaced by fades. `NOTCH_REDUCED_MOTION=1` forces it on (for testing).
pub fn reduced_motion() -> bool {
    if std::env::var_os("NOTCH_REDUCED_MOTION").is_some() {
        return true;
    }
    let mut on = BOOL(1);
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut on as *mut BOOL as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_ok() && !on.as_bool()
}

/// A Windows notification through the tray icon (a toast on Windows 10/11). The timer uses it
/// when its time is up while the pill is hidden.
pub fn notify(hwnd: HWND, title: &str, text: &str) {
    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        uFlags: NIF_INFO,
        dwInfoFlags: NIIF_INFO,
        ..Default::default()
    };
    // szInfoTitle holds 64 UTF-16 units and szInfo 256, each with its terminating NUL
    for (i, c) in title.encode_utf16().take(63).enumerate() {
        nid.szInfoTitle[i] = c;
    }
    for (i, c) in text.encode_utf16().take(255).enumerate() {
        nid.szInfo[i] = c;
    }
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// The local wall-clock time `secs` from now, as "10:31 pm".
pub fn clock_after(secs: u64) -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    crate::timer::ends_at(t.wHour as u32, t.wMinute as u32, t.wSecond as u32, secs)
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

/// What the tray menu shows. The choices below it (theme, accent, text size, display) come back
/// as ids: `ID_THEME + n`, `ID_ACCENT + n`, `ID_TEXT + n` (n = index into the `ALL` list of the
/// choice) and `ID_MONITOR + n` (n = index into `monitors`).
pub struct TrayMenu<'a> {
    pub paused: bool,
    pub autostart: bool,
    pub pill_hidden: bool,
    pub theme: Theme,
    pub accent: Accent,
    pub text_size: TextSize,
    /// (label, is the one in use) for each connected display.
    pub monitors: &'a [(String, bool)],
    /// Size of the colour swatches in the Accent menu follows this DPI.
    pub dpi: u32,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

unsafe fn add_item(menu: HMENU, id: u32, label: &str, checked: bool) {
    unsafe {
        let w = wide(label);
        let flags = if checked { MF_STRING | MF_CHECKED } else { MF_STRING };
        let _ = AppendMenuW(menu, flags, id as usize, PCWSTR(w.as_ptr()));
    }
}

/// Adds `sub` to `menu` as a submenu called `label`.
unsafe fn add_submenu(menu: HMENU, sub: HMENU, label: &str) {
    unsafe {
        let w = wide(label);
        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, PCWSTR(w.as_ptr()));
    }
}

/// A round colour swatch for a menu item (two-toned when `left != right`), as a 32-bit bitmap with
/// per-pixel alpha. The caller frees it with `DeleteObject` once the menu is gone.
unsafe fn swatch_bitmap(left: u32, right: u32, size: i32) -> HBITMAP {
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return HBITMAP::default();
        };
        if bits.is_null() {
            return bmp;
        }
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, (size * size) as usize);
        let mid = (size as f32 - 1.0) / 2.0;
        let rad = size as f32 / 2.0 - 0.5;
        for y in 0..size {
            for x in 0..size {
                let d = ((x as f32 - mid).powi(2) + (y as f32 - mid).powi(2)).sqrt();
                let a = (rad + 0.5 - d).clamp(0.0, 1.0);
                let rgb = if (x as f32) < size as f32 / 2.0 { left } else { right };
                // premultiplied alpha
                let ch = |s: u32| (((rgb >> s) & 0xFF) as f32 * a).round() as u32;
                px[(y * size + x) as usize] = (((a * 255.0).round() as u32) << 24) | (ch(16) << 16) | (ch(8) << 8) | ch(0);
            }
        }
        bmp
    }
}

fn accent_label(a: Accent) -> &'static str {
    match a {
        Accent::Classic => "Classic (amber and cyan)",
        Accent::Sky => "Sky",
        Accent::Amber => "Amber",
        Accent::Mint => "Mint",
        Accent::Violet => "Violet",
        Accent::Rose => "Rose",
    }
}

/// Shows the tray context menu at the cursor and returns the chosen command id (0 = none).
pub fn tray_menu(hwnd: HWND, m: &TrayMenu) -> u32 {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        let mut swatches: Vec<HBITMAP> = Vec::new();

        add_item(menu, ID_OPEN, "Open", false);
        add_item(menu, ID_VISIBILITY, if m.pill_hidden { "Show pill" } else { "Hide pill" }, false);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        if let Ok(sub) = CreatePopupMenu() {
            for (i, t) in Theme::ALL.iter().enumerate() {
                let label = match t {
                    Theme::System => "Match Windows",
                    Theme::Dark => "Dark",
                    Theme::Light => "Light",
                };
                add_item(sub, ID_THEME + i as u32, label, *t == m.theme);
            }
            add_submenu(menu, sub, "Theme");
        }
        if let Ok(sub) = CreatePopupMenu() {
            let size = (14.0 * m.dpi.max(96) as f32 / 96.0).round() as i32;
            for (i, a) in Accent::ALL.iter().enumerate() {
                let id = ID_ACCENT + i as u32;
                add_item(sub, id, accent_label(*a), *a == m.accent);
                let (l, r) = crate::ui::theme::swatch(*a);
                let bmp = swatch_bitmap(l, r, size);
                if !bmp.is_invalid() {
                    let info = MENUITEMINFOW {
                        cbSize: size_of::<MENUITEMINFOW>() as u32,
                        fMask: MIIM_BITMAP,
                        hbmpItem: bmp,
                        ..Default::default()
                    };
                    let _ = SetMenuItemInfoW(sub, id, false, &info);
                    swatches.push(bmp);
                }
            }
            add_submenu(menu, sub, "Accent colour");
        }
        if let Ok(sub) = CreatePopupMenu() {
            for (i, t) in TextSize::ALL.iter().enumerate() {
                let label = match t {
                    TextSize::Small => "Small",
                    TextSize::Medium => "Default",
                    TextSize::Large => "Large",
                };
                add_item(sub, ID_TEXT + i as u32, label, *t == m.text_size);
            }
            add_submenu(menu, sub, "Text size");
        }
        if let Ok(sub) = CreatePopupMenu() {
            for (i, (label, on)) in m.monitors.iter().enumerate() {
                add_item(sub, ID_MONITOR + i as u32, label, *on);
            }
            add_submenu(menu, sub, "Monitor");
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        add_item(menu, ID_PAUSE, "Pause clipboard capture", m.paused);
        add_item(menu, ID_AUTOSTART, "Start with Windows", m.autostart);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        add_item(menu, ID_QUIT, "Quit", false);

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
        let _ = DestroyMenu(menu); // also destroys the submenus
        for b in swatches {
            let _ = DeleteObject(HGDIOBJ(b.0));
        }
        cmd.0 as u32
    }
}

/// True when Windows' *app* colour mode is light (`Theme::System` follows it). Dark if unknown.
pub fn apps_use_light_theme() -> bool {
    personalize_flag(w!("AppsUseLightTheme"))
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
