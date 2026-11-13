//! Draws the pill itself: body, collapsed indicator, the segmented header, and shared bits
//! (hairlines, key caps) used by both tabs.

use super::*;
use crate::peek;
use crate::render::{F, Renderer, color, mix};
use windows::core::Result;

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
    r.fill_rect(RectF { l: p.l + 6.0, t: y, r: p.r - 6.0, b: y + 1.0 }.d2d(), color(r.pal.rule, alpha));
}

pub fn keycap_width(r: &Renderer, text: &str) -> f32 {
    (r.text_w(F::Keycap, text) + 16.0).max(26.0)
}

/// A small keyboard-key badge with its left edge at `x`, vertically centred on `cy`.
/// Returns its right edge.
pub fn keycap(r: &Renderer, text: &str, x: f32, cy: f32, alpha: f32) -> f32 {
    let w = keycap_width(r, text);
    let rect = RectF { l: x, t: cy - 8.5, r: x + w, b: cy + 8.5 };
    r.fill_rrect(rect.d2d(), 5.0, color(r.pal.cap, alpha));
    r.text(text, F::Keycap, rect.d2d(), color(r.pal.text, alpha));
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

    // Body: pure black while collapsed on the dark theme (it reads as part of the screen bezel),
    // easing to the panel colour as it opens. Hairline border keeps it visible on any wallpaper.
    // The collapsed indicator (a faint dash) gives way to the peek's badge.
    // (and it only comes back once the text has gone too, so it never shows through "Copied")
    let dash_gone = peek.map_or(0.0, |p| (p.look.icon + p.look.old_icon).max(p.look.label).max(p.look.line).min(1.0));
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
    let pal = r.pal;
    r.fill_rrect(pill.d2d(), rad, color(mix(pal.rest, pal.body, t), alpha));
    r.stroke_rrect(pill.inset(0.5).d2d(), rad - 0.5, color(pal.wash, pal.edge * alpha), 1.0);

    // collapsed indicator: a faint dash that fades as the pill opens
    let dash_a = (1.0 - t * 4.0).clamp(0.0, 1.0) * 0.28 * dash.clamp(0.0, 1.0) * alpha;
    if dash_a > 0.0 {
        let d = RectF { l: pill.cx() - 16.0, t: pill.cy() - 2.0, r: pill.cx() + 16.0, b: pill.cy() + 2.0 };
        r.fill_rrect(d.d2d(), 2.0, color(pal.wash, dash_a));
    }
}

/// The copy-peek capsule's content: a check badge (or the image's thumbnail), "Copied", and what
/// was copied. It is anchored to the capsule's left edge, so while the capsule widens the badge
/// leads and the text is revealed behind it. When a copy replaces another the old preview fades
/// out as the new one fades in (`look.old_line` / `look.line`).
fn draw_peek(r: &mut Renderer, v: &View, pill: RectF, pk: &PeekView) {
    let look = pk.look;
    if look.icon + look.old_icon < 0.01 {
        return;
    }
    let c = pk.content;
    let pal = r.pal;
    let label = "Copied";
    let label_w = r.text_w(F::Seg, label);
    let new_lay = peek::layout(c.target_w, c.slack, label_w);
    let old_lay = pk.prev.map(|o| peek::layout(o.target_w, o.slack, label_w));
    // badge and label glide from where the previous content had them to where this one wants them
    let lay = old_lay.map_or(new_lay, |o| peek::lerp_layout(o, new_lay, look.shift));
    let cy = pill.cy();

    // keep everything inside the capsule's rounded right end while it is still narrow
    r.push_clip(RectF { l: pill.l, t: pill.t, r: pill.r - 12.0, b: pill.b }.d2d());

    let badge_x = pill.l + lay.lead_x + peek::LEAD / 2.0;
    if let (Some(o), true) = (pk.prev, look.old_icon > 0.01) {
        draw_badge(r, v, o.thumb, badge_x, cy, look.old_icon, 1.0, 1.0);
    }
    draw_badge(r, v, c.thumb, badge_x, cy, look.icon, look.pop, look.check);

    // "Copied" and the preview slide out from behind the badge as they fade in
    let dx = look.slide;
    if look.label > 0.01 {
        let label_r = RectF { l: pill.l + lay.label_x + dx, t: pill.t, r: pill.l + lay.label_x + dx + label_w + 2.0, b: pill.b };
        r.text(label, F::Seg, label_r.d2d(), color(pal.text, look.label));
    }
    if let (Some(o), Some(ol)) = (pk.prev, old_lay) {
        if !o.line.is_empty() && look.old_line > 0.01 {
            let text_r = RectF { l: pill.l + ol.text_x, t: pill.t, r: pill.l + ol.text_x + ol.text_w, b: pill.b };
            r.text(&o.line, F::Tab, text_r.d2d(), color(pal.text2, look.old_line));
        }
    }
    if !c.line.is_empty() && look.line > 0.01 {
        let text_r = RectF { l: pill.l + lay.text_x + dx, t: pill.t, r: pill.l + lay.text_x + dx + lay.text_w, b: pill.b };
        r.text(&c.line, F::Tab, text_r.d2d(), color(pal.text2, look.line));
    }
    r.pop_clip();
}

/// The capsule's leading badge centred on (`cx`, `cy`): an accent disc whose check mark draws
/// itself (`check` is how much of it is drawn), or the copied image as a circle. `pop` scales it.
#[allow(clippy::too_many_arguments)]
fn draw_badge(r: &mut Renderer, v: &View, thumb: Option<u64>, cx: f32, cy: f32, alpha: f32, pop: f32, check: f32) {
    // below this the disc is a barely-there ghost over the returning dash: skip it
    if alpha < 0.04 {
        return;
    }
    let pal = r.pal;
    let rad = 17.0 * pop;
    match thumb {
        Some(id) => match r.thumb(id, &v.store.thumb_path(id)) {
            Some(bmp) => {
                // A circle, the same size as the check badge: the picture fills it (cropped to the
                // centre), with a faint rim so a dark screenshot does not vanish into the capsule.
                let disc = RectF { l: cx - rad, t: cy - rad, r: cx + rad, b: cy + rad };
                r.fill_rrect_cover(&bmp, disc.d2d(), rad, alpha);
                r.stroke_rrect(disc.inset(0.5).d2d(), (rad - 0.5).max(0.0), color(pal.wash, 0.14 * alpha), 1.0);
            }
            None => {
                r.fill_circle(cx, cy, rad, color(pal.tile, alpha));
                r.glyph('\u{E91B}', F::Icon16, cx, cy, color(pal.muted, alpha));
            }
        },
        None => {
            r.fill_circle(cx, cy, rad, color(pal.clip, alpha));
            // The check is two strokes (the short leg, then the long one); each is drawn as far as
            // `check` has got, in proportion to its length (about 1 : 2).
            let ink = color(pal.ink, alpha);
            let at = |dx: f32, dy: f32| (cx + dx * pop, cy + dy * pop);
            let (a, b, c) = (at(-5.4, 0.4), at(-1.6, 4.0), at(5.6, -3.6));
            let (f1, f2) = ((check / 0.33).clamp(0.0, 1.0), ((check - 0.33) / 0.67).clamp(0.0, 1.0));
            let w = 2.2 * pop;
            if f1 > 0.0 {
                r.line(a.0, a.1, a.0 + (b.0 - a.0) * f1, a.1 + (b.1 - a.1) * f1, ink, w);
            }
            if f2 > 0.0 {
                r.line(b.0, b.1, b.0 + (c.0 - b.0) * f2, b.1 + (c.1 - b.1) * f2, ink, w);
            }
        }
    }
}

fn draw_header(r: &Renderer, v: &View, alpha: f32) {
    let pal = r.pal;
    let bg = seg_bg();
    r.fill_rrect(bg.d2d(), bg.h() / 2.0, color(pal.seg, alpha));

    // (rect, label, glyph, tab, hit, accent)
    let segments = [
        (seg_notes(), "Notepad", '\u{E8A5}', Tab::Notes, Hit::TabNotes, pal.notes),
        (seg_clip(), "Clipboard", '\u{E77F}', Tab::Clipboard, Hit::TabClip, pal.clip),
    ];
    for (rect, label, glyph, tab, hit, accent) in segments {
        let active = v.tab == tab;
        if active {
            r.fill_rrect(rect.d2d(), rect.h() / 2.0, color(pal.seg_active(accent), alpha));
        } else if v.hover == hit {
            r.fill_rrect(rect.d2d(), rect.h() / 2.0, color(pal.wash, 0.05 * alpha));
        }
        let fg = if active { accent } else if v.hover == hit { pal.text2 } else { pal.muted };
        r.glyph(glyph, F::Icon16, rect.l + 22.0, rect.cy(), color(fg, alpha));
        let text = RectF { l: rect.l + 37.8, t: rect.t, r: rect.r - 4.0, b: rect.b };
        r.text(label, F::Seg, text.d2d(), color(fg, alpha));
    }

    // keep-open pin and collapse chevron
    let pin = btn_pin();
    if v.hover == Hit::BtnPin {
        r.fill_circle(pin.cx(), pin.cy(), 14.0, color(pal.wash, 0.07 * alpha));
    }
    let pin_c = if v.keep_open { pal.pin } else if v.hover == Hit::BtnPin { pal.text } else { pal.muted };
    r.pin(pin.cx(), pin.cy(), 15.0, color(pin_c, alpha), v.keep_open);

    let ch = btn_collapse();
    if v.hover == Hit::BtnCollapse {
        r.fill_circle(ch.cx(), ch.cy(), 14.0, color(pal.wash, 0.07 * alpha));
    }
    let ch_c = if v.hover == Hit::BtnCollapse { pal.text } else { pal.muted };
    r.chevron_up(ch.cx(), ch.cy(), 5.5, color(ch_c, alpha));
}
