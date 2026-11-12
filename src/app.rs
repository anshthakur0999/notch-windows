//! Application state and behaviour: the expand/collapse state machine, input handling,
//! and the glue between the window, renderer, notepad, clipboard store and shell.
//!
//! Everything lives on the single UI thread. Mutable state uses `Cell`/`RefCell` per field
//! (never one big `RefCell<App>`) because Win32 calls made from inside a handler can
//! synchronously re-enter the window procedure.

use crate::anim::{self, Spring};
use crate::clipboard::{self, Formats};
use crate::notes::Notes;
use crate::render::{self, Renderer};
use crate::settings::Settings;
use crate::cli::{CMD_HIDE, CMD_QUIT, CMD_SHOW, CMD_TOGGLE};
use crate::shell::{self, HK_CLIPBOARD, HK_TOGGLE, HK_VISIBILITY};
use crate::store::{Kind, Store, now_secs};
use crate::ui::clip_list::{self, ListLayout};
use crate::ui::notes_view::{self, NoteTabs};
use crate::ui::{
    self, COLLAPSED_H, COLLAPSED_W, EXPANDED_H, EXPANDED_W, Hit, HitCtx, Tab, View, WIN_H, WIN_W,
};
use crate::window;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::ValidateRect;
use windows::Win32::System::DataExchange::{AddClipboardFormatListener, RemoveClipboardFormatListener};
use windows::Win32::UI::Accessibility::HWINEVENTHOOK;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

const WM_MOUSELEAVE: u32 = 0x02A3;

pub const WM_APP_TRAY: u32 = WM_APP + 1;
pub const WM_APP_CLIPDONE: u32 = WM_APP + 2;
pub const WM_APP_FOCUSCHECK: u32 = WM_APP + 3;
pub const WM_APP_FGCHANGE: u32 = WM_APP + 4;
/// Sent by `notch.exe hide|show|toggle|quit`; `wParam` is a `cli::CMD_*` code.
pub const WM_APP_COMMAND: u32 = WM_APP + 5;

// All timers are one-shot: nothing ticks while the pill is idle.
const T_DWELL: usize = 1;
const T_COLLAPSE: usize = 2;
const T_SAVE: usize = 3;
const T_FS: usize = 4;
const T_PASTE: usize = 5;

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

pub fn install(app: App) -> Rc<App> {
    let rc = Rc::new(app);
    APP.with(|a| *a.borrow_mut() = Some(rc.clone()));
    rc
}

pub fn uninstall() {
    APP.with(|a| *a.borrow_mut() = None);
}

/// Runs `f` with the app, if it exists yet (window messages can arrive during creation).
pub fn with_app<R>(f: impl FnOnce(&App) -> R) -> Option<R> {
    let rc = APP.with(|a| a.borrow().clone())?;
    Some(f(&rc))
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Notch")
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

pub struct App {
    pub hwnd: HWND,
    pub settings: Settings,
    pub dpi: Cell<u32>,
    origin: Cell<(i32, i32)>,

    open: Cell<bool>,
    /// Stays open until dismissed (hotkey / click / typing) instead of closing on mouse-leave.
    pub sticky: Cell<bool>,
    /// The header pin: never auto-close (mouse leave, focus loss) until unpinned or collapsed.
    keep_open: Cell<bool>,
    /// Hidden because a full-screen app or game is in front.
    fs_hidden: Cell<bool>,
    /// Hidden on request (`notch.exe hide`, the visibility hotkey, the tray menu).
    user_hidden: Cell<bool>,
    pub paused: Cell<bool>,
    tab: Cell<Tab>,

    w: Cell<Spring>,
    h: Cell<Spring>,
    animating: Cell<bool>,
    last_frame: Cell<Instant>,

    tracking: Cell<bool>,
    dwell_pending: Cell<bool>,
    /// After an explicit close with the cursor still on the pill, don't instantly re-open.
    hover_lock: Cell<bool>,
    hover: Cell<Hit>,
    /// Highlighted clipboard row (by position in the filtered list).
    sel: Cell<Option<usize>>,
    /// The highlight was set by the keyboard (shows the Enter hint, hides hover buttons).
    kb_mode: Cell<bool>,
    mouse: Cell<(f32, f32)>,
    scroll: Cell<f32>,
    prev_fg: Cell<HWND>,
    want_focus: Cell<bool>,

    /// Clipboard search text and a buffered UTF-16 high surrogate from `WM_CHAR`.
    query: RefCell<String>,
    high_surrogate: Cell<u16>,
    /// Cached filtered list / note-tab layout; `None` = rebuild on next use.
    list: RefCell<Option<ListLayout>>,
    tabs: RefCell<Option<NoteTabs>>,
    /// How far the note-tab row is scrolled (DIPs), and whether the next layout should scroll
    /// the active (or renamed) tab into view first.
    tab_scroll: Cell<f32>,
    reveal_tab: Cell<bool>,
    /// When a tab was last closed by click (see `close_tab`).
    last_close: Cell<Option<Instant>>,
    /// Index of the note tab being renamed inline.
    renaming: Cell<Option<usize>>,

    renderer: RefCell<Option<Renderer>>,
    pub store: RefCell<Store>,
    notes: Notes,
    pub clip_formats: Formats,
    taskbar_created: u32,
    tooltip: RefCell<String>,
    fg_hook: Cell<HWINEVENTHOOK>,
}

impl App {
    pub fn new(hwnd: HWND) -> Result<App> {
        let dir = data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let settings = Settings::load(&dir.join("settings.json"));
        let store = Store::open(&dir);
        let dpi = window::dpi_for(hwnd);
        let renderer = Renderer::new(
            hwnd,
            window::px(WIN_W, dpi) as u32,
            window::px(WIN_H, dpi) as u32,
            dpi as f32,
            settings.use_gpu,
        )?;
        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        Ok(App {
            hwnd,
            settings,
            dpi: Cell::new(dpi),
            origin: Cell::new((0, 0)),
            open: Cell::new(false),
            sticky: Cell::new(false),
            keep_open: Cell::new(false),
            fs_hidden: Cell::new(false),
            user_hidden: Cell::new(false),
            paused: Cell::new(false),
            tab: Cell::new(Tab::Notes),
            w: Cell::new(Spring::new(COLLAPSED_W)),
            h: Cell::new(Spring::new(COLLAPSED_H)),
            animating: Cell::new(false),
            last_frame: Cell::new(Instant::now()),
            tracking: Cell::new(false),
            dwell_pending: Cell::new(false),
            hover_lock: Cell::new(false),
            hover: Cell::new(Hit::None),
            sel: Cell::new(None),
            kb_mode: Cell::new(false),
            mouse: Cell::new((0.0, 0.0)),
            scroll: Cell::new(0.0),
            prev_fg: Cell::new(HWND::default()),
            want_focus: Cell::new(false),
            query: RefCell::new(String::new()),
            high_surrogate: Cell::new(0),
            list: RefCell::new(None),
            tabs: RefCell::new(None),
            tab_scroll: Cell::new(0.0),
            reveal_tab: Cell::new(true),
            last_close: Cell::new(None),
            renaming: Cell::new(None),
            renderer: RefCell::new(Some(renderer)),
            store: RefCell::new(store),
            notes: Notes::new(dir),
            clip_formats: clipboard::register_formats(),
            taskbar_created,
            tooltip: RefCell::new(String::from("Notch")),
            fg_hook: Cell::new(HWINEVENTHOOK::default()),
        })
    }

    /// Positions and shows the pill, registers tray/hotkeys/clipboard listener.
    pub fn startup(&self) {
        let dpi = self.dpi.get();
        self.origin.set(window::place(self.hwnd, dpi));
        let (w, h) = self.size();
        window::set_region(self.hwnd, dpi, w, h);
        self.render();
        unsafe {
            if !self.is_hidden() {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
            let _ = AddClipboardFormatListener(self.hwnd);
        }
        // Hotkeys first, so the tray tooltip can say which ones are really active.
        let notes_hk =
            shell::register_hotkey_or_fallback(self.hwnd, HK_TOGGLE, &self.settings.hotkey_toggle, "Ctrl+Alt+N");
        let clip_hk =
            shell::register_hotkey_or_fallback(self.hwnd, HK_CLIPBOARD, &self.settings.hotkey_clipboard, "Ctrl+Alt+V");
        let vis_hk = shell::register_hotkey_or_fallback(
            self.hwnd,
            HK_VISIBILITY,
            &self.settings.hotkey_visibility,
            "Ctrl+Alt+H",
        );
        let show = |h: &Option<String>| h.clone().unwrap_or_else(|| "unavailable".into());
        let tip = format!(
            "Notch - notes: {}, clipboard: {}, hide/show: {}",
            show(&notes_hk),
            show(&clip_hk),
            show(&vis_hk)
        );
        crate::log(&format!("hotkeys: {tip}"));
        *self.tooltip.borrow_mut() = tip;
        shell::add_tray(self.hwnd, WM_APP_TRAY, &self.tooltip.borrow());
        self.fg_hook.set(shell::install_foreground_hook(self.hwnd));
    }

    pub fn is_animating(&self) -> bool {
        self.animating.get()
    }

    fn size(&self) -> (f32, f32) {
        (self.w.get().pos, self.h.get().pos)
    }

    /// True once the pill is (nearly) fully open and its content is interactive.
    fn expanded(&self) -> bool {
        self.open.get() && self.h.get().pos >= EXPANDED_H - 24.0
    }

    fn to_dip(&self, px: i32) -> f32 {
        px as f32 * 96.0 / self.dpi.get() as f32
    }

    /// The pill may close by itself (mouse leaves / focus is lost).
    fn auto_close_ok(&self) -> bool {
        !self.sticky.get() && !self.keep_open.get()
    }

    // ----------------------------------------------------- list / tab caches

    fn invalidate_list(&self) {
        *self.list.borrow_mut() = None;
    }

    fn ensure_list(&self) {
        if self.list.borrow().is_none() {
            let l = {
                let s = self.store.borrow();
                clip_list::build(s.items(), &self.query.borrow())
            };
            *self.list.borrow_mut() = Some(l);
        }
    }

    fn row_count(&self) -> usize {
        self.ensure_list();
        self.list.borrow().as_ref().map(|l| l.rows.len()).unwrap_or(0)
    }

    /// Index into the store for list row `k`.
    fn store_idx(&self, k: usize) -> Option<usize> {
        self.ensure_list();
        self.list.borrow().as_ref().and_then(|l| l.rows.get(k).map(|r| r.0))
    }

    fn max_scroll(&self) -> f32 {
        self.ensure_list();
        self.list.borrow().as_ref().map(clip_list::max_scroll).unwrap_or(0.0)
    }

    /// Call from event handlers only (the renderer must not be borrowed).
    fn ensure_tabs(&self) {
        if self.tabs.borrow().is_none() {
            if let Some(r) = self.renderer.borrow().as_ref() {
                *self.tabs.borrow_mut() = Some(self.layout_tabs(r));
            }
        }
    }

    /// Lays out the note tabs at the current scroll; if a reveal is pending, first scrolls the
    /// renamed (else the active) tab into view.
    fn layout_tabs(&self, r: &Renderer) -> NoteTabs {
        let names = self.notes.names();
        let renaming = self.renaming.get();
        let mut t = notes_view::layout(r, &names, renaming, self.tab_scroll.get());
        if self.reveal_tab.replace(false) {
            let s = notes_view::scroll_to_reveal(&t, renaming.unwrap_or(self.notes.active()));
            if (s - t.scroll).abs() > 0.01 {
                t = notes_view::layout(r, &names, renaming, s);
            }
        }
        self.tab_scroll.set(t.scroll);
        t
    }

    /// The tab row changed (note added/removed/renamed...): lay it out again, and bring the
    /// active tab into view when `reveal` is set.
    fn relayout_tabs(&self, reveal: bool) {
        if reveal {
            self.reveal_tab.set(true);
        }
        *self.tabs.borrow_mut() = None;
    }

    /// Scrolls the note-tab row by `dx` DIPs (mouse wheel over the notepad header).
    fn scroll_tabs(&self, dx: f32) {
        self.ensure_tabs();
        let max = self.tabs.borrow().as_ref().map(|t| t.max_scroll).unwrap_or(0.0);
        let now = self.tab_scroll.get();
        let next = (now + dx).clamp(0.0, max);
        if (next - now).abs() < 0.01 {
            return;
        }
        self.tab_scroll.set(next);
        self.relayout_tabs(false);
        let (x, y) = self.mouse.get();
        self.hover.set(self.hit_at(x, y)); // a different tab may now be under the pointer
        self.invalidate();
    }

    // ------------------------------------------------------------ animation

    fn start_anim(&self) {
        if !self.animating.get() {
            self.last_frame.set(Instant::now());
            self.animating.set(true);
        }
    }

    fn set_targets(&self, w: f32, h: f32) {
        let mut sw = self.w.get();
        let mut sh = self.h.get();
        sw.set_target(w);
        sh.set_target(h);
        self.w.set(sw);
        self.h.set(sh);
        self.start_anim();
    }

    /// One animation frame. Called by the main loop only while `is_animating()`.
    pub fn tick(&self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame.get()).as_secs_f32();
        self.last_frame.set(now);

        let mut w = self.w.get();
        let mut h = self.h.get();
        // opening has the springy overshoot; closing is stiffer and snaps shut
        let (freq, zeta) = if self.open.get() { (anim::FREQ, anim::ZETA) } else { (anim::CLOSE_FREQ, anim::CLOSE_ZETA) };
        w.step_with(dt, freq, zeta);
        h.step_with(dt, freq, zeta);
        let done = w.settled() && h.settled();
        if done {
            w.snap();
            h.snap();
        }
        self.w.set(w);
        self.h.set(h);

        let t0 = Instant::now();
        window::set_region(self.hwnd, self.dpi.get(), w.pos, h.pos);
        let t1 = Instant::now();
        self.render();
        let t2 = Instant::now();
        if crate::log_enabled() {
            crate::log(&format!(
                "frame dt={:.1}ms region={:.2}ms render+present={:.2}ms h={:.0}",
                dt * 1000.0,
                (t1 - t0).as_secs_f32() * 1000.0,
                (t2 - t1).as_secs_f32() * 1000.0,
                h.pos
            ));
        }

        if done {
            self.animating.set(false);
            self.on_settled();
        }
    }

    fn on_settled(&self) {
        if self.open.get() {
            if self.tab.get() == Tab::Notes {
                self.show_notes();
                if self.want_focus.get() {
                    self.want_focus.set(false);
                    self.notes.focus();
                }
            } else {
                self.want_focus.set(false);
            }
            if self.auto_close_ok() && !self.cursor_in_pill() {
                self.arm_collapse();
            }
        } else if let Some(r) = self.renderer.borrow_mut().as_mut() {
            // fully closed: give back GPU thumbnails and the WIC factory
            r.release_transient();
        }
    }

    // --------------------------------------------------------------- render

    pub fn invalidate(&self) {
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    pub fn render(&self) {
        if self.is_hidden() {
            return;
        }
        let failed = {
            let mut guard = self.renderer.borrow_mut();
            let Some(r) = guard.as_mut() else { return };
            self.ensure_list();
            let names = self.notes.names();
            if self.tabs.borrow().is_none() {
                *self.tabs.borrow_mut() = Some(self.layout_tabs(r));
            }
            let store = self.store.borrow();
            let list = self.list.borrow();
            let tabs = self.tabs.borrow();
            let query = self.query.borrow();
            let (Some(list), Some(tabs)) = (list.as_ref(), tabs.as_ref()) else { return };
            let (w, h) = self.size();
            let view = View {
                w,
                h,
                tab: self.tab.get(),
                hover: self.hover.get(),
                sel: self.sel.get(),
                kb_mode: self.kb_mode.get(),
                scroll: self.scroll.get(),
                items: store.items(),
                store: &store,
                list,
                query: &query,
                focused: unsafe { GetForegroundWindow() } == self.hwnd,
                paused: self.paused.get(),
                keep_open: self.keep_open.get(),
                note_names: &names,
                note_active: self.notes.active(),
                note_tabs: tabs,
                renaming: self.renaming.get(),
                now: now_secs(),
            };
            match ui::pill::draw(r, &view) {
                Ok(()) => false,
                Err(e) => {
                    if render::is_device_lost(&e) {
                        *guard = None;
                        true
                    } else {
                        false
                    }
                }
            }
        };
        if failed {
            self.rebuild_renderer();
        }
    }

    /// The GPU device was removed/reset (driver update, GPU hot-swap): rebuild the pipeline.
    fn rebuild_renderer(&self) {
        let dpi = self.dpi.get();
        if let Ok(r) = Renderer::new(
            self.hwnd,
            window::px(WIN_W, dpi) as u32,
            window::px(WIN_H, dpi) as u32,
            dpi as f32,
            self.settings.use_gpu,
        ) {
            *self.renderer.borrow_mut() = Some(r);
            self.invalidate();
        }
    }

    // ------------------------------------------------------- open / close

    /// Starts expanding (from hover or hotkey). Remembers which window to give focus back to.
    fn expand(&self) {
        if self.open.get() || self.is_hidden() {
            return;
        }
        unsafe {
            let fg = GetForegroundWindow();
            if fg != self.hwnd && fg != self.notes.hwnd() && fg != self.notes.edit_hwnd() {
                self.prev_fg.set(fg);
            }
        }
        self.open.set(true);
        self.relayout_tabs(true); // show the active note's tab even if the row is scrolled elsewhere
        self.set_targets(EXPANDED_W, EXPANDED_H);
    }

    /// Opens (or keeps open) until explicitly dismissed, and takes keyboard focus.
    fn open_sticky(&self, tab: Option<Tab>) {
        if self.is_hidden() {
            return;
        }
        if let Some(t) = tab {
            self.set_tab(t);
        }
        self.expand();
        self.sticky.set(true);
        if self.tab.get() == Tab::Clipboard {
            // keyboard-first: newest entry is pre-selected, Enter pastes it
            self.kb_mode.set(true);
            self.sel.set(if self.row_count() > 0 { Some(0) } else { None });
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), T_COLLAPSE);
        }
        self.activate_pill();
        crate::log(&format!(
            "open_sticky: foreground_is_pill={}",
            unsafe { GetForegroundWindow() } == self.hwnd
        ));
        if unsafe { GetForegroundWindow() } != self.hwnd {
            // Windows refused to give us the foreground: behave like a hover-open instead,
            // otherwise nothing could ever dismiss us.
            self.sticky.set(false);
            self.arm_collapse();
        } else if self.tab.get() == Tab::Notes {
            if self.expanded() {
                self.show_notes();
                self.notes.focus();
            } else {
                self.want_focus.set(true);
            }
        }
    }

    /// Briefly drops WS_EX_NOACTIVATE so the pill can take the foreground for keyboard input.
    fn activate_pill(&self) {
        unsafe {
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex & !(WS_EX_NOACTIVATE.0 as isize));
            let _ = SetForegroundWindow(self.hwnd);
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex);
        }
    }

    fn is_ours(&self, fg: HWND) -> bool {
        let nz = |h: HWND| !h.0.is_null();
        fg == self.hwnd
            || (nz(self.notes.hwnd()) && fg == self.notes.hwnd())
            || (nz(self.notes.edit_hwnd()) && fg == self.notes.edit_hwnd())
    }

    /// Collapses and, if we hold the foreground, hands it back to the window it came from.
    pub fn collapse(&self) {
        if !self.open.get() {
            return;
        }
        self.finish_rename(false);
        unsafe {
            let ours = self.is_ours(GetForegroundWindow());

            self.hover_lock.set(self.cursor_in_pill());
            self.open.set(false);
            self.sticky.set(false);
            self.keep_open.set(false);
            self.want_focus.set(false);
            self.dwell_pending.set(false);
            let _ = KillTimer(Some(self.hwnd), T_COLLAPSE);
            let _ = KillTimer(Some(self.hwnd), T_DWELL);
            self.notes.hide();
            self.hover.set(Hit::None);
            self.sel.set(None);
            self.scroll.set(0.0);
            self.query.borrow_mut().clear();
            self.invalidate_list();
            self.set_targets(COLLAPSED_W, COLLAPSED_H);

            if ours {
                let prev = self.prev_fg.get();
                if !prev.0.is_null() && IsWindow(Some(prev)).as_bool() {
                    let _ = SetForegroundWindow(prev);
                }
            }
        }
    }

    fn arm_collapse(&self) {
        unsafe {
            SetTimer(Some(self.hwnd), T_COLLAPSE, self.settings.collapse_delay_ms.max(10), None);
        }
    }

    fn set_tab(&self, t: Tab) {
        if self.tab.get() == t {
            return;
        }
        self.finish_rename(false);
        self.tab.set(t);
        self.scroll.set(0.0);
        self.hover.set(Hit::None);
        self.sel.set(None);
        if t == Tab::Notes {
            if self.expanded() {
                self.show_notes();
                if self.sticky.get() {
                    self.notes.focus();
                }
            }
        } else {
            self.notes.hide();
        }
        self.invalidate();
    }

    fn show_notes(&self) {
        if !self.open.get() || self.tab.get() != Tab::Notes {
            return;
        }
        let dpi = self.dpi.get();
        let c = ui::notes_rect();
        let (ox, oy) = self.origin.get();
        self.notes.show(
            self.hwnd,
            ox + window::px(c.l, dpi),
            oy + window::px(c.t, dpi),
            window::px(c.w(), dpi),
            window::px(c.h(), dpi),
            dpi,
        );
    }

    pub fn on_notes_focus(&self) {
        crate::log("notes focused");
        // Typing in the notepad keeps the pill open until you click away or press Esc.
        self.sticky.set(true);
        unsafe {
            let _ = KillTimer(Some(self.hwnd), T_COLLAPSE);
        }
    }

    pub fn post_focus_check(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_APP_FOCUSCHECK, WPARAM(0), LPARAM(0));
        }
    }

    fn focus_check(&self) {
        if !self.open.get() || !self.sticky.get() || self.keep_open.get() {
            return;
        }
        let fg = unsafe { GetForegroundWindow() };
        if self.is_ours(fg) {
            return;
        }
        // The user clicked into another app: close without stealing focus back.
        self.collapse();
    }

    /// True if the cursor is over the visible pill (including the strip at the screen edge).
    fn cursor_in_pill(&self) -> bool {
        let mut pt = POINT::default();
        if unsafe { GetCursorPos(&mut pt) }.is_err() {
            return false;
        }
        let (ox, oy) = self.origin.get();
        let x = self.to_dip(pt.x - ox);
        let y = self.to_dip(pt.y - oy);
        let (w, h) = self.size();
        let mut r = ui::pill_rect(w, h);
        r.t = 0.0;
        r.contains(x, y)
    }

    fn hit_at(&self, x: f32, y: f32) -> Hit {
        self.ensure_list();
        self.ensure_tabs();
        let (w, h) = self.size();
        let list = self.list.borrow();
        let tabs = self.tabs.borrow();
        let empty = NoteTabs::empty();
        let Some(list) = list.as_ref() else { return Hit::None };
        ui::hit_test(
            x,
            y,
            &HitCtx {
                w,
                h,
                expanded: self.expanded(),
                tab: self.tab.get(),
                scroll: self.scroll.get(),
                list,
                tabs: tabs.as_ref().unwrap_or(&empty),
            },
        )
    }

    // ---------------------------------------------------------------- input

    fn on_mouse_move(&self, px: i32, py: i32) {
        let (x, y) = (self.to_dip(px), self.to_dip(py));
        if crate::log_enabled() {
            crate::log(&format!(
                "mouse_move px=({px},{py}) dip=({x:.0},{y:.0}) open={} lock={} dwell_pending={}",
                self.open.get(),
                self.hover_lock.get(),
                self.dwell_pending.get(),
            ));
        }
        self.mouse.set((x, y));
        if !self.tracking.get() {
            let mut t = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut t);
            }
            self.tracking.set(true);
        }
        if !self.open.get() {
            if self.hover_lock.get() || self.dwell_pending.get() || self.is_hidden() {
                return;
            }
            let d = self.settings.hover_dwell_ms;
            if d == 0 {
                self.expand();
            } else {
                self.dwell_pending.set(true);
                unsafe {
                    SetTimer(Some(self.hwnd), T_DWELL, d.max(10), None);
                }
            }
            return;
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), T_COLLAPSE);
        }
        let hit = self.hit_at(x, y);
        if hit != self.hover.get() {
            self.hover.set(hit);
            if let Hit::Row(k) | Hit::RowPin(k) | Hit::RowDel(k) = hit {
                self.sel.set(Some(k)); // mouse and keyboard share one highlighted row
                self.kb_mode.set(false);
            }
            self.invalidate();
        }
    }

    fn on_mouse_leave(&self) {
        self.tracking.set(false);
        self.hover_lock.set(false);
        self.dwell_pending.set(false);
        unsafe {
            let _ = KillTimer(Some(self.hwnd), T_DWELL);
        }
        if self.hover.get() != Hit::None {
            self.hover.set(Hit::None);
            if !self.kb_mode.get() {
                self.sel.set(None);
            }
            self.invalidate();
        }
        if self.open.get() && self.auto_close_ok() {
            self.arm_collapse();
        }
    }

    fn on_click(&self, px: i32, py: i32, shift: bool) {
        let (x, y) = (self.to_dip(px), self.to_dip(py));
        if crate::log_enabled() {
            crate::log(&format!(
                "CLICK dip=({x:.0},{y:.0}) open={} expanded={} hit={:?}",
                self.open.get(),
                self.expanded(),
                self.hit_at(x, y)
            ));
        }
        if !self.open.get() {
            if self.hit_at(x, y) == Hit::Pill {
                self.open_sticky(None);
            }
            return;
        }
        match self.hit_at(x, y) {
            Hit::TabNotes => self.set_tab(Tab::Notes),
            Hit::TabClip => self.set_tab(Tab::Clipboard),
            Hit::BtnPin => {
                self.keep_open.set(!self.keep_open.get());
                if self.keep_open.get() {
                    unsafe {
                        let _ = KillTimer(Some(self.hwnd), T_COLLAPSE);
                    }
                }
                self.invalidate();
            }
            Hit::BtnCollapse => self.collapse(),
            Hit::Search => {
                // clicking the field means "I want to type": take keyboard focus
                self.sticky.set(true);
                self.activate_pill();
                self.invalidate();
            }
            Hit::NoteTab(i) => self.select_note(i),
            Hit::NoteClose(i) => self.close_tab(i),
            Hit::NoteAdd => self.add_note(),
            Hit::Row(k) => self.activate_row(k, shift),
            Hit::RowPin(k) => self.toggle_pin_row(k),
            Hit::RowDel(k) => self.delete_row(k),
            _ => {}
        }
    }

    fn on_double_click(&self, px: i32, py: i32, shift: bool) {
        if self.expanded() {
            let (x, y) = (self.to_dip(px), self.to_dip(py));
            match self.hit_at(x, y) {
                Hit::NoteTab(i) => {
                    self.begin_rename(i);
                    return;
                }
                // the second click of a quick double-click on an "x" must not delete another note
                Hit::NoteClose(_) => return,
                _ => {}
            }
        }
        self.on_click(px, py, shift);
    }

    /// Middle-click on a note tab closes it (like browser tabs).
    fn on_middle_click(&self, px: i32, py: i32) {
        if !self.expanded() || self.tab.get() != Tab::Notes {
            return;
        }
        let (x, y) = (self.to_dip(px), self.to_dip(py));
        if let Hit::NoteTab(i) | Hit::NoteClose(i) = self.hit_at(x, y) {
            self.close_tab(i);
        }
    }

    /// Deletes note `i` from a click. Right after one such delete, further clicks are ignored for
    /// a moment: the next tab slides under the pointer, and a double-click must not take it too.
    fn close_tab(&self, i: usize) {
        let recent = self.last_close.get().is_some_and(|t| t.elapsed() < Duration::from_millis(450));
        if recent {
            return;
        }
        self.delete_note(i);
        self.last_close.set(Some(Instant::now()));
    }

    /// Ctrl+W in the notepad: delete the note being edited (asks first if it has text).
    pub fn close_note(&self) {
        self.finish_rename(false);
        self.delete_note(self.notes.active());
    }

    fn on_right_click(&self, px: i32, py: i32) {
        if !self.expanded() {
            return;
        }
        let (x, y) = (self.to_dip(px), self.to_dip(py));
        if let Hit::NoteTab(i) = self.hit_at(x, y) {
            self.note_menu(i);
        }
    }

    /// Horizontal wheel / touchpad swipe (`WM_MOUSEHWHEEL`): moves the note tabs (right = further right).
    pub fn on_hwheel(&self, delta: i16) {
        if self.expanded() && self.tab.get() == Tab::Notes {
            self.scroll_tabs(delta as f32 / 120.0 * 72.0);
        }
    }

    pub fn on_wheel(&self, delta: i16) {
        if self.expanded() && self.tab.get() == Tab::Notes {
            // the notepad's own text scrolls itself; over the pill's chrome the wheel moves the
            // note tabs (wheel down = further right)
            self.scroll_tabs(-(delta as f32) / 120.0 * 72.0);
            return;
        }
        if !self.expanded() || self.tab.get() != Tab::Clipboard {
            return;
        }
        let s = (self.scroll.get() - delta as f32 / 120.0 * clip_list::ROW_H).clamp(0.0, self.max_scroll());
        self.scroll.set(s);
        let (x, y) = self.mouse.get();
        let hit = self.hit_at(x, y);
        self.hover.set(hit);
        if let Hit::Row(k) | Hit::RowPin(k) | Hit::RowDel(k) = hit {
            self.sel.set(Some(k));
            self.kb_mode.set(false);
        }
        self.invalidate();
    }

    /// The list changed under a stationary pointer (a row was deleted, a new entry arrived):
    /// hit-test again at the last mouse position, so the hovered row and its pin/x buttons appear
    /// at once instead of waiting for the next mouse move. While the keyboard is driving, rows
    /// are not hovered (the selection shows instead); with the pointer outside the pill, nothing is.
    fn refresh_hover(&self) {
        if !self.expanded() {
            return;
        }
        if !self.tracking.get() {
            self.hover.set(Hit::None);
            return;
        }
        let (x, y) = self.mouse.get();
        let hit = self.hit_at(x, y);
        if self.tab.get() == Tab::Notes {
            self.hover.set(hit); // the note tabs moved under the pointer
            return;
        }
        if self.tab.get() != Tab::Clipboard {
            return;
        }
        if let Hit::Row(k) | Hit::RowPin(k) | Hit::RowDel(k) = hit {
            if self.kb_mode.get() {
                self.hover.set(Hit::None);
            } else {
                self.hover.set(hit);
                self.sel.set(Some(k));
            }
        } else {
            self.hover.set(hit);
        }
        if crate::log_enabled() {
            crate::log(&format!("refresh_hover at ({x:.0},{y:.0}) -> {:?} sel={:?}", self.hover.get(), self.sel.get()));
        }
    }

    /// Moves the keyboard selection in the clipboard list and keeps it scrolled into view.
    fn move_selection(&self, delta: i32) {
        if !self.expanded() || self.tab.get() != Tab::Clipboard {
            return;
        }
        let n = self.row_count();
        if n == 0 {
            return;
        }
        let cur = self.sel.get().map(|i| i as i32).unwrap_or(if delta > 0 { -1 } else { n as i32 });
        let next = (cur + delta).clamp(0, n as i32 - 1) as usize;
        self.sel.set(Some(next));
        self.kb_mode.set(true);
        let scroll = {
            let l = self.list.borrow();
            l.as_ref().map(|l| clip_list::scroll_to_row(l, next, self.scroll.get())).unwrap_or(0.0)
        };
        self.scroll.set(scroll);
        self.hover.set(Hit::None); // keyboard takes over from the mouse highlight
        self.invalidate();
    }

    /// The search text changed: rebuild the list and select the first match.
    fn query_changed(&self) {
        self.invalidate_list();
        self.scroll.set(0.0);
        self.hover.set(Hit::None);
        self.kb_mode.set(true);
        let n = self.row_count();
        self.sel.set(if n > 0 { Some(0) } else { None });
        self.invalidate();
    }

    /// `WM_CHAR`: typing while the clipboard tab has focus edits the search field.
    fn on_char(&self, c: u16) {
        if !self.expanded() || self.tab.get() != Tab::Clipboard || self.renaming.get().is_some() {
            return;
        }
        {
            let mut q = self.query.borrow_mut();
            match c {
                0x08 => {
                    q.pop();
                }
                c if c < 0x20 || c == 0x7F => return, // Enter, Esc, Ctrl+letter...
                c if (0xD800..0xDC00).contains(&c) => {
                    self.high_surrogate.set(c); // first half of an emoji etc.
                    return;
                }
                c => {
                    let ch = if (0xDC00..0xE000).contains(&c) {
                        let hi = self.high_surrogate.replace(0);
                        char::decode_utf16([hi, c]).next().and_then(|r| r.ok())
                    } else {
                        char::from_u32(c as u32)
                    };
                    if let Some(ch) = ch {
                        if q.chars().count() < 200 {
                            q.push(ch);
                        }
                    }
                }
            }
        }
        self.query_changed();
    }

    fn on_key(&self, vk: u32) {
        let ctrl = key_down(VK_CONTROL);
        match VIRTUAL_KEY(vk as u16) {
            VK_ESCAPE => {
                if self.tab.get() == Tab::Clipboard && !self.query.borrow().is_empty() {
                    self.query.borrow_mut().clear(); // first Esc clears the search
                    self.query_changed();
                } else {
                    self.collapse();
                }
            }
            VK_TAB => {
                let next = if self.tab.get() == Tab::Notes { Tab::Clipboard } else { Tab::Notes };
                self.set_tab(next);
            }
            VK_UP => self.move_selection(-1),
            VK_DOWN => self.move_selection(1),
            VK_PRIOR => self.move_selection(-5),
            VK_NEXT => self.move_selection(5),
            VK_RETURN => {
                if self.expanded() && self.tab.get() == Tab::Clipboard {
                    if let Some(k) = self.sel.get() {
                        self.activate_row(k, key_down(VK_SHIFT));
                    }
                }
            }
            VK_DELETE => {
                if self.expanded() && self.tab.get() == Tab::Clipboard {
                    if let Some(k) = self.sel.get() {
                        self.delete_row(k);
                    }
                }
            }
            VK_P if ctrl => {
                if self.expanded() && self.tab.get() == Tab::Clipboard {
                    if let Some(k) = self.sel.get() {
                        self.toggle_pin_row(k);
                    }
                }
            }
            _ => {}
        }
    }

    fn on_hotkey(&self, id: i32) {
        crate::log(&format!("HOTKEY id={id} open={} tab={:?}", self.open.get(), self.tab.get()));
        match id {
            HK_TOGGLE => {
                if self.open.get() {
                    self.collapse();
                } else {
                    self.open_sticky(None);
                }
            }
            HK_CLIPBOARD => {
                if self.open.get() && self.tab.get() == Tab::Clipboard {
                    self.collapse();
                } else {
                    self.open_sticky(Some(Tab::Clipboard));
                }
            }
            HK_VISIBILITY => self.toggle_user_hidden(),
            _ => {}
        }
    }

    // --------------------------------------------------------- clipboard rows

    fn activate_row(&self, k: usize, shift: bool) {
        let Some(idx) = self.store_idx(k) else { return };
        let (id, kind) = {
            let s = self.store.borrow();
            let it = &s.items()[idx];
            (it.id, it.kind)
        };
        let ok = match kind {
            Kind::Text => {
                let path = self.store.borrow().text_path(id);
                match std::fs::read_to_string(path) {
                    Ok(t) => clipboard::set_text(self.hwnd, &t),
                    Err(_) => false,
                }
            }
            Kind::Image => {
                let path = self.store.borrow().image_path(id);
                clipboard::set_image(self.hwnd, &path, clipboard::png_format(&self.clip_formats))
            }
        };
        crate::log(&format!("activate_row k={k} id={id} kind={kind:?} set_clipboard_ok={ok} shift={shift}"));
        if !ok {
            return;
        }
        let paste = self.settings.paste_on_click && !shift;
        self.collapse(); // also returns focus to the previous window
        if paste {
            unsafe {
                SetTimer(Some(self.hwnd), T_PASTE, 90, None);
            }
        }
    }

    fn toggle_pin_row(&self, k: usize) {
        let Some(idx) = self.store_idx(k) else { return };
        let id = self.store.borrow().items()[idx].id;
        {
            let mut s = self.store.borrow_mut();
            s.toggle_pin(id);
            let _ = s.save();
        }
        self.invalidate_list();
        self.ensure_list();
        // the entry moves between sections: keep the highlight on it
        let pos = {
            let l = self.list.borrow();
            let s = self.store.borrow();
            l.as_ref().and_then(|l| l.rows.iter().position(|r| s.items()[r.0].id == id))
        };
        if let Some(p) = pos {
            self.sel.set(Some(p));
            let scroll = {
                let l = self.list.borrow();
                l.as_ref().map(|l| clip_list::scroll_to_row(l, p, self.scroll.get())).unwrap_or(0.0)
            };
            self.scroll.set(scroll);
        }
        self.hover.set(Hit::None);
        self.invalidate();
    }

    fn delete_row(&self, k: usize) {
        let Some(idx) = self.store_idx(k) else { return };
        {
            let mut s = self.store.borrow_mut();
            let id = s.items()[idx].id;
            if let Some(it) = s.remove(id) {
                s.delete_files(&it);
                if let Some(r) = self.renderer.borrow_mut().as_mut() {
                    r.thumbs.remove(&id);
                }
            }
            let _ = s.save();
        }
        self.invalidate_list();
        let n = self.row_count();
        self.sel.set(if n == 0 { None } else { Some(k.min(n - 1)) });
        self.scroll.set(self.scroll.get().min(self.max_scroll()));
        self.refresh_hover(); // the next row slid up under the pointer
        self.invalidate();
    }

    // ------------------------------------------------------------- notes

    pub fn select_note(&self, i: usize) {
        if i == self.notes.active() || i >= self.notes.count() {
            return;
        }
        self.finish_rename(true);
        self.notes.select(i);
        self.relayout_tabs(true); // e.g. Ctrl+Tab onto a tab that is scrolled out of sight
        if self.sticky.get() {
            self.notes.focus();
        }
        self.invalidate();
    }

    pub fn add_note(&self) {
        self.finish_rename(true);
        if self.notes.add() {
            self.relayout_tabs(true); // the new tab is last: scroll to it
            if self.expanded() && self.tab.get() == Tab::Notes {
                self.show_notes();
                self.sticky.set(true);
                self.notes.focus();
            }
            self.invalidate();
        }
    }

    /// Ctrl+Tab / Ctrl+Shift+Tab in the editor.
    pub fn cycle_note(&self, dir: i32) {
        let n = self.notes.count() as i32;
        if n > 1 {
            let next = (self.notes.active() as i32 + dir).rem_euclid(n) as usize;
            self.select_note(next);
        }
    }

    fn begin_rename(&self, i: usize) {
        if !self.expanded() || self.tab.get() != Tab::Notes {
            return;
        }
        self.finish_rename(true);
        // lay the row out again with this tab widened: later tabs and the "+" shift to its right
        self.renaming.set(Some(i));
        self.relayout_tabs(true); // reveals the tab being renamed
        self.ensure_tabs();
        let rect = self.tabs.borrow().as_ref().and_then(|t| t.tabs.get(i).copied());
        let Some(rect) = rect else {
            self.renaming.set(None);
            *self.tabs.borrow_mut() = None;
            return;
        };
        self.sticky.set(true);
        self.activate_pill();
        let dpi = self.dpi.get();
        let (ox, oy) = self.origin.get();
        // The editor is only the text strip: it starts where the tab's label starts and is one
        // line tall, centred on the label's baseline row. The rounded box around it is drawn by
        // `notes_view` from `rename_box`, so the two always line up.
        let boxr = notes_view::rename_box(&rect);
        let (tx, ty, tw, th) = notes_view::rename_text(&boxr, &rect);
        let started = self.notes.begin_rename(
            self.hwnd,
            ox + window::px(tx, dpi),
            oy + window::px(ty, dpi),
            window::px(tw, dpi),
            window::px(th, dpi),
            &self.notes.name(i),
        );
        if !started {
            self.renaming.set(None);
            *self.tabs.borrow_mut() = None;
        }
        self.invalidate();
    }

    /// Ends an inline rename (Enter/blur commits, Esc cancels).
    pub fn finish_rename(&self, commit: bool) {
        let Some(i) = self.renaming.take() else { return };
        let fg_is_ours = self.is_ours(unsafe { GetForegroundWindow() });
        let text = self.notes.edit_text();
        self.notes.hide_edit();
        if commit {
            self.notes.rename(i, &text);
        }
        *self.tabs.borrow_mut() = None;
        self.invalidate();
        // hand typing focus back to the note unless the user moved to another app
        if fg_is_ours && self.open.get() && self.tab.get() == Tab::Notes {
            self.notes.focus();
        }
    }

    fn note_menu(&self, i: usize) {
        self.sticky.set(true);
        self.activate_pill();
        match shell::popup_menu(self.hwnd, &[(1, "Rename"), (2, "Delete")]) {
            1 => self.begin_rename(i),
            2 => self.delete_note(i),
            _ => {}
        }
    }

    fn delete_note(&self, i: usize) {
        self.notes.flush();
        if self.notes.len_on_disk(i) > 0 {
            let text = format!("Delete \u{201C}{}\u{201D}? This cannot be undone.", self.notes.name(i));
            let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
            let answer = unsafe {
                MessageBoxW(
                    Some(self.hwnd),
                    PCWSTR(wide.as_ptr()),
                    w!("Delete note"),
                    MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2 | MB_TOPMOST | MB_SETFOREGROUND,
                )
            };
            if answer != IDYES {
                return;
            }
        }
        self.notes.delete(i);
        self.relayout_tabs(true);
        self.refresh_hover(); // the next tab slid under the pointer: show its x straight away
        self.invalidate();
    }

    fn set_cursor(&self) {
        let id = match self.hover.get() {
            Hit::None | Hit::Pill => IDC_ARROW,
            Hit::Search => IDC_IBEAM,
            _ => IDC_HAND,
        };
        unsafe {
            if let Ok(c) = LoadCursorW(None, id) {
                SetCursor(Some(c));
            }
        }
    }

    // --------------------------------------------------------------- misc

    fn on_timer(&self, id: usize) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), id); // every timer is one-shot
        }
        if crate::log_enabled() {
            crate::log(&format!("timer {id} open={} in_pill={}", self.open.get(), self.cursor_in_pill()));
        }
        match id {
            T_DWELL => {
                self.dwell_pending.set(false);
                if !self.open.get() && !self.hover_lock.get() && self.cursor_in_pill() {
                    self.expand();
                }
            }
            T_COLLAPSE => {
                if self.open.get() && self.auto_close_ok() {
                    if self.cursor_in_pill() || self.notes.has_focus() {
                        // still hovering (e.g. over the notepad popup): look again shortly
                        self.arm_collapse();
                    } else {
                        self.collapse();
                    }
                }
            }
            T_SAVE => self.notes.flush(),
            T_FS => self.update_fullscreen(),
            T_PASTE => {
                crate::log(&format!("send Ctrl+V; foreground={:?}", unsafe { GetForegroundWindow() }));
                clipboard::send_ctrl_v()
            }
            _ => {}
        }
    }

    fn update_fullscreen(&self) {
        let fs = shell::fullscreen_app_active();
        if fs != self.fs_hidden.get() {
            self.fs_hidden.set(fs);
            self.apply_visibility();
        }
    }

    /// Hidden for any reason: a full-screen app is in front, or the user hid the pill.
    fn is_hidden(&self) -> bool {
        self.fs_hidden.get() || self.user_hidden.get()
    }

    /// Brings the window's visibility in line with `is_hidden()`.
    fn apply_visibility(&self) {
        let visible = unsafe { IsWindowVisible(self.hwnd).as_bool() };
        if self.is_hidden() && visible {
            self.collapse();
            // jump straight to the collapsed size so nothing animates while hidden
            let mut w = self.w.get();
            let mut h = self.h.get();
            w.target = COLLAPSED_W;
            h.target = COLLAPSED_H;
            w.snap();
            h.snap();
            self.w.set(w);
            self.h.set(h);
            self.animating.set(false);
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        } else if !self.is_hidden() && !visible {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
            self.invalidate();
        }
        crate::log(&format!(
            "visibility: user_hidden={} fullscreen_hidden={} window_visible={}",
            self.user_hidden.get(),
            self.fs_hidden.get(),
            unsafe { IsWindowVisible(self.hwnd).as_bool() }
        ));
    }

    /// Hides or shows the pill on request (command, hotkey, tray). Clipboard capture and the
    /// hotkeys keep running while it is hidden.
    pub fn set_user_hidden(&self, hidden: bool) {
        if self.user_hidden.get() != hidden {
            self.user_hidden.set(hidden);
        }
        self.apply_visibility();
    }

    pub fn toggle_user_hidden(&self) {
        self.set_user_hidden(!self.user_hidden.get());
    }

    /// Called once before `startup` for `notch.exe --hidden`.
    pub fn start_hidden(&self) {
        self.user_hidden.set(true);
    }

    /// `WM_APP_COMMAND`: a command sent by another `notch.exe hide|show|toggle|quit`.
    fn on_app_command(&self, cmd: usize) {
        crate::log(&format!("command received: {cmd}"));
        match cmd {
            CMD_HIDE => self.set_user_hidden(true),
            CMD_SHOW => self.set_user_hidden(false),
            CMD_TOGGLE => self.toggle_user_hidden(),
            CMD_QUIT => unsafe {
                let _ = DestroyWindow(self.hwnd);
            },
            _ => {}
        }
    }

    /// Open from the tray: unhide first if needed.
    fn open_from_tray(&self) {
        if self.user_hidden.get() {
            self.set_user_hidden(false);
        }
        self.open_sticky(None);
    }

    fn on_tray(&self, lp: LPARAM) {
        match (lp.0 & 0xFFFF) as u32 {
            WM_LBUTTONUP => {
                if self.open.get() {
                    self.collapse();
                } else {
                    self.open_from_tray();
                }
            }
            WM_RBUTTONUP | WM_CONTEXTMENU => {
                match shell::tray_menu(
                    self.hwnd,
                    self.paused.get(),
                    shell::autostart_enabled(),
                    self.user_hidden.get(),
                ) {
                    shell::ID_OPEN => self.open_from_tray(),
                    shell::ID_VISIBILITY => self.toggle_user_hidden(),
                    shell::ID_PAUSE => {
                        self.paused.set(!self.paused.get());
                        self.invalidate();
                    }
                    shell::ID_AUTOSTART => shell::set_autostart(!shell::autostart_enabled()),
                    shell::ID_QUIT => unsafe {
                        let _ = DestroyWindow(self.hwnd);
                    },
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn on_command(&self, wp: WPARAM, lp: LPARAM) {
        const EN_CHANGE: u32 = 0x0300;
        let code = ((wp.0 >> 16) & 0xFFFF) as u32;
        if code == EN_CHANGE && lp.0 == self.notes.hwnd().0 as isize {
            self.notes.mark_dirty();
            // restarting the same timer id debounces: save 1s after the last keystroke
            unsafe {
                SetTimer(Some(self.hwnd), T_SAVE, 1000, None);
            }
        }
    }

    fn on_dpi_changed(&self, dpi: u32) {
        self.dpi.set(dpi);
        if let Some(r) = self.renderer.borrow_mut().as_mut() {
            let _ = r.resize(window::px(WIN_W, dpi) as u32, window::px(WIN_H, dpi) as u32, dpi as f32);
        }
        self.origin.set(window::place(self.hwnd, dpi));
        let (w, h) = self.size();
        window::set_region(self.hwnd, dpi, w, h);
        if self.notes.is_visible() {
            self.show_notes();
        }
        self.invalidate();
    }

    fn reposition(&self) {
        self.on_dpi_changed(window::dpi_for(self.hwnd));
    }

    /// Called after the clipboard history changed (new/updated item): enforce limits, persist.
    pub fn after_store_change(&self) {
        {
            let mut s = self.store.borrow_mut();
            let removed = s.evict(self.settings.max_items, self.settings.max_image_mb * 1024 * 1024);
            for it in &removed {
                s.delete_files(it);
            }
            let _ = s.save();
        }
        self.invalidate_list();
        let n = self.row_count();
        self.scroll.set(self.scroll.get().min(self.max_scroll()));
        if let Some(k) = self.sel.get() {
            self.sel.set(if n == 0 { None } else { Some(k.min(n - 1)) });
        }
        if self.open.get() {
            self.refresh_hover(); // rows may have shifted under the pointer
            self.invalidate();
        }
    }

    fn cleanup(&self) {
        self.notes.flush();
        let _ = self.store.borrow().save();
        unsafe {
            let _ = RemoveClipboardFormatListener(self.hwnd);
        }
        shell::unregister_hotkeys(self.hwnd);
        shell::remove_tray(self.hwnd);
        shell::remove_foreground_hook(self.fg_hook.get());
    }

    // ---------------------------------------------------- window procedure

    pub fn handle(&self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        unsafe {
            match msg {
                WM_PAINT => {
                    let _ = ValidateRect(Some(hwnd), None);
                    self.render();
                    LRESULT(0)
                }
                WM_ERASEBKGND => LRESULT(1),
                WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
                WM_SETCURSOR => {
                    if (lp.0 & 0xFFFF) as u32 == HTCLIENT {
                        self.set_cursor();
                        LRESULT(1)
                    } else {
                        DefWindowProcW(hwnd, msg, wp, lp)
                    }
                }
                WM_MOUSEMOVE => {
                    let (x, y) = window::mouse_xy(lp);
                    self.on_mouse_move(x, y);
                    LRESULT(0)
                }
                WM_MOUSELEAVE => {
                    self.on_mouse_leave();
                    LRESULT(0)
                }
                WM_LBUTTONUP => {
                    let (x, y) = window::mouse_xy(lp);
                    self.on_click(x, y, key_down(VK_SHIFT));
                    LRESULT(0)
                }
                WM_LBUTTONDBLCLK => {
                    let (x, y) = window::mouse_xy(lp);
                    self.on_double_click(x, y, key_down(VK_SHIFT));
                    LRESULT(0)
                }
                WM_RBUTTONUP => {
                    let (x, y) = window::mouse_xy(lp);
                    self.on_right_click(x, y);
                    LRESULT(0)
                }
                WM_MOUSEWHEEL => {
                    self.on_wheel(((wp.0 >> 16) & 0xFFFF) as u16 as i16);
                    LRESULT(0)
                }
                WM_MOUSEHWHEEL => {
                    self.on_hwheel(((wp.0 >> 16) & 0xFFFF) as u16 as i16);
                    LRESULT(0)
                }
                WM_MBUTTONUP => {
                    let (x, y) = window::mouse_xy(lp);
                    self.on_middle_click(x, y);
                    LRESULT(0)
                }
                WM_KEYDOWN => {
                    self.on_key(wp.0 as u32);
                    LRESULT(0)
                }
                WM_CHAR => {
                    self.on_char(wp.0 as u16);
                    LRESULT(0)
                }
                WM_TIMER => {
                    self.on_timer(wp.0);
                    LRESULT(0)
                }
                WM_HOTKEY => {
                    self.on_hotkey(wp.0 as i32);
                    LRESULT(0)
                }
                WM_CLIPBOARDUPDATE => {
                    clipboard::on_update(self);
                    LRESULT(0)
                }
                WM_APP_CLIPDONE => {
                    clipboard::on_image_done(self, lp);
                    LRESULT(0)
                }
                WM_COMMAND => {
                    self.on_command(wp, lp);
                    LRESULT(0)
                }
                WM_ACTIVATE => {
                    if (wp.0 & 0xFFFF) as u32 == 0 {
                        self.post_focus_check(); // WA_INACTIVE
                    }
                    DefWindowProcW(hwnd, msg, wp, lp)
                }
                WM_APP_FOCUSCHECK => {
                    self.focus_check();
                    LRESULT(0)
                }
                WM_APP_FGCHANGE => {
                    // debounce: the notification state settles a moment after the switch
                    SetTimer(Some(hwnd), T_FS, 250, None);
                    LRESULT(0)
                }
                WM_APP_TRAY => {
                    self.on_tray(lp);
                    LRESULT(0)
                }
                WM_APP_COMMAND => {
                    self.on_app_command(wp.0);
                    LRESULT(0)
                }
                WM_DPICHANGED => {
                    self.on_dpi_changed(((wp.0 >> 16) & 0xFFFF) as u32);
                    LRESULT(0)
                }
                WM_DISPLAYCHANGE => {
                    self.reposition();
                    LRESULT(0)
                }
                WM_SETTINGCHANGE => {
                    // light/dark mode switched: the tray icon has a variant for each taskbar
                    if shell::is_theme_change(lp) {
                        shell::refresh_tray_icon(hwnd);
                    }
                    DefWindowProcW(hwnd, msg, wp, lp)
                }
                WM_QUERYENDSESSION | WM_ENDSESSION => {
                    self.notes.flush();
                    DefWindowProcW(hwnd, msg, wp, lp)
                }
                WM_DESTROY => {
                    self.cleanup();
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => {
                    if msg == self.taskbar_created && msg != 0 {
                        // Explorer restarted: put the tray icon back.
                        shell::add_tray(hwnd, WM_APP_TRAY, &self.tooltip.borrow());
                        return LRESULT(0);
                    }
                    DefWindowProcW(hwnd, msg, wp, lp)
                }
            }
        }
    }
}
