//! Shared UI geometry (all values in DIPs, relative to the pill window's top-left).
//!
//! The window is sized to the fully expanded pill plus a small margin for spring overshoot;
//! the visible pill is drawn centred at the top of that area. All expanded-state layout is
//! expressed as offsets from the expanded pill's top-left corner (see `at`).

pub mod clip_list;
pub mod notes_view;
pub mod pill;
pub mod shelf_view;
pub mod theme;
pub mod timer_view;

use crate::store::{ClipItem, Store};
use crate::timer::Phase;
use clip_list::ListLayout;
use notes_view::NoteTabs;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;

pub const WIN_W: f32 = 480.0;
pub const WIN_H: f32 = 488.0;
pub const TOP: f32 = 6.0;

pub const COLLAPSED_W: f32 = 148.0;
pub const COLLAPSED_H: f32 = 34.0;
pub const EXPANDED_W: f32 = 440.0;
pub const EXPANDED_H: f32 = 456.0;
pub const RADIUS_OPEN: f32 = 26.0;

/// The copy-peek capsule (see `peek.rs`): its height and the range of widths it can take.
pub const PEEK_H: f32 = 46.0;
pub const PEEK_MIN_W: f32 = COLLAPSED_W + 24.0;
pub const PEEK_MAX_W: f32 = 340.0;
/// Room the copy-peek capsule adds on its right for the running timer.
pub const PEEK_TIMER_W: f32 = 96.0;

/// The collapsed pill can widen into a capsule for the timer: `Hover` (cursor on it while a timer
/// runs: notepad, pause/resume, time and clipboard buttons) and `TimesUp` (+5 min / Done).
pub const HOVER_W: f32 = 216.0;
/// The same capsule with a shelf button (while the shelf holds files).
pub const HOVER_SHELF_W: f32 = 240.0;
pub const TIMES_UP_W: f32 = 300.0;
/// The shelf's capsules: "Drop to shelf" while a file is dragged over the pill, and the short
/// "Added 3 files" toast after a drop.
pub const DROP_W: f32 = 196.0;
pub const ADDED_W: f32 = 212.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Capsule {
    None,
    Hover,
    TimesUp,
    Drop,
    Added,
}

impl Capsule {
    /// The collapsed pill's shape. An alarm outranks everything; a file being dragged over the pill
    /// outranks the "Added" toast; and the hover capsule needs a timer that is still running
    /// (`hover` stays set after "Done").
    pub fn pick(open: bool, phase: Phase, hover: bool, drag: bool, toast: bool) -> Capsule {
        match phase {
            _ if open => Capsule::None,
            Phase::Finished => Capsule::TimesUp,
            _ if drag => Capsule::Drop,
            _ if toast => Capsule::Added,
            Phase::Running | Phase::Paused if hover => Capsule::Hover,
            _ => Capsule::None,
        }
    }
}

/// Which box of the timer form takes the typing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TimerField {
    Time,
    Label,
}

/// The notepad's footer separator line and the vertical centre of its content (offsets from the
/// pill top). The clipboard tab has no footer: its list runs down to `LIST_BOTTOM`.
pub const FOOTER_SEP: f32 = 414.0;
pub const FOOTER_CY: f32 = 434.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Notes,
    Clipboard,
    Timer,
    Shelf,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Notes, Tab::Clipboard, Tab::Timer, Tab::Shelf];

    /// The tab after this one (the Tab key).
    pub fn next(self) -> Tab {
        Tab::ALL[(self as usize + 1) % Tab::ALL.len()]
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    None,
    Pill,
    TabNotes,
    TabClip,
    /// Header "keep open" pin button.
    BtnPin,
    /// Header collapse chevron.
    BtnCollapse,
    Search,
    NoteTab(usize),
    /// The little x at the right end of a note tab (deletes the note).
    NoteClose(usize),
    NoteAdd,
    /// Clipboard rows are indexed by their position in the (filtered) list.
    Row(usize),
    RowPin(usize),
    RowDel(usize),
    TabTimer,
    /// The "set a time" form: the two boxes, a preset chip, and Start.
    TimerField,
    TimerLabel,
    TimerPreset(usize),
    TimerStart,
    /// While timing: stop (x), pause / resume, and "+1 min".
    TimerStop,
    TimerPause,
    TimerPlus,
    /// After the time ran out (panel or capsule): "Done" and "+5 min".
    TimerDone,
    TimerAdd5,
    /// The hover capsule: notepad and clipboard buttons, pause / resume ring, and the time.
    PeekNotes,
    PeekClip,
    PeekRing,
    PeekTime,
    /// The shelf: its tab, a tile (by position), "Clear", the "+" slot after the last tile (opens
    /// a file picker) and the shelf button of the hover capsule.
    TabShelf,
    ShelfTile(usize),
    ShelfSlot,
    ShelfClear,
    PeekShelf,
}

#[derive(Clone, Copy, Debug)]
pub struct RectF {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
}

impl RectF {
    pub fn w(&self) -> f32 {
        self.r - self.l
    }
    pub fn h(&self) -> f32 {
        self.b - self.t
    }
    pub fn cx(&self) -> f32 {
        (self.l + self.r) / 2.0
    }
    pub fn cy(&self) -> f32 {
        (self.t + self.b) / 2.0
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.l && x < self.r && y >= self.t && y < self.b
    }
    pub fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.l, top: self.t, right: self.r, bottom: self.b }
    }
    pub fn inset(&self, d: f32) -> RectF {
        RectF { l: self.l + d, t: self.t + d, r: self.r - d, b: self.b - d }
    }
}

/// Visible pill rectangle for a given animated size.
pub fn pill_rect(w: f32, h: f32) -> RectF {
    let l = (WIN_W - w) / 2.0;
    RectF { l, t: TOP, r: l + w, b: TOP + h }
}

/// The fully expanded pill.
pub fn exp() -> RectF {
    pill_rect(EXPANDED_W, EXPANDED_H)
}

/// A rectangle given as an offset/size inside the expanded pill.
pub fn at(dx: f32, dy: f32, w: f32, h: f32) -> RectF {
    let p = exp();
    RectF { l: p.l + dx, t: p.t + dy, r: p.l + dx + w, b: p.t + dy + h }
}

// ---- header ----
/// The header's tab segments in tab order (notepad, clipboard, timer, shelf) and the track behind them.
pub struct Header {
    pub bg: RectF,
    pub seg: [RectF; 4],
}

impl Header {
    pub fn of(&self, t: Tab) -> RectF {
        self.seg[t as usize]
    }
}

/// The open tab shows its label and the others just their icon, so four fit. The timer's segment
/// stays wide while it shows a readout (wide enough for "1:24:58" next to the ring).
pub fn header(open: Tab, timer_on: bool) -> Header {
    let labelled = |t: Tab| match t {
        Tab::Notes => 100.0,
        Tab::Clipboard => 106.0,
        Tab::Timer => 96.0,
        Tab::Shelf => 82.0,
    };
    let mut x = 9.3;
    let seg = Tab::ALL.map(|t| {
        let w = if t == open || (t == Tab::Timer && timer_on) { labelled(t) } else { 44.0 };
        let r = at(x, 9.3, w, 37.4);
        x += w + 3.6;
        r
    });
    Header { bg: at(6.0, 6.0, x - 3.6 + 3.3 - 6.0, 44.0), seg }
}
pub fn btn_pin() -> RectF {
    at(360.0, 13.0, 30.0, 30.0)
}
pub fn btn_collapse() -> RectF {
    at(400.0, 13.0, 30.0, 30.0)
}

// ---- clipboard tab ----
pub fn search_rect() -> RectF {
    at(9.3, 53.5, 421.4, 37.4)
}
/// Where the scrolling list is cut off at the bottom (offset from the pill top). Far enough above
/// the pill's rounded bottom corners that a highlighted row never pokes out past them.
pub const LIST_BOTTOM: f32 = EXPANDED_H - 12.0;
/// Region the scrolling list is clipped to.
pub fn list_viewport() -> RectF {
    at(0.0, 94.0, EXPANDED_W, LIST_BOTTOM - 94.0)
}
/// Offset of the list's first entry from the top of the pill.
pub const LIST_TOP: f32 = 97.0;
/// Visible height available to list content.
pub const LIST_VIEW_H: f32 = LIST_BOTTOM - 2.0 - LIST_TOP;

// ---- notes tab ----
/// Where the native notepad window sits.
pub fn notes_rect() -> RectF {
    at(10.0, 104.0, EXPANDED_W - 20.0, FOOTER_SEP - 4.0 - 104.0)
}

/// True once the opening pill, at its animated size `w` x `h`, is big enough to hold the notepad
/// window: all four corners of `notes_rect()` are inside the pill's rounded outline.
///
/// The notepad is a native window on top of the pill, so it cannot grow with it; it is shown the
/// moment this turns true instead of waiting for the spring to settle. The spring looks finished
/// well before it is formally at rest (the last stretch is sub-pixel), and waiting for that left
/// the open panel blank for a third of a second.
pub fn pill_holds_notes(w: f32, h: f32) -> bool {
    /// Keep the window's corners this far inside the outline, so no sliver pokes out.
    const MARGIN: f32 = 2.0;
    let p = pill_rect(w, h);
    let r = pill::radius(h);
    let n = notes_rect();
    // The nearest point of the pill's straight-edged core (the rectangle inset by the radius):
    // a point is inside the rounded outline when it is no farther than `r` from that core.
    let core = p.inset(r);
    [(n.l, n.t), (n.r, n.t), (n.l, n.b), (n.r, n.b)].into_iter().all(|(x, y)| {
        if x < p.l + MARGIN || x > p.r - MARGIN || y < p.t + MARGIN || y > p.b - MARGIN {
            return false;
        }
        let dx = x - x.clamp(core.l, core.r);
        let dy = y - y.clamp(core.t, core.b);
        dx.hypot(dy) <= r - MARGIN
    })
}

pub struct HitCtx<'a> {
    pub w: f32,
    pub h: f32,
    pub expanded: bool,
    pub tab: Tab,
    pub scroll: f32,
    pub list: &'a ListLayout,
    pub tabs: &'a NoteTabs,
    pub timer: Phase,
    pub capsule: Capsule,
    /// Files on the shelf.
    pub shelf_n: usize,
}

pub fn hit_test(x: f32, y: f32, c: &HitCtx) -> Hit {
    let pill = pill_rect(c.w, c.h);
    if !pill.contains(x, y) {
        return Hit::None;
    }
    if !c.expanded {
        return match c.capsule {
            Capsule::None | Capsule::Drop | Capsule::Added => Hit::Pill,
            Capsule::Hover => timer_view::hover_hit(pill, x, y, c.shelf_n > 0),
            Capsule::TimesUp => timer_view::times_up_hit(pill, x, y),
        };
    }
    let tabs = header(c.tab, c.timer != Phase::Idle);
    for (t, hit) in [(Tab::Notes, Hit::TabNotes), (Tab::Clipboard, Hit::TabClip), (Tab::Timer, Hit::TabTimer), (Tab::Shelf, Hit::TabShelf)] {
        if tabs.of(t).contains(x, y) {
            return hit;
        }
    }
    if btn_pin().contains(x, y) {
        return Hit::BtnPin;
    }
    if btn_collapse().contains(x, y) {
        return Hit::BtnCollapse;
    }
    match c.tab {
        Tab::Clipboard => {
            if search_rect().contains(x, y) {
                return Hit::Search;
            }
            if let Some(hit) = clip_list::hit(x, y, c.scroll, c.list) {
                return hit;
            }
        }
        Tab::Notes => {
            if let Some(hit) = c.tabs.hit(x, y) {
                return hit;
            }
        }
        Tab::Timer => {
            if let Some(hit) = timer_view::hit(x, y, c.timer) {
                return hit;
            }
        }
        Tab::Shelf => {
            if let Some(hit) = shelf_view::hit(x, y, c.scroll, c.shelf_n) {
                return hit;
            }
        }
    }
    Hit::Pill
}

/// Everything the renderer needs for one frame. Built fresh per frame; borrows only.
pub struct View<'a> {
    pub w: f32,
    pub h: f32,
    pub tab: Tab,
    pub hover: Hit,
    /// Highlighted clipboard row (the one under the pointer), by list position.
    pub sel: Option<usize>,
    pub scroll: f32,
    pub items: &'a [ClipItem],
    pub store: &'a Store,
    pub list: &'a ListLayout,
    pub query: &'a str,
    /// The pill window has keyboard focus (so the search box shows a caret).
    pub focused: bool,
    pub paused: bool,
    /// "Keep open" pin is on.
    pub keep_open: bool,
    pub note_names: &'a [String],
    pub note_active: usize,
    pub note_tabs: &'a NoteTabs,
    pub renaming: Option<usize>,
    /// The copy peek, while the collapsed pill is showing one.
    pub peek: Option<PeekView<'a>>,
    /// Unix seconds, for "2 min ago" labels.
    pub now: u64,
    pub timer: TimerView<'a>,
    pub shelf: ShelfView<'a>,
}

/// What the shelf's drawing needs this frame.
pub struct ShelfView<'a> {
    pub items: &'a [crate::shelf::Item],
    /// A file drag is over the pill.
    pub drag_over: bool,
    /// The "Added 3 files" toast: (files added, files on the shelf).
    pub toast: Option<(usize, usize)>,
}

/// What the timer's drawing needs this frame.
pub struct TimerView<'a> {
    pub phase: Phase,
    /// Whole seconds left, and the fraction of the run left (the ring).
    pub left: u64,
    pub frac: f32,
    /// The running timer's label, and the wall-clock time it ends ("10:31 pm").
    pub label: &'a str,
    pub ends: &'a str,
    /// The form: what is typed in the two boxes, what the time box means, and which box has the caret.
    pub input: &'a str,
    pub label_input: &'a str,
    pub parsed: Option<u64>,
    pub focus: TimerField,
    pub capsule: Capsule,
    /// Width the copy-peek capsule reserved on its right for the timer (0 = none).
    pub peek_slot: f32,
}

impl TimerView<'_> {
    /// Running or paused: the pill shows the time left.
    pub fn active(&self) -> bool {
        matches!(self.phase, Phase::Running | Phase::Paused)
    }
}

/// What the renderer needs to draw the copy-peek capsule this frame.
pub struct PeekView<'a> {
    pub content: &'a crate::peek::Content,
    /// What the capsule showed before the last copy, while it cross-fades out.
    pub prev: Option<&'a crate::peek::Content>,
    pub look: crate::peek::Look,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::Spring;

    #[test]
    fn the_hover_capsule_needs_a_running_timer_and_a_closed_panel() {
        use Phase::*;
        for (open, phase, hover, want) in [
            (false, Running, true, Capsule::Hover),
            (false, Paused, true, Capsule::Hover),
            (false, Running, false, Capsule::None),
            (false, Finished, true, Capsule::TimesUp), // an alarm outranks the pointer
            (false, Finished, false, Capsule::TimesUp),
            (false, Idle, true, Capsule::None),        // "Done" clicked with the pointer still on the pill
            (true, Running, true, Capsule::None),
            (true, Finished, false, Capsule::None),
        ] {
            assert_eq!(Capsule::pick(open, phase, hover, false, false), want, "open={open} {phase:?} hover={hover}");
        }
    }

    #[test]
    fn a_drag_over_the_pill_outranks_the_toast_and_the_hover_but_not_an_alarm() {
        use Phase::*;
        // (open, phase, hover, drag, toast)
        assert_eq!(Capsule::pick(false, Idle, false, true, false), Capsule::Drop);
        assert_eq!(Capsule::pick(false, Running, true, true, true), Capsule::Drop, "a drag beats the toast and the hover");
        assert_eq!(Capsule::pick(false, Idle, false, false, true), Capsule::Added);
        assert_eq!(Capsule::pick(false, Running, true, false, true), Capsule::Added, "the toast beats the hover");
        assert_eq!(Capsule::pick(false, Finished, false, true, true), Capsule::TimesUp, "an alarm beats everything");
        assert_eq!(Capsule::pick(true, Idle, false, true, true), Capsule::None, "the open panel has no capsule");
    }

    /// Frames (at 60 Hz) after which the opening pill can hold the notepad, and the frame at which
    /// the spring is formally at rest; the pill's animated size is the same on every frame in
    /// between, so this is exactly what the app sees.
    fn open_timeline() -> (Vec<bool>, usize) {
        let (mut w, mut h) = (Spring::new(COLLAPSED_W), Spring::new(COLLAPSED_H));
        w.set_target(EXPANDED_W);
        h.set_target(EXPANDED_H);
        let mut holds = Vec::new();
        let mut settled_at = None;
        for frame in 1..=120 {
            w.step(1.0 / 60.0);
            h.step(1.0 / 60.0);
            holds.push(pill_holds_notes(w.pos, h.pos));
            if settled_at.is_none() && w.settled() && h.settled() {
                settled_at = Some(frame);
            }
        }
        (holds, settled_at.expect("the opening spring settles within two seconds"))
    }

    #[test]
    fn the_notepad_fits_the_open_pill_but_not_the_closed_one() {
        assert!(pill_holds_notes(EXPANDED_W, EXPANDED_H));
        assert!(!pill_holds_notes(COLLAPSED_W, COLLAPSED_H));
        // tall enough but still narrow, and wide enough but still short
        assert!(!pill_holds_notes(COLLAPSED_W, EXPANDED_H));
        assert!(!pill_holds_notes(EXPANDED_W, COLLAPSED_H));
    }

    #[test]
    fn the_notepad_never_pokes_out_through_a_rounded_corner() {
        // the bottom corners of the window sit close to the pill's rounded bottom corners: a pill
        // that only just reaches below the window must not count while the arc still cuts it off
        let n = notes_rect();
        let h = n.b - TOP + 3.0; // 3 DIP of pill below the window's bottom edge
        assert!(!pill_holds_notes(EXPANDED_W, h), "h={h}");
        assert!(pill_holds_notes(EXPANDED_W, EXPANDED_H));
    }

    #[test]
    fn the_notepad_is_shown_well_before_the_spring_settles_and_stays_shown() {
        let (holds, settled_at) = open_timeline();
        let first = holds.iter().position(|&b| b).expect("the pill opens far enough") + 1;
        // a third of a second of blank panel is what this replaces: it has to be a lot earlier
        assert!(first + 12 <= settled_at, "shown at frame {first}, spring at rest at {settled_at}");
        assert!(first <= 12, "shown at frame {first} (~{} ms)", first * 1000 / 60);
        // once the pill can hold it, the overshoot and settling never take that away again
        assert!(holds[first - 1..].iter().all(|&b| b), "the pill dipped below the notepad again");
    }
}
