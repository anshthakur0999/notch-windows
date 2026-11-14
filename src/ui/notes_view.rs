//! Notepad tab chrome: the note tabs row, divider and footer. (The text itself is a native
//! RichEdit window placed over `notes_rect()`.)

use super::*;
use crate::render::{F, Renderer, color};

/// Where the note tabs and the "+" button are (window coordinates, DIPs).
///
/// While the tabs fit, the "+" follows the last one. Once they do not, the row scrolls: the tabs
/// live in `view` (clipped and hit-tested only inside it) and the "+" stays pinned at the right
/// end, so there is always a way to add another note.
pub struct NoteTabs {
    /// Tab rectangles with the scroll already applied (parts of them may lie outside `view`).
    pub tabs: Vec<RectF>,
    pub add: Option<RectF>,
    /// The part of the row the tabs are visible in.
    pub view: RectF,
    /// How far the row is scrolled (DIPs, 0 = first tab at the left) and the furthest it can go.
    pub scroll: f32,
    pub max_scroll: f32,
    /// Tabs carry a delete "x" (there is more than one note to choose from).
    pub closable: bool,
    /// The one tab that has no "x" because it can never be deleted (the Quick note).
    pub fixed: Option<usize>,
}

impl NoteTabs {
    pub fn empty() -> NoteTabs {
        NoteTabs {
            tabs: Vec::new(),
            add: None,
            view: RectF { l: 0.0, t: 0.0, r: 0.0, b: 0.0 },
            scroll: 0.0,
            max_scroll: 0.0,
            closable: false,
            fixed: None,
        }
    }

    /// Tab `i` shows a delete "x".
    pub fn has_x(&self, i: usize) -> bool {
        self.closable && self.fixed != Some(i)
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        if self.view.contains(x, y) {
            if let Some(i) = self.tabs.iter().position(|t| t.contains(x, y)) {
                if self.has_x(i) && x >= self.tabs[i].r - CLOSE_ZONE {
                    return Some(Hit::NoteClose(i));
                }
                return Some(Hit::NoteTab(i));
            }
        }
        match self.add {
            Some(a) if a.contains(x, y) => Some(Hit::NoteAdd),
            _ => None,
        }
    }
}

/// Extra width of a closable tab for its "x" (a gap plus the glyph), and how much of the tab's
/// right edge counts as the "x" for clicking. The glyph is centred `CLOSE_CX` from the edge.
const CLOSE_SLOT: f32 = 16.0;
const CLOSE_ZONE: f32 = 26.0;
const CLOSE_CX: f32 = 15.5;
const TAB_GAP: f32 = 4.0;
const ADD_W: f32 = 34.0;
/// Space between the scrolling tabs and the pinned "+".
const ADD_GAP: f32 = 6.0;
/// How far a revealed tab is kept from the edge of the row, so its neighbours stay in sight.
const REVEAL_PAD: f32 = 28.0;
/// Pure layout of the row from the tab widths: no text measuring, no drawing, so it is testable.
pub fn plan(widths: &[f32], can_add: bool, scroll: f32) -> NoteTabs {
    let p = exp();
    let (top, bot) = (p.t + 54.0, p.t + 88.0);
    let (row_l, row_r) = (p.l + 12.7, p.r - 8.0);
    let content_w = widths.iter().map(|w| w + TAB_GAP).sum::<f32>() - TAB_GAP;
    let content_w = content_w.max(0.0);
    let fits = row_l + content_w + if can_add { TAB_GAP + ADD_W } else { 0.0 } <= row_r;

    let (scroll, max_scroll, view_r, add) = if fits {
        let after = row_l + content_w + TAB_GAP;
        (0.0, 0.0, row_r, can_add.then_some(RectF { l: after, t: top, r: after + ADD_W, b: bot }))
    } else {
        let view_r = if can_add { row_r - ADD_W - ADD_GAP } else { row_r };
        let max_scroll = (content_w - (view_r - row_l)).max(0.0);
        let add = can_add.then_some(RectF { l: row_r - ADD_W, t: top, r: row_r, b: bot });
        (scroll.clamp(0.0, max_scroll), max_scroll, view_r, add)
    };

    let mut x = row_l - scroll;
    let mut tabs = Vec::with_capacity(widths.len());
    for w in widths {
        tabs.push(RectF { l: x, t: top, r: x + w, b: bot });
        x += w + TAB_GAP;
    }
    NoteTabs { tabs, add, view: RectF { l: row_l, t: top, r: view_r, b: bot }, scroll, max_scroll, closable: false, fixed: None }
}

/// The scroll position that brings tab `i` fully into view (with a little margin), or the
/// current one if it already is.
pub fn scroll_to_reveal(t: &NoteTabs, i: usize) -> f32 {
    let Some(r) = t.tabs.get(i) else { return t.scroll };
    let pad = REVEAL_PAD.min(((t.view.w() - r.w()) / 2.0).max(0.0));
    let mut s = t.scroll;
    if r.l < t.view.l + pad {
        s -= t.view.l + pad - r.l;
    } else if r.r > t.view.r - pad {
        s += r.r - (t.view.r - pad);
    }
    s.clamp(0.0, t.max_scroll)
}

/// Horizontal inset of a tab's label from the tab's left edge (DIPs).
const LABEL_INSET: f32 = 10.5;
/// Narrowest the rename box gets (DIPs), so there is room to type into a short name.
pub const RENAME_MIN_W: f32 = 140.0;
const RENAME_BOX_H: f32 = 30.0;
const RENAME_TEXT_H: f32 = 18.0;

/// Width of the tab being renamed: wide enough to type a real name, but narrow enough to be
/// revealed inside the scrolling row. (`layout` gives that tab this width, so the tabs after it
/// shift right to make room instead of ending up underneath the box.)
fn rename_width(natural: f32) -> f32 {
    let p = exp();
    let max = (p.r - 8.0) - (p.l + 12.7) - ADD_W - ADD_GAP - 2.0 * REVEAL_PAD;
    natural.max(RENAME_MIN_W).min(max)
}

/// The rounded box drawn around the rename editor: a pill centred on the label's row, as wide
/// as the tab (which `layout` has already widened for renaming).
pub fn rename_box(tab: &RectF) -> RectF {
    let cy = tab.t + 14.0; // the label's centre line
    RectF { l: tab.l, t: cy - RENAME_BOX_H / 2.0, r: tab.r, b: cy + RENAME_BOX_H / 2.0 }
}

/// The text strip (x, y, w, h in DIPs) for the native editor inside `rename_box`: it starts
/// where the tab's label starts, so the text does not move when editing begins.
pub fn rename_text(b: &RectF, tab: &RectF) -> (f32, f32, f32, f32) {
    let x = tab.l + LABEL_INSET;
    let cy = (b.t + b.b) / 2.0;
    (x, cy - RENAME_TEXT_H / 2.0, b.r - 10.0 - x, RENAME_TEXT_H)
}

/// Lays out the tab row for the given note names (widths come from real text measurement).
/// The tab being renamed, if any, is widened so there is room to type; `scroll` is how far the
/// row is scrolled (it is clamped, and the result says what it ended up as). `fixed` is the tab
/// that has no "x" (the Quick note).
pub fn layout(r: &Renderer, names: &[String], renaming: Option<usize>, fixed: Option<usize>, scroll: f32) -> NoteTabs {
    let closable = names.len() > 1; // with a single note there is nothing to close
    let widths: Vec<f32> = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let slot = if closable && fixed != Some(i) { CLOSE_SLOT } else { 0.0 };
            let w = r.text_w(F::TabOn, n).min(110.0) + 21.0 + slot;
            if renaming == Some(i) { rename_width(w) } else { w }
        })
        .collect();
    let mut t = plan(&widths, names.len() < crate::notes::MAX_NOTES, scroll);
    t.closable = closable;
    t.fixed = fixed;
    t
}

pub fn draw(r: &Renderer, v: &View, alpha: f32) {
    let p = exp();
    let pal = r.pal;
    pill::hline(r, p.t + 88.0, alpha);

    // The tabs scroll: everything about them is clipped to the row's viewport. The clip is a few
    // DIPs taller (and a touch wider) than the hit area so the rename box, which stands slightly
    // proud of the row, keeps its full outline instead of being cut off at the top.
    let v_ = v.note_tabs.view;
    r.push_clip(RectF { l: v_.l - 2.0, t: v_.t - 4.0, r: v_.r + 2.0, b: v_.b }.d2d());
    for (i, rect) in v.note_tabs.tabs.iter().enumerate() {
        if rect.r < v.note_tabs.view.l || rect.l > v.note_tabs.view.r {
            continue; // entirely off-screen
        }
        let active = i == v.note_active;
        let on_close = v.hover == Hit::NoteClose(i);
        let hover = on_close || v.hover == Hit::NoteTab(i);
        let has_x = v.note_tabs.has_x(i);
        let slot = if has_x { CLOSE_SLOT } else { 0.0 };
        if v.renaming != Some(i) {
            let text = RectF { l: rect.l + LABEL_INSET, t: rect.t, r: rect.r - LABEL_INSET - slot, b: p.t + 82.0 };
            let c = if active {
                pal.text
            } else if hover {
                pal.text2
            } else {
                pal.muted
            };
            let name = v.note_names.get(i).map(String::as_str).unwrap_or("");
            r.text(name, if active { F::TabOn } else { F::Tab }, text.d2d(), color(c, alpha));
            // the delete "x": on the active tab and whichever tab the pointer is on
            if has_x && (active || hover) {
                let xc = if on_close {
                    pal.danger
                } else if active {
                    pal.subtle
                } else {
                    pal.faint
                };
                r.glyph('\u{E711}', F::Icon12, rect.r - CLOSE_CX, p.t + 68.0, color(xc, alpha));
            }
        }
        if active {
            let u = RectF { l: rect.l, t: p.t + 86.6, r: rect.r, b: p.t + 88.6 };
            r.fill_rrect(u.d2d(), 1.0, color(pal.notes, alpha));
        }
    }
    // The rename box goes last so it sits over neighbouring tabs; the native editor (same fill
    // colour as this box) is placed on top of it by `App::begin_rename`.
    if let Some(tab) = v.renaming.and_then(|i| v.note_tabs.tabs.get(i)) {
        let b = rename_box(tab);
        let radius = b.h() / 2.0;
        r.fill_rrect(b.d2d(), radius, color(pal.tile, alpha));
        r.stroke_rrect(b.d2d(), radius, color(pal.notes, alpha * 0.55), 1.0);
    }
    // Soft edges where tabs run on past the viewport (a stepped fade into the pill's colour).
    let nt = v.note_tabs;
    const STEPS: usize = 8;
    const STEP_W: f32 = 3.0;
    for k in 0..STEPS {
        let a = 0.95 * (1.0 - k as f32 / STEPS as f32);
        let x = k as f32 * STEP_W;
        if nt.scroll > 0.5 {
            let s = RectF { l: nt.view.l + x, t: nt.view.t, r: nt.view.l + x + STEP_W, b: nt.view.b };
            r.fill_rect(s.d2d(), color(pal.body, alpha * a));
        }
        if nt.scroll < nt.max_scroll - 0.5 {
            let s = RectF { l: nt.view.r - x - STEP_W, t: nt.view.t, r: nt.view.r - x, b: nt.view.b };
            r.fill_rect(s.d2d(), color(pal.body, alpha * a));
        }
    }
    r.pop_clip();

    if let Some(a) = v.note_tabs.add {
        let c = if v.hover == Hit::NoteAdd { pal.text } else { pal.muted };
        r.glyph('\u{E710}', F::Icon14, a.cx(), p.t + 68.0, color(c, alpha));
    }

    // ---- footer ----
    pill::hline(r, p.t + FOOTER_SEP, alpha);
    let cy = p.t + FOOTER_CY;
    r.glyph('\u{E73E}', F::Icon14, p.l + 26.0, cy, color(pal.notes, alpha));
    r.text(
        "Saved automatically",
        F::Hint,
        RectF { l: p.l + 40.0, t: cy - 9.0, r: p.l + 220.0, b: cy + 9.0 }.d2d(),
        color(pal.muted, alpha),
    );

    let label = "to collapse";
    let lw = r.text_w(F::Hint, label);
    let label_l = p.r - 20.0 - lw;
    r.text(
        label,
        F::Hint,
        RectF { l: label_l, t: cy - 9.0, r: label_l + lw + 4.0, b: cy + 9.0 }.d2d(),
        color(pal.muted, alpha),
    );
    let key_w = pill::keycap_width(r, "Esc");
    pill::keycap(r, "Esc", label_l - 7.0 - key_w, cy, alpha);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_edges() -> (f32, f32) {
        let p = exp();
        (p.l + 12.7, p.r - 8.0)
    }

    #[test]
    fn few_tabs_keep_the_plus_right_after_the_last_tab() {
        let t = plan(&[90.0, 70.0, 70.0], true, 0.0);
        let add = t.add.expect("plus");
        assert_eq!(t.max_scroll, 0.0);
        assert!((add.l - (t.tabs[2].r + TAB_GAP)).abs() < 0.01);
    }

    #[test]
    fn six_typical_tabs_still_offer_the_plus() {
        // this is the row that used to lose its "+" button
        let t = plan(&[97.0, 66.0, 66.0, 66.0, 66.0, 66.0], true, 0.0);
        assert!(t.add.is_some());
    }

    #[test]
    fn many_tabs_pin_the_plus_at_the_right_and_scroll() {
        let (row_l, row_r) = row_edges();
        let t = plan(&[96.0; 12], true, 0.0);
        let add = t.add.expect("plus must stay available");
        assert!((add.r - row_r).abs() < 0.01);
        assert!(t.view.r <= add.l - ADD_GAP + 0.01);
        assert!(t.max_scroll > 0.0);
        assert!((t.tabs[0].l - row_l).abs() < 0.01);
    }

    #[test]
    fn scroll_is_clamped_and_shifts_the_tabs() {
        let t0 = plan(&[96.0; 12], true, 0.0);
        let t = plan(&[96.0; 12], true, 1.0e6);
        assert_eq!(t.scroll, t.max_scroll);
        assert!((t.tabs[0].l - (t0.tabs[0].l - t.max_scroll)).abs() < 0.01);
        assert!((t.tabs[11].r - t.view.r).abs() < 0.01, "last tab should end at the viewport edge");
        assert_eq!(plan(&[96.0; 12], true, -50.0).scroll, 0.0);
    }

    #[test]
    fn reveal_brings_a_hidden_tab_into_view() {
        let t = plan(&[96.0; 12], true, 0.0);
        let s = scroll_to_reveal(&t, 11);
        let after = plan(&[96.0; 12], true, s);
        assert!(after.tabs[11].r <= after.view.r + 0.01 && after.tabs[11].l >= after.view.l);
        assert_eq!(scroll_to_reveal(&t, 0), 0.0, "already visible: unchanged");
        assert_eq!(scroll_to_reveal(&after, 0), 0.0, "scrolls back to the start");
        assert!((scroll_to_reveal(&after, 11) - after.scroll).abs() < 0.01);
    }

    #[test]
    fn clipped_tabs_are_not_clickable_but_the_plus_is() {
        let t = plan(&[96.0; 12], true, 0.0);
        let hidden = t.tabs[11];
        assert!(hidden.l > t.view.r);
        assert_eq!(t.hit(hidden.l + 5.0, hidden.t + 10.0), None);
        let first = t.tabs[0];
        assert_eq!(t.hit(first.l + 5.0, first.t + 10.0), Some(Hit::NoteTab(0)));
        let add = t.add.unwrap();
        assert_eq!(t.hit(add.cx(), add.cy()), Some(Hit::NoteAdd));
    }

    #[test]
    fn no_plus_at_the_note_limit_and_no_tabs_is_fine() {
        let (_, row_r) = row_edges();
        let t = plan(&[96.0; 12], false, 0.0);
        assert!(t.add.is_none());
        assert!((t.view.r - row_r).abs() < 0.01);
        let empty = plan(&[], true, 0.0);
        assert!(empty.tabs.is_empty() && empty.add.is_some());
    }

    #[test]
    fn close_zone_is_a_tab_s_right_edge_and_only_when_closable() {
        let mut t = plan(&[96.0; 3], true, 0.0);
        let tab = t.tabs[1];
        assert_eq!(t.hit(tab.r - 5.0, tab.t + 10.0), Some(Hit::NoteTab(1)), "single note: no x");
        t.closable = true;
        assert_eq!(t.hit(tab.r - 5.0, tab.t + 10.0), Some(Hit::NoteClose(1)));
        assert_eq!(t.hit(tab.l + 20.0, tab.t + 10.0), Some(Hit::NoteTab(1)));
    }

    #[test]
    fn the_fixed_tab_has_no_x_to_click() {
        let mut t = plan(&[96.0; 3], true, 0.0);
        t.closable = true;
        t.fixed = Some(0); // the Quick note
        let (quick, other) = (t.tabs[0], t.tabs[1]);
        assert!(!t.has_x(0) && t.has_x(1));
        assert_eq!(t.hit(quick.r - 5.0, quick.t + 10.0), Some(Hit::NoteTab(0)), "no x on the Quick note");
        assert_eq!(t.hit(other.r - 5.0, other.t + 10.0), Some(Hit::NoteClose(1)));
    }
}
