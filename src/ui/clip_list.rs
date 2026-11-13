//! Clipboard tab: search field, PINNED / RECENT sections and typed rows, with a "Capture paused"
//! note in the header while capture is off. It is driven with the mouse or trackpad (click to
//! paste, hover for pin and delete); only typing in the search box uses the keyboard.

use super::*;
use crate::render::{F, Renderer, color};
use crate::store::{Kind, Sub, ago};

pub const ROW_H: f32 = 48.0;
pub const HDR_H: f32 = 21.0;
/// Distances from a row's right edge where the hover buttons start.
const PIN_ZONE: f32 = 64.0;
const DEL_ZONE: f32 = 38.0;

/// Which entries are visible (after search) and where each sits in list space.
#[derive(Default)]
pub struct ListLayout {
    pub headers: Vec<(f32, &'static str)>,
    /// (index into the store's items, top y in list space)
    pub rows: Vec<(usize, f32)>,
    pub height: f32,
}

fn matches(it: &ClipItem, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    match it.kind {
        Kind::Image => "image".contains(q),
        Kind::Text => it.preview.to_lowercase().contains(q) || it.sub.label(it.kind).to_lowercase().contains(q),
    }
}

/// Builds the list for the current search text. `items` is already pinned-first.
pub fn build(items: &[ClipItem], query: &str) -> ListLayout {
    let q = query.trim().to_lowercase();
    let mut l = ListLayout::default();
    let mut y = 0.0;
    for (label, want_pinned) in [("PINNED", true), ("RECENT", false)] {
        let mut first = true;
        for (i, it) in items.iter().enumerate() {
            if it.pinned != want_pinned || !matches(it, &q) {
                continue;
            }
            if first {
                l.headers.push((y, label));
                y += HDR_H;
                first = false;
            }
            l.rows.push((i, y));
            y += ROW_H;
        }
    }
    l.height = y;
    l
}

pub fn max_scroll(l: &ListLayout) -> f32 {
    (l.height - LIST_VIEW_H).max(0.0)
}

/// Window-space rectangle of a row at list-space `y`.
fn row_rect(y: f32, scroll: f32) -> RectF {
    let p = exp();
    let top = p.t + LIST_TOP + y - scroll;
    RectF { l: p.l + 6.0, t: top, r: p.r - 6.0, b: top + ROW_H - 1.0 }
}

/// Scrolls just enough to bring row `k` into view (an entry that was pinned or unpinned moves to
/// the other section); returns the new scroll offset.
pub fn scroll_to_row(l: &ListLayout, k: usize, scroll: f32) -> f32 {
    let Some(&(_, y)) = l.rows.get(k) else { return scroll };
    let mut s = scroll;
    // include the section header above the first row of a section
    let top = if l.headers.iter().any(|(hy, _)| (hy + HDR_H - y).abs() < 0.01) { y - HDR_H } else { y };
    if top < s {
        s = top;
    } else if y + ROW_H > s + LIST_VIEW_H {
        s = y + ROW_H - LIST_VIEW_H;
    }
    s.clamp(0.0, max_scroll(l))
}

/// Hit-test inside the list. `None` means the point is not over a row.
pub fn hit(x: f32, y: f32, scroll: f32, l: &ListLayout) -> Option<Hit> {
    if !list_viewport().contains(x, y) {
        return None;
    }
    for (k, &(_, ry)) in l.rows.iter().enumerate() {
        let rr = row_rect(ry, scroll);
        if rr.contains(x, y) {
            return Some(if x >= rr.r - DEL_ZONE {
                Hit::RowDel(k)
            } else if x >= rr.r - PIN_ZONE {
                Hit::RowPin(k)
            } else {
                Hit::Row(k)
            });
        }
    }
    None
}

pub fn draw(r: &mut Renderer, v: &View, alpha: f32) {
    let p = exp();
    draw_search(r, v, alpha);

    let vp = list_viewport();
    r.push_clip(vp.d2d());
    if v.list.rows.is_empty() {
        let msg = if v.query.trim().is_empty() {
            "Copy something and it shows up here".to_string()
        } else {
            format!("No matches for \u{201C}{}\u{201D}", v.query.trim())
        };
        r.text(&msg, F::Center, vp.d2d(), color(r.pal.faint, alpha));
    }
    for &(y, label) in &v.list.headers {
        let top = p.t + LIST_TOP + y - v.scroll;
        if top + HDR_H < vp.t || top > vp.b {
            continue;
        }
        let rect = RectF { l: p.l + 19.5, t: top, r: p.l + 220.0, b: top + HDR_H };
        r.text_spaced(label, F::Section, rect.d2d(), color(r.pal.subtle, alpha), 1.0);
    }
    for (k, &(idx, y)) in v.list.rows.iter().enumerate() {
        let rect = row_rect(y, v.scroll);
        if rect.b < vp.t || rect.t > vp.b {
            continue;
        }
        draw_row(r, v, k, idx, rect, alpha);
    }
    r.pop_clip();

    if v.paused {
        draw_paused(r, alpha);
    }
}

fn draw_search(r: &Renderer, v: &View, alpha: f32) {
    let pal = r.pal;
    let s = search_rect();
    r.fill_rrect(s.d2d(), s.h() / 2.0, color(pal.field, alpha));
    r.glyph('\u{E721}', F::Icon16, s.l + 22.0, s.cy(), color(pal.muted, alpha));

    let text_l = s.l + 41.0;
    let text = RectF { l: text_l, t: s.t, r: s.r - 16.0, b: s.b };
    if v.query.is_empty() {
        r.text("Search clipboard history", F::Title, text.d2d(), color(pal.subtle, alpha));
    } else {
        r.text(v.query, F::Title, text.d2d(), color(pal.text, alpha));
    }
    if v.focused {
        let w = if v.query.is_empty() { 0.0 } else { r.text_w(F::Title, v.query) };
        let x = (text_l + w + 1.0).min(text.r);
        r.line(x, s.cy() - 8.5, x, s.cy() + 8.5, color(pal.text, alpha * 0.9), 1.2);
    }
}

fn draw_row(r: &mut Renderer, v: &View, k: usize, idx: usize, rect: RectF, alpha: f32) {
    let item = &v.items[idx];
    let selected = v.sel == Some(k);
    let hovered = matches!(v.hover, Hit::Row(j) | Hit::RowPin(j) | Hit::RowDel(j) if j == k);
    let cy = rect.cy();
    let pal = r.pal;

    if selected {
        r.fill_rrect(rect.d2d(), 12.0, color(pal.sel_fill(), alpha));
        r.stroke_rrect(rect.inset(0.6).d2d(), 11.4, color(pal.sel_edge(), alpha), 1.2);
    }

    // ---- leading icon / thumbnail ----
    let text_l;
    match item.kind {
        Kind::Image => {
            let bx = RectF { l: rect.l + 10.0, t: cy - 17.0, r: rect.l + 57.6, b: cy + 17.0 };
            r.fill_rrect(bx.d2d(), 7.0, color(pal.tile, alpha));
            match r.thumb(item.id, &v.store.thumb_path(item.id)) {
                Some(bmp) => {
                    let ar = if item.height > 0 { item.width as f32 / item.height as f32 } else { 1.0 };
                    let inner = bx.inset(1.0);
                    let (dw, dh) = if ar > inner.w() / inner.h() {
                        (inner.w(), inner.w() / ar)
                    } else {
                        (inner.h() * ar, inner.h())
                    };
                    let dest = RectF {
                        l: inner.cx() - dw / 2.0,
                        t: inner.cy() - dh / 2.0,
                        r: inner.cx() + dw / 2.0,
                        b: inner.cy() + dh / 2.0,
                    };
                    r.draw_bitmap(&bmp, dest.d2d(), alpha);
                }
                None => r.glyph('\u{E91B}', F::Icon16, bx.cx(), bx.cy(), color(pal.muted, alpha)),
            }
            text_l = bx.r + 10.3;
        }
        Kind::Text => {
            let c = color(if selected { pal.clip } else { pal.muted }, alpha);
            let cx = rect.l + 18.6;
            match item.sub {
                Sub::Plain => r.glyph('\u{E8E4}', F::Icon14, cx, cy, c),
                Sub::Link => r.glyph('\u{E71B}', F::Icon14, cx, cy, c),
                Sub::Code => {
                    // "< >" brackets (the icon font's code glyph is "{ }")
                    let k = 4.6;
                    r.line(cx - 2.2, cy - k, cx - 6.4, cy, c, 1.4);
                    r.line(cx - 6.4, cy, cx - 2.2, cy + k, c, 1.4);
                    r.line(cx + 2.2, cy - k, cx + 6.4, cy, c, 1.4);
                    r.line(cx + 6.4, cy, cx + 2.2, cy + k, c, 1.4);
                }
            }
            text_l = rect.l + 37.7;
        }
    }

    // ---- title + subtitle ----
    let right_limit = rect.r
        - if hovered {
            PIN_ZONE + 4.0
        } else if item.pinned {
            34.0
        } else {
            14.0
        };
    let (title, font) = match item.kind {
        Kind::Image => ("Image", F::Row),
        Kind::Text => (item.preview.as_str(), if item.sub == Sub::Code { F::RowMono } else { F::Row }),
    };
    // each line gets a box as tall as its text, so the "text size" setting does not clip it
    let line_h = 16.0 * r.text_scale;
    let title_r = RectF { l: text_l, t: cy + 0.5 - line_h, r: right_limit.max(text_l + 10.0), b: cy + 0.5 };
    r.text(title, font, title_r.d2d(), color(pal.text, alpha));

    let label = item.sub.label(item.kind);
    let subtitle = if item.pinned { label.to_string() } else { format!("{label} \u{00B7} {}", ago(v.now, item.ts)) };
    let sub_r = RectF { l: text_l, t: cy - 0.5, r: title_r.r, b: cy - 0.5 + line_h };
    r.text(&subtitle, F::RowSub, sub_r.d2d(), color(pal.subtle, alpha));

    // ---- trailing controls ----
    if hovered {
        let pin_hover = v.hover == Hit::RowPin(k);
        let pin_c = if item.pinned { pal.pin } else if pin_hover { pal.text } else { pal.subtle };
        r.pin(rect.r - 51.0, cy, 14.0, color(pin_c, alpha), item.pinned);
        let del_c = if v.hover == Hit::RowDel(k) { pal.danger } else { pal.subtle };
        r.glyph('\u{E711}', F::Icon12, rect.r - 21.0, cy, color(del_c, alpha));
    } else if item.pinned {
        r.pin(rect.r - 18.0, cy, 14.0, color(pal.pin, alpha), true);
    }
}

/// "Capture paused" in the header's free space, between the tab switch and the pin button (it
/// used to sit in the footer). Amber, like the pin: that colour means "held" here.
fn draw_paused(r: &Renderer, alpha: f32) {
    let label = "Capture paused";
    let w = r.text_w(F::Hint, label);
    let (cy, right) = (seg_bg().cy(), btn_pin().l - 8.0);
    r.text(
        label,
        F::Hint,
        RectF { l: right - w - 4.0, t: cy - 9.0, r: right, b: cy + 9.0 }.d2d(),
        color(r.pal.pin, alpha),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ClipItem;

    fn item(id: u64, text: &str, pinned: bool) -> ClipItem {
        ClipItem {
            id,
            kind: Kind::Text,
            sub: crate::store::classify(text),
            hash: id,
            preview: text.to_string(),
            chars: text.len() as u32,
            width: 0,
            height: 0,
            bytes: 0,
            pinned,
            ts: 0,
        }
    }

    fn image(id: u64) -> ClipItem {
        ClipItem { kind: Kind::Image, preview: String::new(), width: 10, height: 10, ..item(id, "", false) }
    }

    #[test]
    fn sections_group_pinned_first_with_headers() {
        let items = vec![item(1, "pinned one", true), item(2, "recent a", false), item(3, "recent b", false)];
        let l = build(&items, "");
        assert_eq!(l.headers.iter().map(|h| h.1).collect::<Vec<_>>(), vec!["PINNED", "RECENT"]);
        assert_eq!(l.rows.iter().map(|r| r.0).collect::<Vec<_>>(), vec![0, 1, 2]);
        // header, row, header, row, row
        assert_eq!(l.headers[0].0, 0.0);
        assert_eq!(l.rows[0].1, HDR_H);
        assert_eq!(l.headers[1].0, HDR_H + ROW_H);
        assert_eq!(l.rows[1].1, HDR_H + ROW_H + HDR_H);
        assert_eq!(l.height, 2.0 * HDR_H + 3.0 * ROW_H);
    }

    #[test]
    fn no_pinned_means_no_pinned_header() {
        let l = build(&[item(1, "a", false)], "");
        assert_eq!(l.headers.len(), 1);
        assert_eq!(l.headers[0].1, "RECENT");
    }

    #[test]
    fn search_filters_case_insensitively_and_matches_kinds() {
        let items = vec![
            item(1, "Meeting moved to 4 pm", false),
            item(2, "https://example.com/release-notes", false),
            item(3, "SELECT id FROM orders;", false),
            image(4),
        ];
        assert_eq!(build(&items, "MEETING").rows.len(), 1);
        assert_eq!(build(&items, "release").rows[0].0, 1);
        assert_eq!(build(&items, "link").rows[0].0, 1); // matches the type label too
        assert_eq!(build(&items, "code").rows[0].0, 2);
        assert_eq!(build(&items, "imag").rows[0].0, 3);
        assert_eq!(build(&items, "  ").rows.len(), 4); // blank query = everything
        let none = build(&items, "zzz");
        assert!(none.rows.is_empty() && none.headers.is_empty() && none.height == 0.0);
    }

    #[test]
    fn scroll_bounds_and_follow_selection() {
        let items: Vec<ClipItem> = (0..30).map(|i| item(i, &format!("row {i}"), false)).collect();
        let l = build(&items, "");
        assert!(max_scroll(&l) > 0.0);
        assert_eq!(max_scroll(&build(&items[..3], "")), 0.0);
        // first row: scroll back to the very top (header visible)
        assert_eq!(scroll_to_row(&l, 0, 200.0), 0.0);
        // moving far down scrolls just enough to reveal the row
        let s = scroll_to_row(&l, 25, 0.0);
        let (_, y) = l.rows[25];
        assert!(y + ROW_H <= s + LIST_VIEW_H + 0.01 && y >= s);
        // already visible rows do not move the list
        assert_eq!(scroll_to_row(&l, 1, 0.0), 0.0);
    }

    #[test]
    fn hit_zones_split_row_into_body_pin_and_delete() {
        let l = build(&[item(1, "a", false)], "");
        let (_, y) = l.rows[0];
        let rr = row_rect(y, 0.0);
        let cy = rr.cy();
        assert_eq!(hit(rr.l + 30.0, cy, 0.0, &l), Some(Hit::Row(0)));
        assert_eq!(hit(rr.r - 50.0, cy, 0.0, &l), Some(Hit::RowPin(0)));
        assert_eq!(hit(rr.r - 10.0, cy, 0.0, &l), Some(Hit::RowDel(0)));
        assert_eq!(hit(rr.l + 30.0, rr.b + 40.0, 0.0, &l), None);
    }
}
