//! Shared UI geometry (all values in DIPs, relative to the pill window's top-left).
//!
//! The window is sized to the fully expanded pill plus a small margin for spring overshoot;
//! the visible pill is drawn centred at the top of that area. All expanded-state layout is
//! expressed as offsets from the expanded pill's top-left corner (see `at`).

pub mod clip_list;
pub mod notes_view;
pub mod pill;
pub mod theme;

use crate::store::{ClipItem, Store};
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

/// Footer separator line and the vertical centre of footer content (offsets from pill top).
pub const FOOTER_SEP: f32 = 414.0;
pub const FOOTER_CY: f32 = 434.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Notes,
    Clipboard,
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
pub fn seg_bg() -> RectF {
    at(6.0, 6.0, 216.0, 44.0)
}
pub fn seg_notes() -> RectF {
    at(9.3, 9.3, 100.0, 37.4)
}
pub fn seg_clip() -> RectF {
    at(113.0, 9.3, 106.0, 37.4)
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
/// Region the scrolling list is clipped to.
pub fn list_viewport() -> RectF {
    at(0.0, 94.0, EXPANDED_W, FOOTER_SEP - 1.0 - 94.0)
}
/// Offset of the list's first entry from the top of the pill.
pub const LIST_TOP: f32 = 97.0;
/// Visible height available to list content.
pub const LIST_VIEW_H: f32 = FOOTER_SEP - 3.0 - LIST_TOP;

// ---- notes tab ----
/// Where the native notepad window sits.
pub fn notes_rect() -> RectF {
    at(10.0, 104.0, EXPANDED_W - 20.0, FOOTER_SEP - 4.0 - 104.0)
}

pub struct HitCtx<'a> {
    pub w: f32,
    pub h: f32,
    pub expanded: bool,
    pub tab: Tab,
    pub scroll: f32,
    pub list: &'a ListLayout,
    pub tabs: &'a NoteTabs,
}

pub fn hit_test(x: f32, y: f32, c: &HitCtx) -> Hit {
    if !pill_rect(c.w, c.h).contains(x, y) {
        return Hit::None;
    }
    if !c.expanded {
        return Hit::Pill;
    }
    if seg_notes().contains(x, y) {
        return Hit::TabNotes;
    }
    if seg_clip().contains(x, y) {
        return Hit::TabClip;
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
    }
    Hit::Pill
}

/// Everything the renderer needs for one frame. Built fresh per frame; borrows only.
pub struct View<'a> {
    pub w: f32,
    pub h: f32,
    pub tab: Tab,
    pub hover: Hit,
    /// Highlighted clipboard row (mouse hover or keyboard selection), by list position.
    pub sel: Option<usize>,
    /// True when the highlight came from the keyboard (shows the Enter hint).
    pub kb_mode: bool,
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
}

/// What the renderer needs to draw the copy-peek capsule this frame.
pub struct PeekView<'a> {
    pub content: &'a crate::peek::Content,
    pub look: crate::peek::Look,
}
