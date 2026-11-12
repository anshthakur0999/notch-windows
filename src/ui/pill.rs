//! Draws the pill itself: body, collapsed indicator, the segmented header, and shared bits
//! (hairlines, key caps) used by both tabs.

use super::*;
use crate::peek;
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

/// Corner radius for the current animated size. A short pill is a full capsule (the resting
/// pill and the copy-peek capsule); it eases to the panel radius as it opens.
pub fn radius(h: f32) -> f32 {
    lerp((h * 0.5).min(PEEK_H * 0.5), RADIUS_OPEN, progress(h))
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

    let peek = v.peek.as_ref();
    let body_a = peek.map_or(1.0, |p| p.look.body);
    if body_a < 0.999 {
        // Reduced motion: the capsule cross-fades over the resting pill, which stays underneath.
        let rest = pill_rect(COLLAPSED_W, COLLAPSED_H);
        draw_body(r, rest, COLLAPSED_H / 2.0, 0.0, 1.0, 1.0);
    }

    // Body: pure black while collapsed (it reads as part of the screen bezel), easing to the
    // panel colour as it opens. Hairline border keeps it visible on dark wallpapers.
    // The collapsed indicator (a faint dash) gives way to the peek's badge.
    let dash_gone = peek.map_or(0.0, |p| p.look.icon);
    draw_body(r, pill, rad, t, body_a, 1.0 - dash_gone);

    if let Some(p) = peek {
        draw_peek(r, v, pill, p);
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

/// The pill's body, hairline border and (when collapsed) the faint dash. `t` is how open it is,
/// `alpha` the body's opacity, `dash` how much of the dash is left.
fn draw_body(r: &Renderer, pill: RectF, rad: f32, t: f32, alpha: f32, dash: f32) {
    r.fill_rrect(pill.d2d(), rad, color(mix(0x000000, BODY, t), alpha));
    r.stroke_rrect(pill.inset(0.5).d2d(), rad - 0.5, color(0xFFFFFF, 0.09 * alpha), 1.0);

    // collapsed indicator: a faint dash that fades as the pill opens
    let dash_a = (1.0 - t * 4.0).clamp(0.0, 1.0) * 0.28 * dash.clamp(0.0, 1.0) * alpha;
    if dash_a > 0.0 {
        let d = RectF { l: pill.cx() - 16.0, t: pill.cy() - 2.0, r: pill.cx() + 16.0, b: pill.cy() + 2.0 };
        r.fill_rrect(d.d2d(), 2.0, color(0xFFFFFF, dash_a));
    }
}

/// The copy-peek capsule's content: a check badge (or the image's thumbnail), "Copied", and what
/// was copied. It is anchored to the capsule's left edge, so while the capsule widens the badge
/// leads and the text is revealed behind it.
fn draw_peek(r: &mut Renderer, v: &View, pill: RectF, pk: &PeekView) {
    let look = pk.look;
    if look.icon < 0.01 {
        return;
    }
    let c = pk.content;
    let label = "Copied";
    let label_w = r.text_w(F::Seg, label);
    let lay = peek::layout(c.target_w, c.slack, label_w);
    let cy = pill.cy();

    // keep everything inside the capsule's rounded right end while it is still narrow
    r.push_clip(RectF { l: pill.l, t: pill.t, r: pill.r - 12.0, b: pill.b }.d2d());

    let lead = RectF { l: pill.l + lay.lead_x, t: cy - peek::LEAD / 2.0, r: pill.l + lay.lead_x + peek::LEAD, b: cy + peek::LEAD / 2.0 };
    match c.thumb {
        Some(id) => match r.thumb(id, &v.store.thumb_path(id)) {
            Some(bmp) => {
                // A circle, the same size as the check badge: the picture fills it (cropped to the
                // centre), with a faint rim so a dark screenshot does not vanish into the black
                // capsule.
                let disc = RectF { l: lead.cx() - 17.0, t: cy - 17.0, r: lead.cx() + 17.0, b: cy + 17.0 };
                r.fill_rrect_cover(&bmp, disc.d2d(), 17.0, look.icon);
                r.stroke_rrect(disc.inset(0.5).d2d(), 16.5, color(0xFFFFFF, 0.14 * look.icon), 1.0);
            }
            None => {
                r.fill_circle(lead.cx(), cy, 17.0, color(0x1B1E24, look.icon));
                r.glyph('\u{E91B}', F::Icon16, lead.cx(), cy, color(0x9AA0A8, look.icon));
            }
        },
        None => {
            // sky-blue badge with a dark check mark
            r.fill_circle(lead.cx(), cy, 17.0, color(SKY, look.icon));
            let (x, y) = (lead.cx(), cy);
            let ink = color(0x06222E, look.icon);
            r.line(x - 5.4, y + 0.4, x - 1.6, y + 4.0, ink, 2.2);
            r.line(x - 1.6, y + 4.0, x + 5.6, y - 3.6, ink, 2.2);
        }
    }

    if look.text > 0.01 {
        let label_r = RectF { l: pill.l + lay.label_x, t: pill.t, r: pill.l + lay.label_x + label_w + 2.0, b: pill.b };
        r.text(label, F::Seg, label_r.d2d(), color(0xF1F3F5, look.text));
        if !c.line.is_empty() {
            let text_r = RectF { l: pill.l + lay.text_x, t: pill.t, r: pill.l + lay.text_x + lay.text_w, b: pill.b };
            r.text(&c.line, F::Tab, text_r.d2d(), color(0xC9CDD3, look.text));
        }
    }
    r.pop_clip();
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
