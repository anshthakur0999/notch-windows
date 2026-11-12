//! Draws the pill itself: body, collapsed indicator, the segmented header, and shared bits
//! (hairlines, key caps) used by both tabs.

use super::*;
use crate::render::{F, Renderer, color, mix};
use windows::core::Result;

pub const BODY: u32 = 0x0B0D11;
pub const AMBER: u32 = 0xE9B44C;
pub const PIN_AMBER: u32 = 0xF2B84B;
pub const SKY: u32 = 0x4FC3F7;
pub const MUTED: u32 = 0x9BA1AB;

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Fraction of the way from collapsed to expanded, based on the animated height.
pub fn progress(h: f32) -> f32 {
    ((h - COLLAPSED_H) / (EXPANDED_H - COLLAPSED_H)).clamp(0.0, 1.0)
}

/// Corner radius for the current animated size.
pub fn radius(h: f32) -> f32 {
    lerp(COLLAPSED_H / 2.0, RADIUS_OPEN, progress(h))
}

/// 1px horizontal rule across the pill at `y`.
pub fn hline(r: &Renderer, y: f32, alpha: f32) {
    let p = exp();
    r.fill_rect(RectF { l: p.l + 6.0, t: y, r: p.r - 6.0, b: y + 1.0 }.d2d(), color(0x22252B, alpha));
}

pub fn keycap_width(r: &Renderer, text: &str) -> f32 {
    (r.text_w(F::Keycap, text) + 16.0).max(26.0)
}

/// A small keyboard-key badge with its left edge at `x`, vertically centred on `cy`.
/// Returns its right edge.
pub fn keycap(r: &Renderer, text: &str, x: f32, cy: f32, alpha: f32) -> f32 {
    let w = keycap_width(r, text);
    let rect = RectF { l: x, t: cy - 8.5, r: x + w, b: cy + 8.5 };
    r.fill_rrect(rect.d2d(), 5.0, color(0x20232A, alpha));
    r.text(text, F::Keycap, rect.d2d(), color(0xE6E8EB, alpha));
    x + w
}

pub fn draw(r: &mut Renderer, v: &View) -> Result<()> {
    r.begin();

    let w = v.w.clamp(COLLAPSED_W * 0.5, WIN_W);
    let h = v.h.clamp(COLLAPSED_H * 0.5, WIN_H - TOP);
    let pill = pill_rect(w, h);
    let t = progress(h);
    let rad = radius(h);

    // Body: pure black while collapsed (it reads as part of the screen bezel), easing to the
    // panel colour as it opens. Hairline border keeps it visible on dark wallpapers.
    r.fill_rrect(pill.d2d(), rad, color(mix(0x000000, BODY, t), 1.0));
    r.stroke_rrect(pill.inset(0.5).d2d(), rad - 0.5, color(0xFFFFFF, 0.09), 1.0);

    // collapsed indicator: a faint dash that fades as the pill opens
    let dash_a = (1.0 - t * 4.0).clamp(0.0, 1.0) * 0.28;
    if dash_a > 0.0 {
        let d = RectF { l: pill.cx() - 16.0, t: pill.cy() - 2.0, r: pill.cx() + 16.0, b: pill.cy() + 2.0 };
        r.fill_rrect(d.d2d(), 2.0, color(0xFFFFFF, dash_a));
    }

    // Content is laid out for the fully open size and fades in late; the clip keeps it inside
    // the pill while the body is still growing.
    let alpha = smoothstep(0.55, 1.0, t);
    if alpha > 0.01 {
        r.push_clip(pill.d2d());
        draw_header(r, v, alpha);
        match v.tab {
            Tab::Notes => notes_view::draw(r, v, alpha),
            Tab::Clipboard => clip_list::draw(r, v, alpha),
        }
        r.pop_clip();
    }

    r.end()
}

fn draw_header(r: &Renderer, v: &View, alpha: f32) {
    let bg = seg_bg();
    r.fill_rrect(bg.d2d(), bg.h() / 2.0, color(0x15171C, alpha));

    // (rect, label, glyph, tab, hit, active fill, active accent)
    let segments = [
        (seg_notes(), "Notepad", '\u{E8A5}', Tab::Notes, Hit::TabNotes, 0x3B2F1B, AMBER),
        (seg_clip(), "Clipboard", '\u{E77F}', Tab::Clipboard, Hit::TabClip, 0x1E3A47, SKY),
    ];
    for (rect, label, glyph, tab, hit, fill, accent) in segments {
        let active = v.tab == tab;
        if active {
            r.fill_rrect(rect.d2d(), rect.h() / 2.0, color(fill, alpha));
        } else if v.hover == hit {
            r.fill_rrect(rect.d2d(), rect.h() / 2.0, color(0xFFFFFF, 0.05 * alpha));
        }
        let fg = if active { accent } else if v.hover == hit { 0xC9CDD3 } else { MUTED };
        r.glyph(glyph, F::Icon16, rect.l + 22.0, rect.cy(), color(fg, alpha));
        let text = RectF { l: rect.l + 37.8, t: rect.t, r: rect.r - 4.0, b: rect.b };
        r.text(label, F::Seg, text.d2d(), color(fg, alpha));
    }

    // keep-open pin and collapse chevron
    let pin = btn_pin();
    if v.hover == Hit::BtnPin {
        r.fill_circle(pin.cx(), pin.cy(), 14.0, color(0xFFFFFF, 0.07 * alpha));
    }
    let pin_c = if v.keep_open { PIN_AMBER } else if v.hover == Hit::BtnPin { 0xE6E8EB } else { MUTED };
    r.pin(pin.cx(), pin.cy(), 15.0, color(pin_c, alpha), v.keep_open);

    let ch = btn_collapse();
    if v.hover == Hit::BtnCollapse {
        r.fill_circle(ch.cx(), ch.cy(), 14.0, color(0xFFFFFF, 0.07 * alpha));
    }
    let ch_c = if v.hover == Hit::BtnCollapse { 0xE6E8EB } else { MUTED };
    r.chevron_up(ch.cx(), ch.cy(), 5.5, color(ch_c, alpha));
}
