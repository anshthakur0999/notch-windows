//! Application state and behaviour: the expand/collapse state machine, input handling,
//! and the glue between the window, renderer, notepad, clipboard store and shell.
//!
//! Everything lives on the single UI thread. Mutable state uses `Cell`/`RefCell` per field
//! (never one big `RefCell<App>`) because Win32 calls made from inside a handler can
//! synchronously re-enter the window procedure.

use crate::anim::{self, Spring};
use crate::clipboard::{self, Formats};
use crate::notes::{NoteStyle, Notes};
use crate::peek::{self, Advance, Copied, Peek};
use crate::render::{self, F, Renderer};
use crate::settings::{Accent, Settings, TextSize, Theme};
use crate::cli::{CMD_HIDE, CMD_QUIT, CMD_SHOW, CMD_TOGGLE};
use crate::shell::{self, HK_CLIPBOARD, HK_TOGGLE, HK_VISIBILITY};
use crate::store::{Kind, Store, now_secs};
use crate::ui::clip_list::{self, ListLayout};
use crate::ui::theme::Palette;
use crate::ui::notes_view::{self, NoteTabs};
use crate::shelf::Shelf;
use crate::timer::{self, Phase, Timer};
use crate::ui::timer_view::PRESETS;
use crate::audio;
use crate::devices::{self, Device, Event};
use crate::media::{self, Cmd, Media, Track};
use crate::policy;
use crate::ui::{
    self, COLLAPSED_H, COLLAPSED_W, CARD_W, Capsule, CardView, DevicesView, EXPANDED_H, EXPANDED_W, ADDED_W, DROP_W, Hit, HitCtx, MediaView,
    NOTICE_MIN_W, PEEK_H, PEEK_TIMER_W, PeekView, SPARK_H, SPARK_W, ShelfView, TIMES_UP_W, TRACK_W, Tab, TimerField, TimerView, View, WIN_H,
    WIN_W, hover_w, rest_w,
};
use crate::pair;
use crate::ui::devices_tab::{self, Links, Row, Rows, Screen};
use crate::ui::devices_view::{Card, Notice, NoticeKind};
use crate::ui::{media_view, shelf_view};
use crate::viz::{self, Listener};
use crate::window;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::OnceLock;
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
pub const WM_APP_THUMB: u32 = WM_APP + 6;
/// The media worker has a new snapshot; the visualizer's bars have moved.
pub const WM_APP_MEDIA: u32 = WM_APP + 7;
pub const WM_APP_VIZ: u32 = WM_APP + 8;
/// The devices worker has a new list or something to say (a device connected, a battery ran low).
pub const WM_APP_DEVICES: u32 = WM_APP + 9;

// All timers are one-shot: nothing ticks while the pill is idle.
const T_DWELL: usize = 1;
const T_COLLAPSE: usize = 2;
const T_SAVE: usize = 3;
const T_FS: usize = 4;
const T_PASTE: usize = 5;
/// The copy-peek capsule has been up long enough: start taking it down.
const T_PEEK: usize = 6;
/// The timer's displayed second is about to change (or its time is up).
const T_TIMER: usize = 7;
/// The "Added 3 files" toast has been up long enough.
const T_SHELF: usize = 8;
/// The "now playing" banner has been up long enough.
const T_TRACK: usize = 9;
/// The Media tab's time labels are due a second tick.
const T_MEDIA: usize = 10;
/// The device card moves on to its next stage (the ring is done and it opens; it has been up long
/// enough).
const T_DEVICE: usize = 11;
/// A device that was the audio output disconnected a moment ago: Windows has moved the sound by now.
const T_GONE: usize = 12;
/// A ring is turning on the Devices tab (a connect, a search, a pairing): redraw it.
const T_SPIN: usize = 13;

/// How long the device capsule sweeps its ring before it opens, and how long the card and the short
/// notices stay up (longer while the pointer is on them).
const SPARK_MS: u32 = 150;
const CARD_HOLD_MS: u32 = 4000;
const NOTICE_HOLD_MS: u32 = 2800;
const GONE_WAIT_MS: u32 = 900;
/// After a device connects or goes, the Devices tab reads the playback devices again, up to this
/// many times (first quickly, then slowly: after a Connect the earbuds' playback device can take
/// half a minute to come up), and stops as soon as every connected audio device has one.
const OUT_REREADS: u8 = 18;
const OUT_REREAD_MS: u32 = 1500;
const OUT_REREAD_SLOW_MS: u32 = 4000;

/// The device card on show, and where it is in its life.
struct DevCard {
    card: Card,
    since: Instant,
    /// This card opens with the ring (a new connection does; the rest just widen), and whether it
    /// is still in that opening.
    spark: bool,
    sparking: bool,
    hold_ms: u32,
}

impl DevCard {
    /// The capsule's size now: the short one for the opening, then the card's own.
    fn size(&self) -> (f32, f32) {
        if self.sparking { (SPARK_W, SPARK_H) } else { self.card.size() }
    }
}

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

/// Seconds since the first ring was drawn: what the spinners turn by.
fn clock() -> f32 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
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

/// Applies one `WM_CHAR` to a text box (at most `max` characters): backspace, a character, or
/// half of a surrogate pair (`high` holds the first half until the second arrives). Returns true
/// if the text changed.
fn type_into(high: &Cell<u16>, s: &mut String, c: u16, max: usize) -> bool {
    match c {
        0x08 => s.pop().is_some(),
        c if c < 0x20 || c == 0x7F => false, // Enter, Esc, Ctrl+letter...
        c if (0xD800..0xDC00).contains(&c) => {
            high.set(c); // first half of an emoji etc.
            false
        }
        c => {
            let ch = if (0xDC00..0xE000).contains(&c) {
                let hi = high.replace(0);
                // with no first half pending, [0, c] would decode as a NUL
                (hi != 0).then(|| char::decode_utf16([hi, c]).next().and_then(|r| r.ok())).flatten()
            } else {
                char::from_u32(c as u32)
            };
            match ch {
                Some(ch) if s.chars().count() < max => {
                    s.push(ch);
                    true
                }
                _ => false,
            }
        }
    }
}

/// The colours for the current theme and accent. `Theme::System` follows Windows' app mode.
fn palette_for(s: &Settings) -> Palette {
    let light = match s.theme {
        Theme::Light => true,
        Theme::Dark => false,
        Theme::System => shell::apps_use_light_theme(),
    };
    Palette::new(light, s.accent)
}

pub struct App {
    pub hwnd: HWND,
    /// Loaded from `settings.json`; the tray menu changes the look settings at run time.
    settings: RefCell<Settings>,
    pub dpi: Cell<u32>,
    origin: Cell<(i32, i32)>,

    open: Cell<bool>,
    /// Stays open until dismissed (hotkey / click / typing) instead of closing on mouse-leave.
    pub sticky: Cell<bool>,
    /// The header pin: never auto-close (mouse leave, focus loss) until unpinned or collapsed.
    keep_open: Cell<bool>,
    /// The shelf's file picker is up: the panel must not close under it.
    picking: Cell<bool>,
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
    /// Highlighted clipboard row (by position in the filtered list): the one under the pointer.
    sel: Cell<Option<usize>>,
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
    /// The "Copied" capsule shown by the collapsed pill after a copy (see `peek.rs`).
    peek: RefCell<Option<Peek>>,
    /// The timer (one at a time) and its form: what is typed, which box has the caret.
    timer: RefCell<Timer>,
    timer_input: RefCell<String>,
    timer_label: RefCell<String>,
    timer_focus: Cell<TimerField>,
    /// The cursor rests on the collapsed pill while a timer runs: it widens into the hover capsule.
    hover_peek: Cell<bool>,
    /// Width the copy-peek capsule reserved for the timer when it was composed.
    peek_slot: Cell<f32>,
    /// When "stop" or "Done" was clicked: the form appears under the pointer, so a double-click
    /// must not press "Start" with its second click.
    last_timer_stop: Cell<Option<Instant>>,
    /// The file shelf, and what the pill is doing about it: a file drag is over the pill, our own
    /// drag out is in progress (the pill must not take its own files back), the "Added" toast
    /// (files dropped, files on the shelf), and a press on a tile that may turn into a drag.
    shelf: RefCell<Shelf>,
    drag_over: Cell<bool>,
    dragging_out: Cell<bool>,
    shelf_toast: Cell<Option<(usize, usize)>>,
    press: Cell<Option<(Hit, i32, i32)>>,
    /// Live media: the worker that follows Windows' media session, what it last reported, the
    /// "now playing" banner, whether a snapshot has been seen yet (what already played at launch
    /// gets no banner), and the visualizer's listener (only while it has something to draw).
    media: Media,
    track: RefCell<Option<Track>>,
    track_toast: Cell<bool>,
    media_seen: Cell<bool>,
    viz: RefCell<Option<Listener>>,
    /// Bluetooth devices: what is connected, the playback devices (read when the Devices tab
    /// shows them), the card on the collapsed pill, and a disconnect that was the audio output,
    /// waiting for Windows to move the sound before it says where it went.
    devs: RefCell<Vec<Device>>,
    outputs: RefCell<Vec<audio::Output>>,
    /// The width of each output's chip; the paired devices that are not connected; the connects and
    /// disconnects started from the tab; which screen it is on; what the Add screen found.
    out_w: RefCell<Vec<f32>>,
    out_retry: Cell<u8>,
    avail: RefCell<Vec<Device>>,
    links: RefCell<Links>,
    screen: Cell<Screen>,
    pairs: RefCell<pair::Snapshot>,
    devcard: RefCell<Option<DevCard>>,
    gone: RefCell<Option<Device>>,

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
        // size everything for the display the pill is going to be on
        let dpi = window::target_dpi(&settings.monitor).unwrap_or_else(|| window::dpi_for(hwnd));
        let mut renderer = Renderer::new(
            hwnd,
            window::px(WIN_W, dpi) as u32,
            window::px(WIN_H, dpi) as u32,
            dpi as f32,
            settings.use_gpu,
        )?;
        let pal = palette_for(&settings);
        let scale = settings.text_size.scale();
        renderer.set_look(pal, scale);
        let notes = Notes::new(dir.clone());
        notes.set_style(NoteStyle::new(&pal, scale));
        let mut shelf = Shelf::load(&dir);
        if shelf.prune(settings.shelf_hours, now_secs()) {
            let _ = shelf.save(&dir);
        }
        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        Ok(App {
            hwnd,
            settings: RefCell::new(settings),
            dpi: Cell::new(dpi),
            origin: Cell::new((0, 0)),
            open: Cell::new(false),
            sticky: Cell::new(false),
            keep_open: Cell::new(false),
            picking: Cell::new(false),
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
            peek: RefCell::new(None),
            timer: RefCell::new(Timer::new()),
            timer_input: RefCell::new("25m".into()),
            timer_label: RefCell::new("Focus".into()),
            timer_focus: Cell::new(TimerField::Time),
            hover_peek: Cell::new(false),
            peek_slot: Cell::new(0.0),
            last_timer_stop: Cell::new(None),
            shelf: RefCell::new(shelf),
            drag_over: Cell::new(false),
            dragging_out: Cell::new(false),
            shelf_toast: Cell::new(None),
            press: Cell::new(None),
            media: Media::start(hwnd, WM_APP_MEDIA),
            track: RefCell::new(None),
            track_toast: Cell::new(false),
            media_seen: Cell::new(false),
            viz: RefCell::new(None),
            devs: RefCell::new(Vec::new()),
            outputs: RefCell::new(Vec::new()),
            out_w: RefCell::new(Vec::new()),
            out_retry: Cell::new(0),
            avail: RefCell::new(Vec::new()),
            links: RefCell::new(Links::default()),
            screen: Cell::new(Screen::List),
            pairs: RefCell::new(pair::Snapshot::default()),
            devcard: RefCell::new(None),
            gone: RefCell::new(None),
            renderer: RefCell::new(Some(renderer)),
            store: RefCell::new(store),
            notes,
            clip_formats: clipboard::register_formats(),
            taskbar_created,
            tooltip: RefCell::new(String::from("Notch")),
            fg_hook: Cell::new(HWINEVENTHOOK::default()),
        })
    }

    /// Positions and shows the pill, registers tray/hotkeys/clipboard listener.
    pub fn startup(&self) {
        let dpi = self.dpi.get();
        self.origin.set(window::place(self.hwnd, dpi, &self.cfg().monitor));
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
        let (hk_notes, hk_clip, hk_vis) = {
            let s = self.cfg();
            (s.hotkey_toggle.clone(), s.hotkey_clipboard.clone(), s.hotkey_visibility.clone())
        };
        let notes_hk = shell::register_hotkey_or_fallback(self.hwnd, HK_TOGGLE, &hk_notes, "Ctrl+Alt+N");
        let clip_hk = shell::register_hotkey_or_fallback(self.hwnd, HK_CLIPBOARD, &hk_clip, "Ctrl+Alt+V");
        let vis_hk = shell::register_hotkey_or_fallback(self.hwnd, HK_VISIBILITY, &hk_vis, "Ctrl+Alt+H");
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
        shell::watch_fullscreen(self.hwnd);
        crate::dnd::register(self.hwnd);
        devices::start(self.hwnd, WM_APP_DEVICES);
    }

    pub fn is_animating(&self) -> bool {
        self.animating.get()
    }

    fn cfg(&self) -> std::cell::Ref<'_, Settings> {
        self.settings.borrow()
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
        !self.sticky.get() && !self.keep_open.get() && !self.picking.get()
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
        let quick = self.notes.quick();
        let mut t = notes_view::layout(r, &names, renaming, quick, self.tab_scroll.get());
        if self.reveal_tab.replace(false) {
            let s = notes_view::scroll_to_reveal(&t, renaming.unwrap_or(self.notes.active()));
            if (s - t.scroll).abs() > 0.01 {
                t = notes_view::layout(r, &names, renaming, quick, s);
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
        if self.renaming.get().is_some() {
            return; // the rename box is a window of its own and would not follow the tabs
        }
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

        self.peek_advance(now);

        let mut w = self.w.get();
        let mut h = self.h.get();
        // Opening (and the copy peek widening) has the springy overshoot; closing the panel is stiffer
        // and snaps shut; the copy peek easing back to the resting pill is in between.
        let (growing, peek_leaving) = {
            let p = self.peek.borrow();
            let widening = self.open.get() || p.as_ref().is_some_and(Peek::growing) || self.capsule() != Capsule::None;
            (widening, p.is_some())
        };
        let (freq, zeta) = if growing {
            (anim::FREQ, anim::ZETA)
        } else if peek_leaving {
            (anim::PEEK_OUT_FREQ, anim::PEEK_OUT_ZETA)
        } else {
            (anim::CLOSE_FREQ, anim::CLOSE_ZETA)
        };
        w.step_with(dt, freq, zeta);
        h.step_with(dt, freq, zeta);
        // a fade the springs do not drive (the peek's text going out) also keeps the loop alive
        let done = w.settled() && h.settled() && !self.peek.borrow().as_ref().is_some_and(|p| p.busy(now));
        if done {
            w.snap();
            h.snap();
        }
        self.w.set(w);
        self.h.set(h);
        self.reveal_notes_early(w.pos, h.pos);

        let t0 = Instant::now();
        window::set_region(self.hwnd, self.dpi.get(), w.pos, h.pos);
        let t1 = Instant::now();
        self.render();
        let t2 = Instant::now();
        if crate::log_enabled() {
            crate::log(&format!(
                "frame dt={:.1}ms region={:.2}ms render+present={:.2}ms w={:.1} h={:.1}",
                dt * 1000.0,
                (t1 - t0).as_secs_f32() * 1000.0,
                (t2 - t1).as_secs_f32() * 1000.0,
                w.pos,
                h.pos
            ));
        }

        if done {
            self.animating.set(false);
            self.on_settled();
        }
    }

    /// While the pill opens on the notepad tab, shows the note as soon as the pill is big enough to
    /// hold it (`ui::pill_holds_notes`) instead of when the spring has formally come to rest, which
    /// is a good 250 ms after the panel already looks open and left the note area blank.
    fn reveal_notes_early(&self, w: f32, h: f32) {
        if !self.open.get() || self.tab.get() != Tab::Notes || self.notes.is_visible() {
            return;
        }
        if !ui::pill_holds_notes(w, h) {
            return;
        }
        crate::log(&format!("notes shown early at w={w:.1} h={h:.1}"));
        self.show_notes();
        if self.want_focus.get() {
            self.want_focus.set(false);
            self.notes.focus();
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
        } else {
            {
                let mut peek = self.peek.borrow_mut();
                match peek.as_ref() {
                    // the capsule has finished widening and is being held: keep its thumbnail
                    Some(p) if p.growing() => return,
                    // it has shrunk back to the resting pill; the last frame was drawn with the peek
                    // up (no shelf count or timer on it), so repaint now it is gone
                    Some(_) => {
                        *peek = None;
                        self.invalidate();
                    }
                    None => {}
                }
            }
            if let Some(r) = self.renderer.borrow_mut().as_mut() {
                // fully closed: give back GPU thumbnails and the WIC factory
                r.release_transient();
            }
        }
    }

    // ------------------------------------------------------------ copy peek

    /// A copy just happened (text, or an image once it has been encoded): while the pill is
    /// collapsed, widen it into a capsule that says "Copied" and shows what it was.
    ///
    /// No peek when it would only be noise: the panel is open (the new entry is right there),
    /// the pill is hidden (a full-screen app is in front, or the user hid it), or the pill is
    /// still closing from a big size. Copies that are not recorded (password managers, paused,
    /// our own pastes) never get here.
    pub fn show_copy_peek(&self, copied: Copied) {
        let (enabled, preview, hold_ms) = {
            let s = self.cfg();
            (s.copy_peek, s.copy_peek_preview, s.copy_peek_ms)
        };
        // (the time's-up and hover capsules outrank a copy toast)
        if !enabled || self.open.get() || self.is_hidden() || self.capsule() != Capsule::None || self.h.get().pos > PEEK_H + 8.0 {
            crate::log(&format!(
                "copy peek skipped: enabled={enabled} open={} hidden={} h={:.0}",
                self.open.get(),
                self.is_hidden(),
                self.h.get().pos
            ));
            return;
        }
        let mut content = {
            let r = self.renderer.borrow();
            let Some(r) = r.as_ref() else { return };
            let label_w = r.text_w(F::Seg, "Copied");
            peek::compose(copied, preview, label_w, |line| r.measure(F::Tab, line))
        };
        // a running timer keeps the right end of the capsule: the copy takes the left
        let timer_w = if self.timer.borrow().active() { PEEK_TIMER_W } else { 0.0 };
        content.target_w += timer_w;
        self.peek_slot.set(timer_w);
        let target_w = content.target_w;
        let now = Instant::now();
        let reduced = shell::reduced_motion();
        crate::log(&format!("copy peek: {:?} width={target_w:.0} reduced={reduced}", content.line));
        {
            let mut slot = self.peek.borrow_mut();
            match slot.as_mut() {
                // another copy while the capsule is up: cross-fade to the new content, hold again,
                // no replay
                Some(p) => p.swap(content, now),
                None => *slot = Some(Peek::new(content, reduced, now)),
            }
        }
        self.set_targets(target_w, PEEK_H);
        if reduced {
            // no widening: the capsule is simply there and fades in
            let (mut w, mut h) = (self.w.get(), self.h.get());
            w.snap();
            h.snap();
            self.w.set(w);
            self.h.set(h);
            window::set_region(self.hwnd, self.dpi.get(), w.pos, h.pos);
        }
        unsafe {
            // restarting the one-shot timer holds the capsule for another full period
            SetTimer(Some(self.hwnd), T_PEEK, hold_ms, None);
        }
        self.invalidate();
    }

    /// The hold is over: take the capsule down, unless the cursor is on it.
    fn peek_hold_over(&self) {
        if self.peek.borrow().is_none() {
            return;
        }
        if self.cursor_in_pill() {
            unsafe {
                SetTimer(Some(self.hwnd), T_PEEK, 250, None); // look again shortly
            }
            return;
        }
        let reduced = {
            let mut slot = self.peek.borrow_mut();
            let Some(p) = slot.as_mut() else { return };
            p.begin_leave(Instant::now());
            p.reduced
        };
        if reduced {
            self.start_anim(); // the fade-out needs the frame loop; the size does not move
        } else {
            // the width starts easing back at once, while the text is still fading out
            self.to_rest();
        }
    }

    /// Per frame: reduced motion only, drop the peek once it has faded out.
    fn peek_advance(&self, now: Instant) {
        let step = self.peek.borrow_mut().as_mut().map(|p| p.advance(now));
        match step {
            Some(Advance::Finished) => {
                // reduced motion: the capsule has faded out; go straight back to the resting size
                *self.peek.borrow_mut() = None;
                self.to_rest();
                let (mut w, mut h) = (self.w.get(), self.h.get());
                w.snap();
                h.snap();
                self.w.set(w);
                self.h.set(h);
            }
            _ => {}
        }
    }

    /// Drops the peek at once (the pill is opening or being hidden).
    fn peek_clear(&self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), T_PEEK);
        }
        *self.peek.borrow_mut() = None;
    }

    // ----------------------------------------------------------------- timer

    /// The collapsed pill's shape: a capsule while the time is up, or while the cursor rests on it
    /// with a timer running; the plain pill otherwise.
    fn capsule(&self) -> Capsule {
        Capsule::pick(
            self.open.get(),
            self.timer.borrow().phase(),
            self.hover_peek.get(),
            self.drag_over.get(),
            self.shelf_toast.get().is_some(),
            self.track.borrow().is_some(),
            self.track_toast.get(),
            self.devcard.borrow().is_some(),
        )
    }

    /// The size the pill rests at while it is not open.
    fn rest_size(&self) -> (f32, f32) {
        let (timer, media, shelf) = (self.timer.borrow().active(), self.track.borrow().is_some(), self.shelf.borrow().len() > 0);
        match self.capsule() {
            Capsule::TimesUp => (TIMES_UP_W, PEEK_H),
            Capsule::Hover => (hover_w(timer, shelf, media), PEEK_H),
            Capsule::Drop => (DROP_W, PEEK_H),
            Capsule::Added => (ADDED_W, PEEK_H),
            Capsule::Track => (TRACK_W, PEEK_H),
            Capsule::Device => self.devcard.borrow().as_ref().map_or((SPARK_W, SPARK_H), DevCard::size),
            Capsule::None => (rest_w(timer, media, shelf), COLLAPSED_H),
        }
    }

    fn to_rest(&self) {
        let (w, h) = self.rest_size();
        self.set_targets(w, h);
    }

    /// The timer changed the pill's shape (stopped, extended, dismissed): go to the right resting
    /// size, unless the panel is open or a copy toast is on show.
    fn settle_capsule(&self) {
        if !self.open.get() && self.peek.borrow().is_none() && !self.is_hidden() {
            self.to_rest();
        }
    }

    /// The cursor has rested on the collapsed pill: with a timer running it widens into the hover
    /// capsule instead of opening the panel.
    fn on_dwell(&self) {
        if self.timer.borrow().active() || self.track.borrow().is_some() {
            self.hover_peek.set(true);
            self.peek_clear();
            self.to_rest();
        } else {
            self.expand();
        }
    }

    /// The timer's state changed: set the one wake-up at the next digit change (none unless it is
    /// running), and redraw.
    fn timer_changed(&self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), T_TIMER);
        }
        if let Some(wait) = self.timer.borrow().next_tick(Instant::now()) {
            unsafe {
                SetTimer(Some(self.hwnd), T_TIMER, wait.as_millis() as u32, None);
            }
        }
        self.invalidate();
    }

    fn timer_tick(&self) {
        if self.timer.borrow_mut().tick(Instant::now()) {
            self.on_timer_finished();
        }
        self.timer_changed();
    }

    /// The time is up: the pill widens into the "Time's up" capsule. With the pill hidden (a
    /// full-screen app in front, or hidden on request) a Windows notification says so instead.
    fn on_timer_finished(&self) {
        if self.is_hidden() {
            let label = self.timer.borrow().label.clone();
            shell::notify(self.hwnd, "Time's up", if label.is_empty() { "Your timer has finished." } else { &label });
        }
        self.peek_clear(); // an alarm outranks a copy toast
        self.settle_capsule();
    }

    fn start_timer(&self) {
        let Some(secs) = timer::parse(&self.timer_input.borrow()) else { return };
        let label = self.timer_label.borrow().clone();
        self.timer.borrow_mut().start(secs, &label, Instant::now());
        self.timer_changed();
    }

    fn timer_pause_toggle(&self) {
        self.timer.borrow_mut().toggle_pause(Instant::now());
        self.timer_changed();
    }

    /// "+1 min" and "+5 min".
    fn timer_add(&self, secs: u64) {
        self.timer.borrow_mut().add(secs, Instant::now());
        self.timer_changed();
        self.settle_capsule(); // "+5 min" on the time's-up capsule: back to the plain pill
    }

    /// Stop (x) or Done: back to the form.
    fn timer_stop(&self) {
        self.timer.borrow_mut().cancel();
        self.last_timer_stop.set(Some(Instant::now()));
        self.timer_changed();
        self.settle_capsule();
    }

    /// Clicking a box of the form means "I want to type": take keyboard focus.
    fn focus_timer_box(&self, field: TimerField) {
        self.timer_focus.set(field);
        self.sticky.set(true);
        self.activate_pill();
        self.invalidate();
    }

    /// A click on a control of the Timer tab.
    fn on_timer_click(&self, hit: Hit) {
        let just_stopped = self.last_timer_stop.get().is_some_and(|t| t.elapsed() < Duration::from_millis(450));
        match hit {
            Hit::TimerField => self.focus_timer_box(TimerField::Time),
            Hit::TimerLabel => self.focus_timer_box(TimerField::Label),
            Hit::TimerPreset(i) => {
                *self.timer_input.borrow_mut() = PRESETS[i].1.to_string();
                self.invalidate();
            }
            Hit::TimerStart if !just_stopped => self.start_timer(),
            Hit::TimerStop | Hit::TimerDone => self.timer_stop(),
            Hit::TimerPause => self.timer_pause_toggle(),
            Hit::TimerPlus => self.timer_add(60),
            Hit::TimerAdd5 => self.timer_add(300),
            _ => {}
        }
    }

    // ----------------------------------------------------------------- media

    /// `WM_APP_MEDIA`: the worker has a new snapshot of what plays. A different track while the pill
    /// is closed gets the "now playing" banner.
    fn on_media(&self) {
        if let Some(art) = media::take_art() {
            if let Some(r) = self.renderer.borrow_mut().as_mut() {
                r.set_media_art(art);
            }
        }
        let next = media::track();
        let fresh = next.as_ref().is_some_and(|n| n.playing && media::is_new_track(self.track.borrow().as_ref(), n));
        crate::log(&format!(
            "media: {:?} fresh={fresh}",
            next.as_ref().map(|t| (&t.title, &t.artist, &t.source, t.playing, t.len))
        ));
        *self.track.borrow_mut() = next;
        let seen = self.media_seen.replace(true);
        if self.track.borrow().is_none() {
            self.track_toast_clear();
        } else if fresh && seen {
            self.show_track_banner();
        }
        self.sync_viz();
        self.media_clock();
        self.settle_capsule(); // the resting pill gains or loses its media chip
        self.invalidate();
    }

    /// The banner is for the closed pill, when nothing is in the way of it.
    fn show_track_banner(&self) {
        if self.open.get() || self.is_hidden() || self.peek.borrow().is_some() {
            return;
        }
        self.track_toast.set(true);
        unsafe {
            SetTimer(Some(self.hwnd), T_TRACK, 3600, None);
        }
    }

    fn track_toast_clear(&self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), T_TRACK);
        }
        if self.track_toast.replace(false) {
            self.settle_capsule();
            self.invalidate();
        }
    }

    /// Listens for the visualizer only while there is something to draw: music is playing, the
    /// setting is on, and the bars can be seen (the closed pill, or the Media tab). Otherwise nothing
    /// is sampled and no thread runs.
    fn sync_viz(&self) {
        let want = self.cfg().visualizer
            && self.track.borrow().as_ref().is_some_and(|t| t.playing)
            && !self.is_hidden()
            && (!self.open.get() || self.tab.get() == Tab::Media);
        viz::set_big(self.open.get());
        let mut listener = self.viz.borrow_mut();
        if want && listener.is_none() {
            *listener = Some(Listener::start(self.hwnd, WM_APP_VIZ));
        } else if !want && listener.is_some() {
            *listener = None;
        }
    }

    /// The Media tab's time labels need a tick each second while it is open and playing (the bars
    /// redraw them when they run, but not in silence or with the visualizer off).
    fn media_clock(&self) {
        let wanted = self.open.get() && self.tab.get() == Tab::Media && self.track.borrow().as_ref().is_some_and(|t| t.playing);
        unsafe {
            if wanted {
                SetTimer(Some(self.hwnd), T_MEDIA, 1000, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), T_MEDIA);
            }
        }
    }

    /// A click on a media control (on the Media tab, or on the hover capsule's play button).
    fn on_media_click(&self, hit: Hit, x: f32) {
        match hit {
            Hit::MediaPlay | Hit::PeekPlay => self.media.send(Cmd::Toggle),
            Hit::MediaPrev => self.media.send(Cmd::Prev),
            Hit::MediaNext => self.media.send(Cmd::Next),
            Hit::MediaSeek => {
                if let Some(t) = self.track.borrow_mut().as_mut().filter(|t| t.len > 0.0) {
                    let to = media_view::seek_frac(x) as f64 * t.len;
                    self.media.send(Cmd::Seek(to));
                    // the bar moves now; Windows confirms the new position a moment later
                    (t.pos, t.at) = (to, Instant::now());
                }
                self.invalidate();
            }
            Hit::MediaStyle => {
                let on = !self.cfg().visualizer;
                self.settings.borrow_mut().visualizer = on;
                self.save_settings();
                self.sync_viz();
                self.invalidate();
            }
            _ => {}
        }
    }

    // --------------------------------------------------------------- devices

    /// `WM_APP_DEVICES`: the worker has a new list of connected devices and maybe news.
    fn on_devices(&self) {
        let list = devices::list();
        // a card still up shows the battery as it arrives (it usually follows the connection)
        if let Some(c) = self.devcard.borrow_mut().as_mut() {
            if let Card::Connected(d) | Card::Low(d) = &mut c.card {
                if let Some(n) = list.iter().find(|n| n.name == d.name) {
                    d.battery = n.battery;
                }
            }
        }
        *self.devs.borrow_mut() = list;
        *self.avail.borrow_mut() = devices::available();
        let on_tab = self.open.get() && self.tab.get() == Tab::Devices;
        if on_tab {
            self.refresh_outputs();
        }
        let now = Instant::now();
        let mut moved = false;
        for ev in devices::take_events() {
            crate::log(&format!("devices: {ev:?}"));
            moved |= matches!(ev, Event::Connected { .. } | Event::Disconnected(_));
            match ev {
                Event::Link { dev, connect, ok } => {
                    if let Some(addr) = dev.addr {
                        self.links.borrow_mut().finish(addr, connect, ok, now);
                    }
                }
                // a connect the user asked for shows on its row of the tab; it is not announced
                Event::Connected { dev, .. } if self.links.borrow().ours(dev.addr, now) => {}
                Event::Connected { dev, back: false } => self.show_device_card(Card::Connected(dev)),
                // a mouse or keyboard that dozes off and wakes is not news; earbuds coming back are
                Event::Connected { dev, back: true } if dev.kind.is_audio() => {
                    let name = dev.name.clone();
                    self.show_notice(NoticeKind::Back, dev.kind, name, Some("Reconnected".into()));
                }
                Event::Connected { back: true, .. } => {}
                Event::Low(dev) => self.show_device_card(Card::Low(dev)),
                Event::Disconnected(dev) => self.on_device_gone(dev),
            }
        }
        if on_tab && moved {
            // Windows moves the sound to another output a moment after a device goes, and a new
            // one's playback device turns up a few seconds after it connects: look again a few times
            // shortcut: re-reads on a short schedule; an IMMNotificationClient would say exactly when
            self.out_retry.set(OUT_REREADS);
            unsafe {
                SetTimer(Some(self.hwnd), T_GONE, GONE_WAIT_MS, None);
            }
        }
        self.sync_pairing();
        self.links.borrow_mut().prune(now);
        self.spin_clock();
        self.invalidate();
    }

    /// The Add screen follows what the pairing worker found; it ends when a pairing worked or
    /// Bluetooth went off.
    fn sync_pairing(&self) {
        if self.screen.get() != Screen::Add {
            return;
        }
        if !devices::bluetooth_on() || pair::take_paired() {
            self.close_add();
        } else {
            *self.pairs.borrow_mut() = pair::snapshot();
        }
    }

    /// What the Devices tab lists right now, as owned data (`.shape()` borrows it).
    fn tab_rows(&self) -> Rows {
        let now = Instant::now();
        let (links, pairs) = (self.links.borrow(), self.pairs.borrow());
        let near = pairs
            .nearby
            .iter()
            .map(|n| {
                let busy = pairs.pairing.as_deref() == Some(n.id.as_str());
                let failed = pairs.failed.as_deref() == Some(n.id.as_str());
                (if busy { Row::Busy } else if failed { Row::Failed } else { Row::Idle }, busy && pairs.code.is_some())
            })
            .collect();
        Rows {
            screen: self.screen.get(),
            off: !devices::bluetooth_on(),
            chips: self.out_w.borrow().clone(),
            conn: self.devs.borrow().iter().map(|d| links.row(d.addr, false, now)).collect(),
            avail: self.avail.borrow().iter().map(|d| links.row(d.addr, true, now)).collect(),
            near,
        }
    }

    /// Keeps the ring turning (about 30 times a second) only while one is on show: a connect or
    /// disconnect under way, a search, a pairing. Otherwise nothing ticks.
    fn spin_clock(&self) {
        let pairs = self.pairs.borrow();
        let on_add = self.screen.get() == Screen::Add && (pairs.searching || pairs.pairing.is_some());
        let wanted = self.open.get() && self.tab.get() == Tab::Devices && (on_add || self.links.borrow().busy(Instant::now()));
        unsafe {
            if wanted {
                SetTimer(Some(self.hwnd), T_SPIN, 33, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), T_SPIN);
            }
        }
    }

    /// Connect (`connect`: from the Available list) or Disconnect (from the Connected one), or
    /// "Try again" on either.
    fn on_link_click(&self, i: usize, connect: bool) {
        let dev = if connect { self.avail.borrow().get(i).cloned() } else { self.devs.borrow().get(i).cloned() };
        let Some(dev) = dev else { return };
        let Some(addr) = dev.addr else { return };
        crate::log(&format!("link click: {} {}", if connect { "connect" } else { "disconnect" }, dev.name));
        self.links.borrow_mut().start(addr, connect);
        devices::link(dev, connect);
        self.spin_clock();
        self.invalidate();
    }

    /// "Add device": the search starts, and only now.
    fn open_add(&self) {
        if !devices::bluetooth_on() {
            return;
        }
        self.screen.set(Screen::Add);
        self.scroll.set(0.0);
        self.hover.set(Hit::None);
        pair::start_search();
        *self.pairs.borrow_mut() = pair::snapshot();
        self.spin_clock();
        self.invalidate();
    }

    /// Back to the lists: the search stops, a pairing is cancelled.
    fn close_add(&self) {
        pair::close();
        self.screen.set(Screen::List);
        self.scroll.set(0.0);
        self.hover.set(Hit::None);
        *self.pairs.borrow_mut() = pair::Snapshot::default();
        self.spin_clock();
        self.invalidate();
    }

    /// Leaving the tab (another tab, or the panel folding): failures do not follow, the search ends.
    fn leave_devices(&self) {
        self.links.borrow_mut().clear_failed();
        if self.screen.get() == Screen::Add {
            self.close_add();
        }
    }

    /// Stop, or Search again.
    fn on_add_stop(&self) {
        if self.pairs.borrow().searching {
            pair::stop_search();
        } else if self.pairs.borrow().pairing.is_none() {
            pair::start_search();
        }
        *self.pairs.borrow_mut() = pair::snapshot();
        self.spin_clock();
        self.invalidate();
    }

    fn on_add_pair(&self, i: usize) {
        let id = self.pairs.borrow().nearby.get(i).map(|n| n.id.clone());
        if let Some(id) = id {
            pair::pair(&id);
            *self.pairs.borrow_mut() = pair::snapshot();
            self.spin_clock();
            self.invalidate();
        }
    }

    fn on_add_cancel(&self) {
        pair::cancel();
        *self.pairs.borrow_mut() = pair::snapshot();
        self.spin_clock();
        self.invalidate();
    }

    /// Reads the playback devices again (the Devices tab shows them).
    fn refresh_outputs(&self) {
        let outs = audio::outputs();
        let widths = match self.renderer.borrow().as_ref() {
            Some(r) => outs.iter().map(|o| devices_tab::chip_width(r.measure(F::Seg, devices_tab::chip_label(o)))).collect(),
            None => vec![120.0; outs.len()],
        };
        if crate::log_enabled() {
            crate::log(&format!("outputs: {:?}", outs.iter().map(|o| format!("{} ({}){}", o.name, o.sub, if o.default { "*" } else { "" })).collect::<Vec<_>>()));
        }
        *self.out_w.borrow_mut() = widths;
        *self.outputs.borrow_mut() = outs;
    }

    /// The card is for the closed pill, when nothing is in the way of it and the setting is on. A
    /// new connection opens with the ring; with reduced motion in Windows it just appears.
    fn show_device_card(&self, card: Card) {
        if !self.cfg().device_banner || self.open.get() || self.is_hidden() || self.peek.borrow().is_some() {
            return;
        }
        let reduced = shell::reduced_motion();
        let spark = matches!(card, Card::Connected(_)) && !reduced;
        let hold_ms = if matches!(card, Card::Notice(_)) { NOTICE_HOLD_MS } else { CARD_HOLD_MS };
        *self.devcard.borrow_mut() = Some(DevCard { card, since: Instant::now(), spark, sparking: spark, hold_ms });
        unsafe {
            SetTimer(Some(self.hwnd), T_DEVICE, if spark { SPARK_MS } else { hold_ms }, None);
        }
        self.to_rest();
        if reduced {
            let (mut w, mut h) = (self.w.get(), self.h.get());
            w.snap();
            h.snap();
            self.w.set(w);
            self.h.set(h);
            window::set_region(self.hwnd, self.dpi.get(), w.pos, h.pos);
        }
        self.invalidate();
    }

    /// A short notice, its width fitted to its text.
    fn show_notice(&self, kind: NoticeKind, icon: devices::Kind, title: String, sub: Option<String>) {
        let w = {
            let r = self.renderer.borrow();
            let Some(r) = r.as_ref() else { return };
            let text = r.measure(F::Seg, &title).max(sub.as_deref().map_or(0.0, |s| r.measure(F::Hint, s)));
            (54.0 + text + if kind == NoticeKind::Switched { 36.0 } else { 14.0 }).clamp(NOTICE_MIN_W, CARD_W)
        };
        self.show_device_card(Card::Notice(Notice { kind, icon, title, sub, w }));
    }

    /// `T_DEVICE`: the ring is done, so the card opens; or the card has been up long enough, so it
    /// folds back, unless the pointer is on it.
    fn device_stage(&self) {
        let hold = {
            let mut slot = self.devcard.borrow_mut();
            let Some(c) = slot.as_mut() else { return };
            if c.sparking {
                c.sparking = false;
                Some(c.hold_ms)
            } else {
                None
            }
        };
        if let Some(ms) = hold {
            unsafe {
                SetTimer(Some(self.hwnd), T_DEVICE, ms, None);
            }
            self.to_rest(); // the card's size
            self.invalidate();
        } else if self.cursor_in_pill() {
            unsafe {
                SetTimer(Some(self.hwnd), T_DEVICE, 250, None); // look again shortly
            }
        } else {
            self.device_clear();
        }
    }

    /// Takes the card down (it ran out, the x, the panel opened, the pill was hidden).
    fn device_clear(&self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), T_DEVICE);
        }
        let had = self.devcard.borrow_mut().take().is_some();
        if had {
            self.settle_capsule();
            self.invalidate();
        }
    }

    /// "Use for audio": make the earbuds' playback device the default, and say whether that worked.
    fn on_device_use(&self) {
        let dev = match self.devcard.borrow().as_ref().map(|c| &c.card) {
            Some(Card::Connected(d)) => d.clone(),
            _ => return,
        };
        let outs = audio::outputs();
        let ok = audio::output_for(&dev.name, &outs).is_some_and(|o| policy::set_default(&o.id));
        crate::log(&format!("use for audio: {} -> {ok}", dev.name));
        let (kind, sub) = if ok { (NoticeKind::Switched, "Now the audio output") } else { (NoticeKind::Gone, "No audio output for it yet") };
        self.show_notice(kind, dev.kind, dev.name, Some(sub.into()));
    }

    /// A device disconnected. If it was the sound's way out, Windows moves the sound to another
    /// device a moment later: wait for that before saying where it went.
    fn on_device_gone(&self, dev: Device) {
        // only earbuds and the like are announced: a mouse that dozes off is not news
        if !self.cfg().device_banner || !dev.kind.is_audio() {
            return;
        }
        // shortcut: if Windows has already moved the sound by the time the disconnect arrives, this
        // says "no" and the plain banner shows; fix by remembering the default as it changes
        let was_output = audio::outputs().iter().any(|o| o.default && audio::output_for(&dev.name, std::slice::from_ref(o)).is_some());
        if was_output {
            *self.gone.borrow_mut() = Some(dev);
            unsafe {
                SetTimer(Some(self.hwnd), T_GONE, GONE_WAIT_MS, None);
            }
        } else {
            let title = format!("{} disconnected", dev.name);
            self.show_notice(NoticeKind::Gone, dev.kind, title, None);
        }
    }

    /// `T_GONE`: the disconnect that was the audio output, with where the sound went.
    fn gone_resolved(&self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), T_GONE);
        }
        self.refresh_outputs();
        self.invalidate();
        // done once every connected audio device has its playback device (a disconnect has none to wait for)
        let waiting = {
            let outs = self.outputs.borrow();
            self.devs.borrow().iter().any(|d| d.kind.is_audio() && audio::output_for(&d.name, &outs).is_none())
        };
        let left = self.out_retry.get();
        if left > 0 && waiting && self.open.get() && self.tab.get() == Tab::Devices {
            self.out_retry.set(left - 1);
            unsafe {
                SetTimer(Some(self.hwnd), T_GONE, if left > OUT_REREADS / 2 { OUT_REREAD_MS } else { OUT_REREAD_SLOW_MS }, None);
            }
        } else {
            self.out_retry.set(0);
        }
        let Some(dev) = self.gone.borrow_mut().take() else { return };
        let outs = audio::outputs();
        let back = outs.iter().find(|o| o.default).filter(|o| audio::output_for(&dev.name, std::slice::from_ref(o)).is_none());
        let sub = back.map(|o| format!("Audio back on {}", o.name));
        self.show_notice(NoticeKind::Gone, dev.kind, format!("{} disconnected", dev.name), sub);
    }

    /// A click on an audio output of the Devices tab: it becomes the default.
    fn on_output_click(&self, i: usize) {
        let Some(id) = self.outputs.borrow().get(i).filter(|o| !o.default).map(|o| o.id.clone()) else { return };
        crate::log(&format!("set default output: {}", policy::set_default(&id)));
        self.refresh_outputs();
        self.invalidate();
    }

    /// The empty Devices tab's button: Windows' own Bluetooth page.
    fn open_bt_settings(&self) {
        unsafe {
            windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("ms-settings:bluetooth"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        }
    }

    // ----------------------------------------------------------------- shelf

    fn save_shelf(&self) {
        if let Err(e) = self.shelf.borrow().save(&data_dir()) {
            crate::log(&format!("could not save the shelf: {e}"));
        }
    }

    /// Drops files held longer than the `shelf_hours` setting. True if any went.
    fn shelf_prune(&self) -> bool {
        let hours = self.cfg().shelf_hours;
        let changed = self.shelf.borrow_mut().prune(hours, now_secs());
        if changed {
            self.save_shelf();
        }
        changed
    }

    /// Re-reads what the tiles show (size, still there?) as the Shelf tab is about to be seen.
    fn shelf_refresh(&self) {
        self.shelf_prune();
        self.shelf.borrow_mut().refresh();
        self.clamp_shelf_scroll();
        self.request_thumbs();
    }

    /// Asks the shell for a preview of every file on the shelf that has not been asked for yet.
    fn request_thumbs(&self) {
        let mut guard = self.renderer.borrow_mut();
        let Some(r) = guard.as_mut() else { return };
        let want: Vec<String> = {
            let shelf = self.shelf.borrow();
            shelf.items().iter().filter(|i| !i.missing).filter(|i| r.shelf_asked.insert(i.path.clone())).map(|i| i.path.clone()).collect()
        };
        if !want.is_empty() {
            crate::thumbs::request(self.hwnd, WM_APP_THUMB, want);
        }
    }

    /// `WM_APP_THUMB`: previews arrived; the open Shelf tab shows them.
    fn on_thumbs(&self) {
        if let Some(r) = self.renderer.borrow_mut().as_mut() {
            crate::thumbs::take().into_iter().for_each(|t| r.add_shelf_thumb(t));
        }
        if self.open.get() && self.tab.get() == Tab::Shelf {
            self.invalidate();
        }
    }

    fn clamp_shelf_scroll(&self) {
        let max = shelf_view::max_scroll(self.shelf.borrow().len());
        self.scroll.set(self.scroll.get().clamp(0.0, max));
    }

    /// Files can be dropped on the pill, unless it is hidden or the drag is our own.
    pub fn drag_accepts(&self) -> bool {
        !self.dragging_out.get() && !self.is_hidden()
    }

    /// A file drag entered (`over`) or left the pill: it widens into "Drop to shelf". Returns
    /// whether the files would be taken.
    pub fn on_drag(&self, over: bool) -> bool {
        if over && !self.drag_accepts() {
            return false;
        }
        if self.drag_over.replace(over) != over {
            if !self.open.get() {
                self.peek_clear();
                self.settle_capsule();
            }
            self.invalidate();
        }
        over
    }

    /// Files were dropped on the pill: they go on the shelf. With the panel open it shows them; with
    /// the pill collapsed a short toast says so.
    pub fn on_drop(&self, files: Vec<String>) -> bool {
        if self.dragging_out.get() || files.is_empty() {
            self.on_drag(false);
            return false;
        }
        self.drag_over.set(false);
        let total = {
            let mut s = self.shelf.borrow_mut();
            s.add(&files, now_secs());
            s.len()
        };
        self.save_shelf();
        self.scroll.set(0.0);
        if self.open.get() {
            self.set_tab(Tab::Shelf);
            self.request_thumbs();
        } else {
            self.shelf_toast.set(Some((files.len(), total)));
            unsafe {
                SetTimer(Some(self.hwnd), T_SHELF, 2400, None);
            }
            self.peek_clear();
            self.settle_capsule();
        }
        self.invalidate();
        true
    }

    /// Takes the "Added" toast down.
    fn shelf_toast_clear(&self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), T_SHELF);
        }
        if self.shelf_toast.replace(None).is_some() {
            self.settle_capsule();
            self.invalidate();
        }
    }

    /// Shows the file picker. The panel stays open meanwhile (the dialog takes the focus and the
    /// pointer leaves the pill, either of which would close it), then goes back to its usual rules.
    fn pick_files(&self) -> Vec<String> {
        self.picking.set(true);
        let files = crate::dnd::pick_files(self.hwnd);
        self.picking.set(false);
        if self.open.get() {
            if self.sticky.get() {
                // the pill is no-activate, so it does not get the foreground back by itself: without
                // it Esc and a click on another app would stop closing the panel
                self.activate_pill();
                if unsafe { GetForegroundWindow() } != self.hwnd {
                    self.sticky.set(false);
                    self.arm_collapse();
                }
            } else if self.auto_close_ok() && !self.cursor_in_pill() {
                self.arm_collapse();
            }
        }
        files
    }

    /// A click on a tile (its x removes it, anywhere else ticks it unless it is missing), on "Clear"
    /// or on the "+" slot.
    fn on_shelf_click(&self, hit: Hit, x: f32, y: f32) {
        match hit {
            Hit::ShelfTile(i) => {
                let (bx, by) = shelf_view::badge(shelf_view::tile(i, self.scroll.get()));
                let mut shelf = self.shelf.borrow_mut();
                if (x - bx).hypot(y - by) <= 13.0 {
                    shelf.remove(i);
                    drop(shelf);
                    self.save_shelf();
                } else if !shelf.items().get(i).is_some_and(|it| it.missing) {
                    shelf.toggle(i);
                }
            }
            Hit::ShelfClear => {
                self.shelf.borrow_mut().clear();
                self.save_shelf();
            }
            Hit::ShelfSlot => {
                let files = self.pick_files();
                if !files.is_empty() {
                    self.on_drop(files);
                }
            }
            _ => return,
        }
        self.clamp_shelf_scroll();
        self.refresh_hover();
        self.invalidate();
    }

    /// The button went down: on a tile that may be the start of a drag out.
    fn on_press(&self, px: i32, py: i32) {
        self.press.set(None);
        if !self.expanded() {
            return;
        }
        let hit = self.hit_at(self.to_dip(px), self.to_dip(py));
        if matches!(hit, Hit::ShelfTile(_)) {
            self.press.set(Some((hit, px, py)));
        }
    }

    /// The pointer moved with the button held: far enough from where it went down and it is a drag.
    fn maybe_start_drag(&self, px: i32, py: i32) {
        let Some((hit, x0, y0)) = self.press.get() else { return };
        let (dx, dy) = unsafe { (GetSystemMetrics(SM_CXDRAG).max(4), GetSystemMetrics(SM_CYDRAG).max(4)) };
        if (px - x0).abs() >= dx || (py - y0).abs() >= dy {
            self.press.set(None);
            self.start_drag_out(hit);
        }
    }

    /// Drags the files of a tile (the ticked ones, if it is one of them) or all of them out to
    /// whatever the user drops them on. Blocks until the button is released. The panel shrinks to
    /// the pill first, so it does not cover the target.
    fn start_drag_out(&self, hit: Hit) {
        let files = {
            let shelf = self.shelf.borrow();
            match hit {
                Hit::ShelfTile(i) => shelf.drag_set(i),
                _ => Vec::new(),
            }
        };
        if files.is_empty() {
            return;
        }
        crate::log(&format!("drag out: {} file(s)", files.len()));
        self.dragging_out.set(true);
        self.collapse();
        let (mut w, mut h) = (self.w.get(), self.h.get());
        w.snap();
        h.snap();
        self.w.set(w);
        self.h.set(h);
        window::set_region(self.hwnd, self.dpi.get(), w.pos, h.pos);
        self.render();
        let took = crate::dnd::drag_out(&files);
        crate::log(&format!("drag out finished: took={took}"));
        self.dragging_out.set(false);
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
        self.shelf_prune(); // so the count never shows a file that has expired
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
            let peek = self.peek.borrow();
            let (Some(list), Some(tabs)) = (list.as_ref(), tabs.as_ref()) else { return };
            let (w, h) = self.size();
            let peek_view = peek.as_ref().map(|p| PeekView {
                content: &p.content,
                prev: p.prev.as_ref(),
                look: p.look(w, Instant::now()),
            });
            let timer = self.timer.borrow();
            let shelf = self.shelf.borrow();
            let track = self.track.borrow();
            let media = track.as_ref().map(|t| MediaView {
                track: t,
                // flat unless the listener runs: paused, hidden or switched off draws no movement
                levels: if self.viz.borrow().is_some() { viz::levels() } else { [0.0; viz::BANDS] },
                bars_on: self.cfg().visualizer,
            });
            let (devs, outputs, devcard) = (self.devs.borrow(), self.outputs.borrow(), self.devcard.borrow());
            let (avail, pairs, rows) = (self.avail.borrow(), self.pairs.borrow(), self.tab_rows());
            let card = devcard.as_ref().map(|c| CardView { card: &c.card, age: c.since.elapsed().as_secs_f32(), spark: c.spark });
            let (timer_input, timer_label) = (self.timer_input.borrow(), self.timer_label.borrow());
            let (tnow, parsed) = (Instant::now(), timer::parse(&timer_input));
            let ends = shell::clock_after(if timer.phase() == Phase::Idle { parsed.unwrap_or(0) } else { timer.left_secs(tnow) });
            let view = View {
                w,
                h,
                tab: self.tab.get(),
                hover: self.hover.get(),
                sel: self.sel.get(),
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
                peek: peek_view,
                now: now_secs(),
                timer: TimerView {
                    phase: timer.phase(),
                    left: timer.left_secs(tnow),
                    frac: timer.fraction(tnow),
                    label: &timer.label,
                    ends: &ends,
                    input: &timer_input,
                    label_input: &timer_label,
                    parsed,
                    focus: self.timer_focus.get(),
                    capsule: self.capsule(),
                    peek_slot: self.peek_slot.get(),
                },
                shelf: ShelfView { items: shelf.items(), drag_over: self.drag_over.get(), toast: self.shelf_toast.get() },
                media,
                devices: DevicesView {
                    shape: rows.shape(),
                    outputs: &outputs,
                    conn: &devs,
                    avail: &avail,
                    near: &pairs.nearby,
                    code: pairs.code.as_deref(),
                    searching: pairs.searching,
                    clock: clock(),
                },
                card,
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
        let (use_gpu, pal, scale) = {
            let s = self.cfg();
            (s.use_gpu, palette_for(&s), s.text_size.scale())
        };
        if let Ok(mut r) = Renderer::new(
            self.hwnd,
            window::px(WIN_W, dpi) as u32,
            window::px(WIN_H, dpi) as u32,
            dpi as f32,
            use_gpu,
        ) {
            r.set_look(pal, scale);
            *self.renderer.borrow_mut() = Some(r);
            self.media.send(Cmd::ResendArt); // the new device has not got the cover
            self.invalidate();
        }
    }

    // --------------------------------------------------- tray settings (live)

    /// Saves `settings.json` after a change made from the tray menu.
    fn save_settings(&self) {
        let path = data_dir().join("settings.json");
        if let Err(e) = self.cfg().save(&path) {
            crate::log(&format!("could not save settings: {e}"));
        }
    }

    /// Pushes the theme, accent and text size to the renderer and the notepad editors and
    /// redraws. Cheap enough to call on every change (and when Windows switches light/dark).
    fn apply_look(&self) {
        let (pal, scale) = {
            let s = self.cfg();
            (palette_for(&s), s.text_size.scale())
        };
        if let Some(r) = self.renderer.borrow_mut().as_mut() {
            r.set_look(pal, scale);
        }
        self.notes.set_style(NoteStyle::new(&pal, scale));
        self.invalidate();
    }

    /// The display the pill is on changed (menu choice, or displays added/removed).
    fn move_to_monitor(&self) {
        crate::log(&format!("monitors: {:?}", window::monitors().iter().map(|m| (&m.name, m.primary)).collect::<Vec<_>>()));
        self.reposition();
    }

    /// A choice made in the tray menu's Theme / Accent / Text size / Monitor submenus.
    fn on_menu_choice(&self, id: u32) {
        let pick = |base: u32, n: usize| (id >= base && ((id - base) as usize) < n).then(|| (id - base) as usize);
        if let Some(i) = pick(shell::ID_THEME, Theme::ALL.len()) {
            self.settings.borrow_mut().theme = Theme::ALL[i];
            self.save_settings();
            self.apply_look();
        } else if let Some(i) = pick(shell::ID_ACCENT, Accent::ALL.len()) {
            self.settings.borrow_mut().accent = Accent::ALL[i];
            self.save_settings();
            self.apply_look();
        } else if let Some(i) = pick(shell::ID_TEXT, TextSize::ALL.len()) {
            self.settings.borrow_mut().text_size = TextSize::ALL[i];
            self.save_settings();
            self.apply_look();
        } else if id >= shell::ID_MONITOR {
            let list = window::monitors();
            let Some(m) = list.get((id - shell::ID_MONITOR) as usize) else { return };
            // choosing the primary display stores "follow the primary" rather than its name
            self.settings.borrow_mut().monitor = if m.primary { String::new() } else { m.name.clone() };
            self.save_settings();
            self.move_to_monitor();
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
        if self.tab.get() == Tab::Notes {
            // build the editor before the first frame (only the first time ever), not mid-animation
            self.notes.prepare(self.hwnd);
        }
        self.peek_clear(); // the springs carry on from wherever the capsule was
        self.hover_peek.set(false);
        self.shelf_toast_clear();
        self.track_toast_clear();
        self.device_clear();
        if self.tab.get() == Tab::Shelf {
            self.shelf_refresh();
        }
        if self.tab.get() == Tab::Devices {
            self.refresh_outputs(); // the default may have changed since the tab was last open
        }
        self.open.set(true);
        self.relayout_tabs(true); // show the active note's tab even if the row is scrolled elsewhere
        self.set_targets(EXPANDED_W, EXPANDED_H);
        self.sync_viz();
        self.media_clock();
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
        if self.tab.get() == Tab::Devices {
            self.leave_devices();
        }
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
            self.hover_peek.set(false);
            self.to_rest(); // the plain pill, or the "Time's up" capsule if the time ran out
            self.sync_viz();
            self.media_clock();
            self.spin_clock();

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
            SetTimer(Some(self.hwnd), T_COLLAPSE, self.cfg().collapse_delay_ms.max(10), None);
        }
    }

    fn set_tab(&self, t: Tab) {
        if self.tab.get() == t {
            return;
        }
        self.finish_rename(false);
        if self.tab.get() == Tab::Devices {
            self.leave_devices();
        }
        self.tab.set(t);
        self.scroll.set(0.0);
        self.hover.set(Hit::None);
        self.sel.set(None);
        if t == Tab::Shelf {
            self.shelf_refresh();
        }
        if t == Tab::Devices {
            self.refresh_outputs();
        }
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
        self.sync_viz();
        self.media_clock();
        self.spin_clock();
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
        if !self.open.get() || !self.sticky.get() || self.keep_open.get() || self.picking.get() {
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
        let rows = self.tab_rows();
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
                timer: self.timer.borrow().phase(),
                capsule: self.capsule(),
                shelf_n: self.shelf.borrow().len(),
                media: self.track.borrow().is_some(),
                seekable: self.track.borrow().as_ref().is_some_and(|t| t.seekable),
                dev: rows.shape(),
                card_buttons: self.devcard.borrow().as_ref().is_some_and(|c| c.card.has_buttons() && !c.sparking),
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
            let capsule = self.capsule();
            if capsule != Capsule::None {
                // a capsule is up: light the button under the pointer (it needs no dwell to open)
                let hit = self.hit_at(x, y);
                if hit != self.hover.get() {
                    self.hover.set(hit);
                    self.invalidate();
                }
            }
            if capsule != Capsule::None || self.hover_lock.get() || self.dwell_pending.get() || self.is_hidden() {
                return;
            }
            let d = self.cfg().hover_dwell_ms;
            if d == 0 {
                self.on_dwell();
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
                self.sel.set(Some(k));
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
            self.sel.set(None);
            self.invalidate();
        }
        if self.open.get() && self.auto_close_ok() {
            self.arm_collapse();
        }
        if self.hover_peek.replace(false) {
            self.to_rest(); // the hover capsule goes back to the plain pill
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
            match self.hit_at(x, y) {
                // the time's-up capsule opens the Timer tab; the plain pill and the hover capsule, the panel
                Hit::Pill if self.capsule() == Capsule::TimesUp => self.open_sticky(Some(Tab::Timer)),
                Hit::Pill if self.capsule() == Capsule::Added => self.open_sticky(Some(Tab::Shelf)),
                Hit::Pill if self.capsule() == Capsule::Track => self.open_sticky(Some(Tab::Media)),
                Hit::Pill if self.capsule() == Capsule::Device => self.open_sticky(Some(Tab::Devices)),
                Hit::DeviceUse => self.on_device_use(),
                Hit::DeviceClose => self.device_clear(),
                Hit::Pill => self.open_sticky(None),
                Hit::PeekNotes => self.open_sticky(Some(Tab::Notes)),
                Hit::PeekClip => self.open_sticky(Some(Tab::Clipboard)),
                Hit::PeekShelf => self.open_sticky(Some(Tab::Shelf)),
                Hit::PeekTime => self.open_sticky(Some(Tab::Timer)),
                Hit::PeekArt => self.open_sticky(Some(Tab::Media)),
                Hit::PeekPlay => self.on_media_click(Hit::PeekPlay, x),
                Hit::PeekRing => self.timer_pause_toggle(),
                h @ (Hit::TimerDone | Hit::TimerAdd5) => self.on_timer_click(h),
                _ => {}
            }
            return;
        }
        match self.hit_at(x, y) {
            Hit::TabNotes => self.set_tab(Tab::Notes),
            Hit::TabClip => self.set_tab(Tab::Clipboard),
            Hit::TabTimer => self.set_tab(Tab::Timer),
            Hit::TabShelf => self.set_tab(Tab::Shelf),
            Hit::TabMedia => self.set_tab(Tab::Media),
            Hit::TabDevices => self.set_tab(Tab::Devices),
            Hit::OutputRow(i) => self.on_output_click(i),
            Hit::BtSettings => self.open_bt_settings(),
            Hit::DeviceDisconnect(i) => self.on_link_click(i, false),
            Hit::DeviceConnect(i) => self.on_link_click(i, true),
            Hit::AddOpen => self.open_add(),
            Hit::BtOn => devices::turn_on(),
            Hit::AddBack => self.close_add(),
            Hit::AddStop => self.on_add_stop(),
            Hit::AddPair(i) => self.on_add_pair(i),
            Hit::AddCancel => self.on_add_cancel(),
            h @ (Hit::MediaPlay | Hit::MediaPrev | Hit::MediaNext | Hit::MediaSeek | Hit::MediaStyle) => self.on_media_click(h, x),
            h @ (Hit::ShelfTile(_) | Hit::ShelfSlot | Hit::ShelfClear) => self.on_shelf_click(h, x, y),
            h @ (Hit::TimerField
            | Hit::TimerLabel
            | Hit::TimerPreset(_)
            | Hit::TimerStart
            | Hit::TimerStop
            | Hit::TimerPause
            | Hit::TimerPlus
            | Hit::TimerDone
            | Hit::TimerAdd5) => self.on_timer_click(h),
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
        if self.expanded() && self.tab.get() == Tab::Shelf {
            let max = shelf_view::max_scroll(self.shelf.borrow().len());
            self.scroll.set((self.scroll.get() - delta as f32 / 120.0 * 60.0).clamp(0.0, max));
            let (x, y) = self.mouse.get();
            self.hover.set(self.hit_at(x, y));
            self.invalidate();
            return;
        }
        if self.expanded() && self.tab.get() == Tab::Devices {
            let max = devices_tab::max_scroll(&self.tab_rows().shape());
            self.scroll.set((self.scroll.get() - delta as f32 / 120.0 * 60.0).clamp(0.0, max));
            let (x, y) = self.mouse.get();
            self.hover.set(self.hit_at(x, y));
            self.invalidate();
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
        }
        self.invalidate();
    }

    /// The list changed under a stationary pointer (a row was deleted, a new entry arrived):
    /// hit-test again at the last mouse position, so the hovered row and its pin/x buttons appear
    /// at once instead of waiting for the next mouse move. With the pointer outside the pill,
    /// nothing is hovered.
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
        self.hover.set(hit);
        if let Hit::Row(k) | Hit::RowPin(k) | Hit::RowDel(k) = hit {
            self.sel.set(Some(k));
        }
        if crate::log_enabled() {
            crate::log(&format!("refresh_hover at ({x:.0},{y:.0}) -> {:?} sel={:?}", self.hover.get(), self.sel.get()));
        }
    }

    /// The search text changed: rebuild the list from the top. Whatever is under the pointer gets
    /// highlighted again.
    fn query_changed(&self) {
        self.invalidate_list();
        self.scroll.set(0.0);
        self.hover.set(Hit::None);
        self.sel.set(None);
        self.refresh_hover();
        self.invalidate();
    }

    /// `WM_CHAR`: typing edits the clipboard search box, or a box of the timer's form.
    fn on_char(&self, c: u16) {
        if !self.expanded() || self.renaming.get().is_some() {
            return;
        }
        match self.tab.get() {
            Tab::Clipboard => {
                if type_into(&self.high_surrogate, &mut self.query.borrow_mut(), c, 200) {
                    self.query_changed();
                }
            }
            Tab::Timer if self.timer.borrow().phase() == Phase::Idle => {
                let (text, max) = match self.timer_focus.get() {
                    TimerField::Time => (&self.timer_input, 12),
                    TimerField::Label => (&self.timer_label, 30),
                };
                if type_into(&self.high_surrogate, &mut text.borrow_mut(), c, max) {
                    self.invalidate();
                }
            }
            _ => {}
        }
    }

    /// Keys the pill itself handles: Esc, Tab, Enter on the timer's form and Del on the shelf. (The clipboard list
    /// is driven with the mouse or trackpad; typing goes to a text box through `on_char`.)
    fn on_key(&self, vk: u32) {
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
                self.set_tab(self.tab.get().next());
            }
            VK_DELETE if self.expanded() && self.tab.get() == Tab::Shelf => {
                self.shelf.borrow_mut().remove_selected();
                self.save_shelf();
                self.clamp_shelf_scroll();
                self.refresh_hover();
                self.invalidate();
            }
            VK_RETURN if self.expanded() && self.tab.get() == Tab::Timer && self.timer.borrow().phase() == Phase::Idle => {
                self.start_timer();
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
        let paste = self.cfg().paste_on_click && !shift;
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
        let items: &[(u32, &str)] =
            if self.notes.quick() == Some(i) { &[(1, "Rename")] } else { &[(1, "Rename"), (2, "Delete")] };
        match shell::popup_menu(self.hwnd, items) {
            1 => self.begin_rename(i),
            2 => self.delete_note(i),
            _ => {}
        }
    }

    fn delete_note(&self, i: usize) {
        if self.notes.quick() == Some(i) {
            return; // the Quick note is permanent (Ctrl+W, middle-click and the menu land here too)
        }
        // The rename box is a window of its own, tied to a tab number. Deleting a note moves the
        // tabs after it, so end the rename (keeping what was typed) while the number is still right.
        self.finish_rename(true);
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
            Hit::Search | Hit::TimerField | Hit::TimerLabel => IDC_IBEAM,
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
                    self.on_dwell();
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
            T_SAVE => {
                self.notes.flush();
            }
            T_PEEK => self.peek_hold_over(),
            T_TIMER => self.timer_tick(),
            T_SHELF => self.shelf_toast_clear(),
            T_TRACK => self.track_toast_clear(),
            T_DEVICE => self.device_stage(),
            T_GONE => self.gone_resolved(),
            T_SPIN => {
                self.invalidate();
                self.spin_clock();
            }
            T_MEDIA => {
                self.media_clock();
                self.invalidate();
            }
            T_FS => self.update_fullscreen(),
            T_PASTE => {
                crate::log(&format!("send Ctrl+V; foreground={:?}", unsafe { GetForegroundWindow() }));
                clipboard::send_ctrl_v()
            }
            _ => {}
        }
    }

    fn update_fullscreen(&self) {
        let fs = shell::fullscreen_app_active(self.hwnd);
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
            self.peek_clear();
            self.device_clear();
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
            self.to_rest(); // a time's-up capsule that came due while hidden is waiting
            self.invalidate();
        }
        self.sync_viz(); // nothing listens while the pill is out of sight
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
                // the display in use (named, else the primary one) is the checked one
                let monitors = window::monitors();
                let want = self.cfg().monitor.clone();
                let current = window::pick_monitor(&monitors, &want).map(|m| m.name.clone());
                let items: Vec<(String, bool)> = monitors
                    .iter()
                    .map(|m| {
                        let mut label = format!("{} ({} \u{00D7} {})", window::monitor_label(m), m.width, m.height);
                        if m.primary {
                            label.push_str(", primary");
                        }
                        (label, current.as_deref() == Some(m.name.as_str()))
                    })
                    .collect();
                let (theme, accent, text_size) = {
                    let s = self.cfg();
                    (s.theme, s.accent, s.text_size)
                };
                let choice = shell::tray_menu(
                    self.hwnd,
                    &shell::TrayMenu {
                        paused: self.paused.get(),
                        autostart: shell::autostart_enabled(),
                        pill_hidden: self.user_hidden.get(),
                        theme,
                        accent,
                        text_size,
                        monitors: &items,
                        dpi: self.dpi.get(),
                    },
                );
                match choice {
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
                    0 => {}
                    id => self.on_menu_choice(id),
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
        let want = self.cfg().monitor.clone();
        self.origin.set(window::place(self.hwnd, dpi, &want));
        let (w, h) = self.size();
        window::set_region(self.hwnd, dpi, w, h);
        if self.notes.is_visible() {
            self.show_notes();
        }
        self.invalidate();
    }

    /// Puts the pill (back) on its display: after a display change, or when another one is chosen.
    /// The window is sized for that display's DPI before it moves there.
    fn reposition(&self) {
        let want = self.cfg().monitor.clone();
        let dpi = window::target_dpi(&want).unwrap_or_else(|| window::dpi_for(self.hwnd));
        self.on_dpi_changed(dpi);
    }

    /// Called after the clipboard history changed (new/updated item): enforce limits, persist.
    pub fn after_store_change(&self) {
        {
            let mut s = self.store.borrow_mut();
            let (max_items, max_image_mb) = {
                let c = self.cfg();
                (c.max_items, c.max_image_mb)
            };
            let removed = s.evict(max_items, max_image_mb * 1024 * 1024);
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
        self.save_shelf();
        let _ = self.store.borrow().save();
        unsafe {
            let _ = RemoveClipboardFormatListener(self.hwnd);
        }
        crate::dnd::unregister(self.hwnd);
        shell::unregister_hotkeys(self.hwnd);
        shell::remove_tray(self.hwnd);
        shell::remove_foreground_hook(self.fg_hook.get());
        shell::unwatch_fullscreen(self.hwnd);
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
                WM_LBUTTONDOWN => {
                    let (x, y) = window::mouse_xy(lp);
                    self.on_press(x, y);
                    LRESULT(0)
                }
                WM_MOUSEMOVE => {
                    let (x, y) = window::mouse_xy(lp);
                    if wp.0 & 1 != 0 {
                        self.maybe_start_drag(x, y); // MK_LBUTTON
                    }
                    self.on_mouse_move(x, y);
                    LRESULT(0)
                }
                WM_MOUSELEAVE => {
                    self.on_mouse_leave();
                    LRESULT(0)
                }
                WM_LBUTTONUP => {
                    let (x, y) = window::mouse_xy(lp);
                    self.press.set(None);
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
                    clipboard::on_image_done(self);
                    LRESULT(0)
                }
                WM_APP_THUMB => {
                    self.on_thumbs();
                    LRESULT(0)
                }
                WM_APP_MEDIA => {
                    self.on_media();
                    LRESULT(0)
                }
                WM_APP_DEVICES => {
                    self.on_devices();
                    LRESULT(0)
                }
                WM_APP_VIZ => {
                    viz::redraw_handled();
                    self.invalidate();
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
                WM_APP_COMMAND if lp.0 == crate::cli::CMD_MAGIC => {
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
                    // light/dark mode switched: the tray icon has a variant for each taskbar, and
                    // "Match Windows" follows the app mode
                    if shell::is_theme_change(lp) {
                        shell::refresh_tray_icon(hwnd);
                        if self.cfg().theme == Theme::System {
                            self.apply_look();
                        }
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
                        shell::watch_fullscreen(hwnd);
                        return LRESULT(0);
                    }
                    DefWindowProcW(hwnd, msg, wp, lp)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds UTF-16 units the way `WM_CHAR` delivers them; returns the text and how many changed it.
    fn typed(start: &str, units: &[u16], max: usize) -> (String, usize) {
        let high = Cell::new(0);
        let mut s = start.to_string();
        let n = units.iter().filter(|&&c| type_into(&high, &mut s, c, max)).count();
        (s, n)
    }

    #[test]
    fn typing_adds_characters_and_backspace_removes_one() {
        assert_eq!(typed("", &[b'h' as u16, b'i' as u16], 10), ("hi".into(), 2));
        assert_eq!(typed("abc", &[0x08], 10), ("ab".into(), 1));
        assert_eq!(typed("", &[0x08], 10), ("".into(), 0), "backspace on an empty box changes nothing");
    }

    #[test]
    fn control_keys_type_nothing() {
        // Enter, Esc, Tab, Ctrl+A, Del
        assert_eq!(typed("x", &[0x0D, 0x1B, 0x09, 0x01, 0x7F], 10), ("x".into(), 0));
    }

    #[test]
    fn an_emoji_arrives_in_two_halves_and_becomes_one_character() {
        let units: Vec<u16> = "😀".encode_utf16().collect();
        assert_eq!(typed("", &units, 10), ("😀".into(), 1));
        // a lone second half is not a character
        assert_eq!(typed("", &units[1..], 10), ("".into(), 0));
        // backspace removes the whole emoji, not half of it
        assert_eq!(typed("a😀", &[0x08], 10), ("a".into(), 1));
    }

    #[test]
    fn the_limit_counts_characters_not_bytes() {
        assert_eq!(typed("ab", &[b'c' as u16, b'd' as u16], 3), ("abc".into(), 1));
        let units: Vec<u16> = "é😀".encode_utf16().collect();
        assert_eq!(typed("", &units, 2), ("é😀".into(), 2));
        assert_eq!(typed("é😀", &[b'x' as u16], 2), ("é😀".into(), 0));
    }
}
