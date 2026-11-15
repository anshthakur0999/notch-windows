//! The shelf's drawing: the Shelf tab (a grid of file tiles, or the empty drop zone) and the pieces
//! of it shown on the pill: the count on the resting pill, the "Drop to shelf" capsule while a file
//! is dragged over, and the "Added 3 files" toast.
//!
//! Geometry is in DIPs, as offsets from the expanded pill like the other tabs.

use super::timer_view::{self, centred, grown, lit};
use super::*;
use crate::render::{F, Renderer, color, mix};
use crate::shelf::{Item, Kind, MAX_FILES, size_text};

// ---------------------------------------------------------------- tab geometry

/// Tiles per row, and the size of a tile and the gaps between them.
pub const COLS: usize = 3;
const TILE_W: f32 = 130.0;
const TILE_H: f32 = 128.0;
const GAP_X: f32 = 11.5;
const PITCH_Y: f32 = 139.0;
/// Where the first row of tiles starts (below the "5 files" row).
const GRID_TOP: f32 = 100.0;

/// "5 files · 1 missing" at the left of the row under the header.
pub fn summary() -> RectF {
    at(20.0, 56.0, 250.0, 36.0)
}
pub fn clear_btn() -> RectF {
    at(382.0, 56.0, 50.0, 36.0)
}
/// Where the tiles scroll, cut off just above the footer rule.
pub fn viewport() -> RectF {
    at(0.0, 96.0, EXPANDED_W, FOOTER_SEP - 4.0 - 96.0)
}
/// The tile in grid position `i`, scrolled by `scroll`.
pub fn tile(i: usize, scroll: f32) -> RectF {
    let (col, row) = (i % COLS, i / COLS);
    at(14.0 + col as f32 * (TILE_W + GAP_X), GRID_TOP + row as f32 * PITCH_Y - scroll, TILE_W, TILE_H)
}
/// Centre of the x on a tile's top-right corner that removes it from the shelf (shown while the
/// tile is hovered, or always when the file is missing).
pub fn badge(t: RectF) -> (f32, f32) {
    (t.r - 20.0, t.t + 20.0)
}
/// Centre of the tick on a tile's top-left corner, shown while it is selected.
fn tick_badge(t: RectF) -> (f32, f32) {
    (t.l + 20.0, t.t + 20.0)
}
/// Draws the remove x at `t`'s badge.
fn x_badge(r: &Renderer, t: RectF, alpha: f32) {
    let (bx, by) = badge(t);
    r.fill_circle(bx, by, 10.0, color(r.pal.rest, 0.92 * alpha));
    r.glyph('\u{E711}', F::Icon12, bx, by, color(r.pal.text, alpha));
}
/// How far the grid can scroll for `n` files (the last tile is the "drop files here" slot).
pub fn max_scroll(n: usize) -> f32 {
    let tiles = n + usize::from(n < MAX_FILES);
    let bottom = GRID_TOP + tiles.div_ceil(COLS) as f32 * PITCH_Y - (PITCH_Y - TILE_H) + 6.0;
    (bottom - (FOOTER_SEP - 4.0)).max(0.0)
}
/// The empty shelf's drop zone.
pub fn empty_box() -> RectF {
    at(14.0, 58.0, 412.0, 346.0)
}

/// Hit-test inside the Shelf tab with `n` files (`None` = not on a control).
pub fn hit(x: f32, y: f32, scroll: f32, n: usize) -> Option<Hit> {
    if n == 0 {
        return None;
    }
    if clear_btn().contains(x, y) {
        Some(Hit::ShelfClear)
    } else if viewport().contains(x, y) {
        (0..n)
            .find(|&i| tile(i, scroll).contains(x, y))
            .map(Hit::ShelfTile)
            .or_else(|| (n < MAX_FILES && tile(n, scroll).contains(x, y)).then_some(Hit::ShelfSlot))
    } else {
        None
    }
}

// ---------------------------------------------------------------- the Shelf tab

pub fn draw(r: &Renderer, v: &View, alpha: f32) {
    let empty = v.shelf.items.is_empty();
    if empty {
        draw_empty(r, v, alpha);
    } else {
        draw_grid(r, v, alpha);
    }
    let p = exp();
    let cy = p.t + FOOTER_CY;
    pill::hline(r, p.t + FOOTER_SEP, alpha);
    let hint = if empty { "Or drag files onto the pill from anywhere" } else { "Drag a tile out to copy it into any app" };
    r.text(hint, F::Hint, RectF { l: p.l + 20.0, t: cy - 9.0, r: p.l + 300.0, b: cy + 9.0 }.d2d(), color(r.pal.muted, alpha));
    if empty {
        pill::collapse_hint(r, cy, alpha);
    }
}

fn draw_empty(r: &Renderer, v: &View, alpha: f32) {
    let pal = r.pal;
    let over = v.shelf.drag_over;
    let b = empty_box();
    r.fill_rrect(b.d2d(), 22.0, color(mix(pal.body, pal.shelf, if over { 0.10 } else { 0.04 }), alpha));
    r.dashed_rrect(b.inset(0.8).d2d(), 21.2, color(pal.shelf, (if over { 0.85 } else { 0.4 }) * alpha), 1.6);
    let (cx, cy) = (b.cx(), b.t + 92.0);
    r.fill_circle(cx, cy, 32.0, color(mix(pal.body, pal.shelf, 0.2), alpha));
    r.tray(cx, cy, 28.0, color(pal.shelf, alpha));
    r.text("Drop files here", F::Head, RectF { l: b.l, t: cy + 52.0, r: b.r, b: cy + 84.0 }.d2d(), color(pal.text, alpha));
    let lines = ["Hold them while you work, then drag them out", "to any app. The shelf keeps links, so nothing", "is copied."];
    for (k, line) in lines.iter().enumerate() {
        let y = cy + 94.0 + k as f32 * 22.0;
        r.text(line, F::Center, RectF { l: b.l, t: y, r: b.r, b: y + 22.0 }.d2d(), color(pal.muted, alpha));
    }
}

fn draw_grid(r: &Renderer, v: &View, alpha: f32) {
    let pal = r.pal;
    let items = v.shelf.items;
    let (n, missing) = (items.len(), items.iter().filter(|i| i.missing).count());

    let s = summary();
    let count = format!("{n} {}", if n == 1 { "file" } else { "files" });
    let cw = r.measure(F::Seg, &count);
    r.text(&count, F::Seg, RectF { r: s.l + cw + 4.0, ..s }.d2d(), color(pal.text, alpha));
    if missing > 0 {
        r.text(&format!("\u{00B7} {missing} missing"), F::Tab, RectF { l: s.l + cw + 8.0, ..s }.d2d(), color(pal.muted, alpha));
    }

    let c = clear_btn();
    centred(r, "Clear", F::Seg, c, color(if v.hover == Hit::ShelfClear { pal.text } else { pal.muted }, alpha));

    let vp = viewport();
    r.push_clip(vp.d2d());
    for (i, it) in items.iter().enumerate() {
        let t = tile(i, v.scroll);
        if t.b >= vp.t && t.t <= vp.b {
            draw_tile(r, v, it, i, t, alpha);
        }
    }
    if n < MAX_FILES {
        let t = tile(n, v.scroll);
        if t.b >= vp.t && t.t <= vp.b {
            draw_slot(r, v, t, alpha);
        }
    }
    r.pop_clip();
    if v.shelf.drag_over {
        r.dashed_rrect(at(8.0, 94.0, 424.0, 318.0).d2d(), 18.0, color(pal.shelf, 0.6 * alpha), 1.5);
    }
}

fn kind_glyph(k: Kind) -> char {
    match k {
        Kind::Image => '\u{E91B}',
        Kind::Folder => '\u{E8B7}',
        Kind::File => '\u{E7C3}',
    }
}

/// A round badge with a tick in it.
fn tick(r: &Renderer, (cx, cy): (f32, f32), alpha: f32) {
    r.fill_circle(cx, cy, 10.0, color(r.pal.shelf, alpha));
    let ink = color(r.pal.ink, alpha);
    r.line(cx - 4.6, cy + 0.4, cx - 1.4, cy + 3.4, ink, 2.0);
    r.line(cx - 1.4, cy + 3.4, cx + 4.8, cy - 3.2, ink, 2.0);
}

fn draw_tile(r: &Renderer, v: &View, it: &Item, i: usize, t: RectF, alpha: f32) {
    let pal = r.pal;
    let hovered = v.hover == Hit::ShelfTile(i);
    let thumb = RectF { l: t.l + 6.0, t: t.t + 6.0, r: t.r - 6.0, b: t.t + 86.0 };
    let (name, meta) = (RectF { l: t.l + 9.0, t: t.t + 90.0, r: t.r - 8.0, b: t.t + 109.0 }, RectF { l: t.l + 9.0, t: t.t + 107.0, r: t.r - 8.0, b: t.t + 125.0 });

    if it.missing {
        r.fill_rrect(t.d2d(), 14.0, color(lit(r, pal.seg, hovered), alpha));
        r.dashed_rrect(thumb.d2d(), 10.0, color(pal.faint, 0.9 * alpha), 1.3);
        r.glyph('\u{E7BA}', F::Icon24, thumb.cx(), thumb.cy() - 10.0, color(pal.muted, alpha));
        centred(r, "Missing", F::Seg, RectF { t: thumb.cy() + 8.0, b: thumb.cy() + 28.0, ..thumb }, color(pal.muted, alpha));
        r.text(it.name(), F::Seg, name.d2d(), color(pal.muted, alpha));
        r.text("Moved or deleted", F::Hint, meta.d2d(), color(pal.faint, alpha));
        x_badge(r, t, alpha);
        return;
    }

    let sel = it.selected;
    r.fill_rrect(t.d2d(), 14.0, color(if sel { mix(pal.seg, pal.shelf, 0.14) } else { lit(r, pal.seg, hovered) }, alpha));
    r.fill_rrect(thumb.d2d(), 10.0, color(if sel { mix(pal.tile, pal.shelf, 0.12) } else { pal.tile }, alpha));
    match r.shelf_tex.get(&it.path) {
        // a real thumbnail fills the slot; a file icon sits in it at a fixed size
        Some((bmp, false)) => r.fill_rrect_cover(bmp, thumb.d2d(), 10.0, alpha),
        Some((bmp, true)) => {
            let icon = RectF { l: thumb.cx() - 28.0, t: thumb.cy() - 28.0, r: thumb.cx() + 28.0, b: thumb.cy() + 28.0 };
            r.draw_bitmap(bmp, icon.d2d(), alpha);
        }
        None => r.glyph(kind_glyph(it.kind()), F::Icon24, thumb.cx(), thumb.cy(), color(if sel { pal.shelf } else { pal.muted }, alpha)),
    }
    r.text(it.name(), F::Seg, name.d2d(), color(pal.text, alpha));
    let type_label = it.type_label();
    let sub = if it.folder || it.size == 0 { type_label } else { format!("{type_label} \u{00B7} {}", size_text(it.size)) };
    r.text(&sub, F::Hint, meta.d2d(), color(pal.muted, alpha));
    if sel {
        r.stroke_rrect(t.inset(1.0).d2d(), 13.0, color(pal.shelf, alpha), 2.0);
        tick(r, tick_badge(t), alpha);
    }
    if hovered {
        x_badge(r, t, alpha);
    }
}

/// The last tile: a dashed "drop files here" slot.
fn draw_slot(r: &Renderer, v: &View, t: RectF, alpha: f32) {
    let pal = r.pal;
    let a = if v.shelf.drag_over || v.hover == Hit::ShelfSlot { 0.9 } else { 0.5 };
    r.dashed_rrect(t.inset(1.0).d2d(), 14.0, color(pal.shelf, a * alpha), 1.5);
    let (cx, cy) = (t.cx(), t.t + 48.0);
    r.line(cx - 8.0, cy, cx + 8.0, cy, color(pal.shelf, alpha), 2.0);
    r.line(cx, cy - 8.0, cx, cy + 8.0, color(pal.shelf, alpha), 2.0);
    centred(r, "Drop files here", F::Seg, RectF { t: t.t + 66.0, b: t.t + 90.0, ..t }, color(pal.shelf, alpha));
}

// ---------------------------------------------------------------- on the pill

/// The shelf's dot on a crowded resting pill.
const DOT_W: f32 = 8.0;

/// Width of the shelf's readout on the resting pill: the tray, then the count.
fn chip_width(r: &Renderer, n: usize) -> f32 {
    21.0 + r.clock_width(F::ClockSm, &n.to_string())
}

/// The collapsed pill while a timer runs, media plays or the shelf holds files: the timer's ring and
/// time, then the media chip, then the shelf's tray and count, each only while in use. Three at
/// once do not fit as they are, so the shelf shrinks to a dot (the hover capsule has its count).
pub fn draw_resting(r: &Renderer, v: &View, pill: RectF, alpha: f32) {
    let (timer, media, n) = (v.timer.active(), v.media.as_ref(), v.shelf.items.len());
    let dot = timer && media.is_some() && n > 0;
    let tw = if timer { timer_view::chip_width(r, &v.timer) } else { 0.0 };
    let mw = if media.is_some() { media_view::CHIP_W } else { 0.0 };
    let sw = match n {
        0 => 0.0,
        _ if dot => DOT_W,
        _ => chip_width(r, n),
    };
    const GAP: f32 = 22.0; // the divider and the room either side of it
    let parts = [tw, mw, sw].iter().filter(|w| **w > 0.0).count();
    let total = tw + mw + sw + GAP * parts.saturating_sub(1) as f32;
    let mut x = pill.cx() - total / 2.0;
    let cy = pill.cy();
    let divider = |x: &mut f32| {
        r.line(*x + GAP / 2.0, cy - 7.0, *x + GAP / 2.0, cy + 7.0, color(r.pal.wash, 0.16 * alpha), 1.0);
        *x += GAP;
    };
    if timer {
        timer_view::draw_chip(r, &v.timer, x, cy, r.pal.timer, r.pal.text, alpha);
        x += tw;
    }
    if let Some(m) = media {
        if timer {
            divider(&mut x);
        }
        media_view::draw_chip(r, m, x, cy, alpha);
        x += mw;
    }
    if n > 0 {
        if timer || media.is_some() {
            divider(&mut x);
        }
        if dot {
            r.fill_circle(x + DOT_W / 2.0, cy, DOT_W / 2.0, color(r.pal.shelf, alpha));
        } else {
            r.tray(x + 7.0, cy, 14.0, color(r.pal.shelf, alpha));
            r.clock_text(&n.to_string(), F::ClockSm, x + 21.0, cy, color(r.pal.text, alpha));
        }
    }
}

/// "Drop to shelf": the capsule a file dragged over the pill widens it into.
pub fn draw_drop(r: &Renderer, pill: RectF) {
    let pal = r.pal;
    let rad = pill::radius(pill.h());
    let a = grown(pill, DROP_W);
    r.fill_rrect(pill.d2d(), rad, color(mix(pal.rest, pal.shelf, 0.10), a));
    r.dashed_rrect(pill.inset(3.0).d2d(), rad - 3.0, color(pal.shelf, 0.8), 1.5);
    if a < 0.02 {
        return;
    }
    let label = "Drop to shelf";
    let tw = r.measure(F::Seg, label);
    let x = pill.cx() - (16.0 + 9.0 + tw) / 2.0;
    r.tray(x + 8.0, pill.cy(), 16.0, color(pal.shelf, a));
    r.text(label, F::Seg, RectF { l: x + 25.0, t: pill.t, r: x + 29.0 + tw, b: pill.b }.d2d(), color(pal.shelf, a));
}

/// "Added 3 files / 5 on the shelf": the toast after a drop.
pub fn draw_added(r: &Renderer, v: &View, pill: RectF) {
    let Some((added, total)) = v.shelf.toast else { return };
    let a = grown(pill, ADDED_W);
    if a < 0.02 {
        return;
    }
    let pal = r.pal;
    let cy = pill.cy();
    let badge = RectF { l: pill.l + 9.0, t: cy - 15.0, r: pill.l + 39.0, b: cy + 15.0 };
    r.fill_rrect(badge.d2d(), 9.0, color(mix(pal.rest, pal.shelf, 0.26), a));
    r.glyph('\u{E7C3}', F::Icon14, badge.cx(), cy, color(pal.shelf, a));
    let title = format!("Added {added} {}", if added == 1 { "file" } else { "files" });
    let words = |dy: f32| RectF { l: pill.l + 50.0, t: cy + dy, r: pill.r - 12.0, b: cy + dy + 18.0 };
    r.text(&title, F::Seg, words(-19.0).d2d(), color(pal.text, a));
    r.text(&format!("{total} on the shelf"), F::Hint, words(0.0).d2d(), color(pal.muted, a));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_tiles_fit_across_the_panel_and_never_overlap() {
        let p = exp();
        for i in 0..COLS {
            let t = tile(i, 0.0);
            assert!(t.l >= p.l + 8.0 && t.r <= p.r - 8.0, "tile {i} fits");
            if i > 0 {
                assert!(tile(i - 1, 0.0).r < t.l, "tiles {} and {i} do not touch", i - 1);
            }
        }
        assert!(tile(0, 0.0).b < tile(COLS, 0.0).t, "rows do not touch");
        // the row under the header sits above the first tiles
        assert!(summary().b <= tile(0, 0.0).t && clear_btn().r <= p.r - 6.0);
        assert!(summary().r <= clear_btn().l, "the file count has room");
    }

    #[test]
    fn a_click_finds_the_tile_it_is_on_and_nothing_in_the_gaps() {
        let n = 5;
        for i in 0..n {
            let t = tile(i, 0.0);
            assert_eq!(hit(t.cx(), t.cy(), 0.0, n), Some(Hit::ShelfTile(i)));
        }
        let (a, b) = (tile(0, 0.0), tile(1, 0.0));
        assert_eq!(hit((a.r + b.l) / 2.0, a.cy(), 0.0, n), None, "between two tiles");
        // the slot after the last file is the "add files" button, until the shelf is full
        let slot = tile(n, 0.0);
        assert_eq!(hit(slot.cx(), slot.cy(), 0.0, n), Some(Hit::ShelfSlot));
        // scrolled out of view (under the header row) a tile cannot be hit
        let t = tile(0, 60.0);
        assert!(t.t < viewport().t);
        assert_eq!(hit(t.cx(), t.t + 4.0, 60.0, n), None);
        assert_eq!(hit(clear_btn().cx(), clear_btn().cy(), 0.0, n), Some(Hit::ShelfClear));
        assert_eq!(hit(clear_btn().cx(), clear_btn().cy(), 0.0, 0), None, "no buttons on an empty shelf");
    }

    #[test]
    fn the_remove_x_and_the_tick_sit_in_opposite_corners_of_the_tile() {
        let t = tile(0, 0.0);
        let ((xx, xy), (tx, ty)) = (badge(t), tick_badge(t));
        assert!(xx > t.cx() && tx < t.cx() && xy == ty, "x top-right, tick top-left");
        assert!(t.contains(xx, xy) && t.contains(tx, ty));
    }

    #[test]
    fn the_grid_scrolls_just_far_enough_to_reach_the_last_tile() {
        assert_eq!(max_scroll(0), 0.0);
        assert_eq!(max_scroll(5), 0.0, "five files and the slot fill two rows exactly");
        let n = 11;
        let far = max_scroll(n);
        assert!(far > 0.0);
        let last = tile(n, far); // the slot after the 11th file
        assert!(last.b <= viewport().b, "the last row ends inside the viewport");
        assert!(tile(n, far - 1.0).b > last.b, "and not a pixel further than needed");
        assert!(max_scroll(MAX_FILES) > 0.0 && max_scroll(MAX_FILES - 1) > 0.0);
    }
}
