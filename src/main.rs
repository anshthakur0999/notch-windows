#![cfg_attr(not(test), windows_subsystem = "windows")]
#![allow(unsafe_op_in_unsafe_fn)]

mod anim;
mod app;
mod cli;
mod clipboard;
mod notes;
mod render;
mod settings;
mod shell;
mod store;
mod ui;
mod window;

use app::App;
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

/// Debug aid: appends to `%TEMP%\notch.log` when the `NOTCH_LOG` env var is set.
pub fn log_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("NOTCH_LOG").is_some())
}

pub fn log(msg: &str) {
    if !log_enabled() {
        return;
    }
    use std::io::Write;
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let t = START.get_or_init(std::time::Instant::now).elapsed().as_millis();
    let path = std::env::temp_dir().join("notch.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{t:>7}ms] {msg}");
    }
}

fn fatal(text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(None, PCWSTR(wide.as_ptr()), w!("Notch"), MB_OK | MB_ICONERROR);
    }
}

/// Prints to the terminal that launched us, if any. (This is a GUI-subsystem program, so it
/// has no console of its own; started from Explorer this quietly does nothing.)
fn say(msg: &str) {
    use std::io::Write;
    unsafe {
        let _ = windows::Win32::System::Console::AttachConsole(
            windows::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
        let _ = writeln!(f, "{msg}");
    }
}

/// Sends `hide` / `show` / `toggle` / `quit` to the running instance.
fn send_command(cmd: usize, name: &str) {
    unsafe {
        match FindWindowW(window::CLASS_NAME, PCWSTR::null()) {
            Ok(h) if !h.0.is_null() => {
                if PostMessageW(Some(h), app::WM_APP_COMMAND, WPARAM(cmd), LPARAM(0)).is_ok() {
                    say(&format!("Notch: {name}"));
                } else {
                    say("Notch: could not reach the running instance.");
                }
            }
            _ => say("Notch is not running. Start it with: notch.exe"),
        }
    }
}

fn main() {
    let start_hidden = match cli::parse(std::env::args().skip(1)) {
        cli::Action::Run { start_hidden } => start_hidden,
        cli::Action::Send(cmd) => {
            let name = match cmd {
                cli::CMD_HIDE => "hidden",
                cli::CMD_SHOW => "shown",
                cli::CMD_TOGGLE => "visibility toggled",
                _ => "quitting",
            };
            send_command(cmd, name);
            return;
        }
        cli::Action::Help => {
            say(cli::USAGE);
            return;
        }
        cli::Action::Unknown(arg) => {
            say(&format!("Unknown command '{arg}'.\n\n{}", cli::USAGE));
            return;
        }
    };
    unsafe {
        // Single instance.
        let _mutex = match CreateMutexW(None, false, w!("Local\\Notch.Singleton.v1")) {
            Ok(h) => h,
            Err(_) => return,
        };
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }

        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let hwnd = match window::create() {
            Ok(h) => h,
            Err(e) => {
                fatal(&format!("Could not create the window: {e}"));
                return;
            }
        };
        let app = match App::new(hwnd) {
            Ok(a) => a,
            Err(e) => {
                fatal(&format!("Could not start the graphics pipeline: {e}"));
                let _ = DestroyWindow(hwnd);
                return;
            }
        };
        if start_hidden {
            app.start_hidden();
        }
        let app = app::install(app);
        app.startup();

        run(&app);

        drop(app);
        app::uninstall();
        CoUninitialize();
    }
}

/// Event-driven main loop.
///
/// Idle: blocks in `GetMessageW` (0% CPU, no timers). While the pill is animating: pump
/// pending messages, draw a frame, then sleep until the next compositor frame (vsync).
fn run(app: &App) {
    unsafe {
        let mut msg = MSG::default();
        loop {
            if app.is_animating() {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        return;
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                if app.is_animating() {
                    app.tick();
                    render::wait_vsync();
                }
            } else {
                let r = GetMessageW(&mut msg, None, 0, 0);
                if r.0 <= 0 {
                    return; // WM_QUIT (0) or error (-1)
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}
