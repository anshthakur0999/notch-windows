//! Devices' card: what pops up on the pill when a device connects (the Devices tab is in
//! `devices_tab`). A ring sweeps round a slightly wider capsule, which then opens into a card with the device's name
//! and battery (earbuds also get "Use for audio"), or into a short notice (the output was switched,
//! a device disconnected).
//!
//! Geometry is in DIPs, as offsets from the expanded pill like the other tabs; the card's own is
//! offsets from the animated capsule's left and top, so it grows out of the corner like the hover
//! capsule does.

use super::timer_view::{grown, lit};
use super::*;
use crate::audio::OutKind;
use crate::devices::{Device, Kind};
use crate::render::{F, Renderer, color, mix};

// ---------------------------------------------------------------- what the card says

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NoticeKind {
    /// "Now the audio output": after "Use for audio" worked, or did not.
    Switched,
    Gone,
    /// "Reconnected": the device was here a moment ago.
    Back,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Notice {
    pub kind: NoticeKind,
    /// The kind of device, for its icon.
    pub icon: Kind,
    pub title: String,
    pub sub: Option<String>,
    /// Width of the capsule: fitted to the text when the notice is made.
    pub w: f32,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Card {
    /// A device connected: its name and battery, and "Use for audio" for earbuds and the like.
    Connected(Device),
    /// The battery fell to 20% or 10%: the same card without the buttons.
    Low(Device),
    Notice(Notice),
}

impl Card {
    /// The size the capsule opens to.
    pub fn size(&self) -> (f32, f32) {
        match self {
            Card::Connected(d) if d.kind.is_audio() => (CARD_W, CARD_H_AUDIO),
            Card::Connected(_) | Card::Low(_) => (CARD_W, CARD_H),
            Card::Notice(n) => (n.w, NOTICE_H),
        }
    }

    pub fn has_buttons(&self) -> bool {
        matches!(self, Card::Connected(d) if d.kind.is_audio())
    }

    /// The device the card is about, for the icon the opening capsule shows.
    pub fn kind(&self) -> Kind {
        match self {
            Card::Connected(d) | Card::Low(d) => d.kind,
            Card::Notice(n) => n.icon,
        }
    }
}

pub(super) fn kind_glyph(k: Kind) -> char {
    match k {
        Kind::Audio => '\u{E7F6}',
        Kind::Mouse => '\u{E962}',
        Kind::Keyboard => '\u{E765}',
        Kind::Controller => '\u{E7FC}',
    }
}

pub(super) fn out_glyph(k: OutKind) -> char {
    match k {
        OutKind::Speakers => '\u{E767}',
        OutKind::Headphones => '\u{E7F6}',
        OutKind::Display => '\u{E7F4}',
    }
}

/// Right-aligned text whose right edge is at `right`, in a left-aligned style `f`.
pub(super) fn right_text(r: &Renderer, s: &str, f: F, right: f32, top: f32, h: f32, c: windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F) {
    let w = r.measure(f, s);
    r.text(s, f, RectF { l: right - w - 2.0, t: top, r: right + 4.0, b: top + h }.d2d(), c);
}

/// A little battery: an outline, its nub and a sliver of charge, centred on (cx, cy).
pub(super) fn battery_icon(r: &Renderer, cx: f32, cy: f32, c: windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F) {
    let body = RectF { l: cx - 7.0, t: cy - 4.0, r: cx + 6.0, b: cy + 4.0 };
    r.stroke_rrect(body.inset(0.6).d2d(), 2.0, c, 1.2);
    r.fill_rrect(RectF { l: cx + 6.5, t: cy - 2.0, r: cx + 8.0, b: cy + 2.0 }.d2d(), 0.6, c);
    r.fill_rrect(RectF { l: cx - 5.0, t: cy - 2.0, r: cx - 3.0, b: cy + 2.0 }.d2d(), 0.8, c);
}

/// A battery bar: a track with the charge in it (never narrower than a dot, so 3% is visible).
pub(super) fn battery_bar(r: &Renderer, bar: RectF, pct: u8, fill: u32, alpha: f32) {
    let pal = r.pal;
    r.fill_rrect(bar.d2d(), bar.h() / 2.0, color(mix(pal.rest, pal.wash, 0.14), alpha));
    let w = (bar.w() * pct as f32 / 100.0).max(bar.h());
    r.fill_rrect(RectF { r: bar.l + w, ..bar }.d2d(), bar.h() / 2.0, color(fill, alpha));
}

// ---------------------------------------------------------------- the card on the pill

const PAD: f32 = 13.0;
pub(super) const BAR_W: f32 = 49.0;

fn card_tile(p: RectF) -> RectF {
    RectF { l: p.l + PAD, t: p.t + PAD, r: p.l + PAD + 48.0, b: p.t + PAD + 48.0 }
}
/// "Use for audio" (under the device's row) and the x beside it, as offsets from the capsule.
fn use_btn(p: RectF) -> RectF {
    RectF { l: p.l + PAD, t: p.t + 72.0, r: p.l + PAD + 260.0, b: p.t + 116.0 }
}
const CLOSE_R: f32 = 22.0;
fn close_c(p: RectF) -> (f32, f32) {
    (p.l + CARD_W - PAD - CLOSE_R, p.t + 94.0)
}

/// Hit-test on the open card; the buttons are only live once the capsule has grown to hold them.
pub fn card_hit(p: RectF, x: f32, y: f32, buttons: bool) -> Hit {
    if buttons && p.w() > CARD_W - 40.0 && p.h() > CARD_H_AUDIO - 24.0 {
        let c = close_c(p);
        if (x - c.0).hypot(y - c.1) <= CLOSE_R {
            return Hit::DeviceClose;
        }
        if use_btn(p).contains(x, y) {
            return Hit::DeviceUse;
        }
    }
    Hit::Pill
}

/// The capsule in all its states: the ring and the icons of the opening, then the card or the notice.
pub fn draw_card(r: &Renderer, v: &View, pill: RectF) {
    let Some(cv) = &v.card else { return };
    let (target_w, _) = cv.card.size();
    let a = grown(pill, target_w);
    let pal = r.pal;
    if cv.spark {
        // the opening: a ring sweeps once round the wider capsule while the device's icon fades in,
        // and both give way as the card grows
        let p = (cv.age / 0.15).clamp(0.0, 1.0);
        if p < 1.0 {
            let rad = pill::radius(pill.h());
            r.sweep_rrect(pill.inset(1.2).d2d(), rad - 1.2, color(pal.device, 1.0 - pill::smoothstep(0.75, 1.0, p)), 1.7, p);
        }
        let ia = pill::smoothstep(0.0, 0.12, cv.age) * (1.0 - a);
        if ia > 0.02 {
            let (cx, cy) = (pill.cx(), pill.cy());
            r.glyph('\u{E702}', F::Icon14, cx - 17.0, cy, color(pal.device, ia));
            let tile = RectF { l: cx + 1.0, t: cy - 13.0, r: cx + 27.0, b: cy + 13.0 };
            r.fill_rrect(tile.d2d(), 8.0, color(mix(pal.rest, pal.device, 0.24), ia));
            r.glyph(kind_glyph(cv.card.kind()), F::Icon14, tile.cx(), tile.cy(), color(pal.device, ia));
        }
    }
    if a < 0.02 {
        return;
    }
    r.push_clip(pill.d2d());
    match cv.card {
        Card::Connected(d) | Card::Low(d) => device_card(r, v, d, cv.card.has_buttons(), pill, a),
        Card::Notice(n) => notice(r, n, pill, a),
    }
    r.pop_clip();
}

fn device_card(r: &Renderer, v: &View, d: &Device, buttons: bool, p: RectF, a: f32) {
    let pal = r.pal;
    let tint = if d.low() { pal.danger } else { pal.device };
    let tile = card_tile(p);
    r.fill_rrect(tile.d2d(), 15.0, color(mix(pal.rest, tint, 0.2), a));
    r.glyph(kind_glyph(d.kind), F::Icon24, tile.cx(), tile.cy(), color(tint, a));

    let (x, right) = (tile.r + 12.0, p.l + CARD_W - PAD);
    let words = |top: f32, l: f32| RectF { l, t: p.t + top, r: right - 62.0, b: p.t + top + 20.0 };
    r.text(&d.name, F::Seg, words(18.0, x).d2d(), color(pal.text, a));
    if d.low() {
        battery_icon(r, x + 7.0, p.t + 48.0, color(pal.danger, a));
        r.text("Battery low", F::Tab, words(38.0, x + 20.0).d2d(), color(pal.danger, a));
    } else {
        r.fill_circle(x + 3.5, p.t + 48.0, 3.0, color(pal.device, a));
        r.text("Connected", F::Tab, words(38.0, x + 14.0).d2d(), color(pal.muted, a));
    }
    match d.battery {
        Some(pct) => {
            let s = format!("{pct}%");
            let w = r.measure(F::Head, &s);
            r.text(&s, F::Head, RectF { l: right - w - 3.0, t: p.t + 17.0, r: right + 3.0, b: p.t + 45.0 }.d2d(), color(if d.low() { pal.danger } else { pal.text }, a));
            battery_bar(r, RectF { l: right - BAR_W, t: p.t + 51.0, r: right, b: p.t + 56.0 }, pct, tint, a);
        }
        // never a made-up number
        None => right_text(r, "Battery unknown", F::Hint, right, p.t + 28.0, 18.0, color(pal.muted, a)),
    }

    if buttons {
        let b = use_btn(p);
        r.fill_rrect(b.d2d(), b.h() / 2.0, color(lit(r, pal.device, v.hover == Hit::DeviceUse), a));
        let label = "Use for audio";
        let lw = r.measure(F::Seg, label);
        let x0 = b.cx() - (16.0 + 8.0 + lw) / 2.0;
        r.glyph('\u{E767}', F::Icon16, x0 + 8.0, b.cy(), color(pal.ink, a));
        r.text(label, F::Seg, RectF { l: x0 + 24.0, t: b.t, r: x0 + 30.0 + lw, b: b.b }.d2d(), color(pal.ink, a));
        let c = close_c(p);
        r.fill_circle(c.0, c.1, CLOSE_R, color(lit(r, pal.tile, v.hover == Hit::DeviceClose), a));
        r.glyph('\u{E711}', F::Icon14, c.0, c.1, color(pal.text, a));
    }
}

fn notice(r: &Renderer, n: &Notice, p: RectF, a: f32) {
    let pal = r.pal;
    let cy = p.cy();
    let tile = RectF { l: p.l + 8.0, t: cy - 18.0, r: p.l + 44.0, b: cy + 18.0 };
    let gone = n.kind == NoticeKind::Gone;
    r.fill_rrect(tile.d2d(), 11.0, color(if gone { pal.tile } else { mix(pal.rest, pal.device, 0.24) }, a));
    let glyph = if n.kind == NoticeKind::Switched { '\u{E767}' } else { kind_glyph(n.icon) };
    r.glyph(glyph, F::Icon16, tile.cx(), tile.cy(), color(if gone { pal.muted } else { pal.device }, a));
    let check = n.kind == NoticeKind::Switched;
    let right = p.l + n.w - if check { 36.0 } else { 14.0 };
    let (t_top, t_bot) = if n.sub.is_some() { (-19.0, -1.0) } else { (-9.0, 9.0) };
    r.text(&n.title, F::Seg, RectF { l: p.l + 54.0, t: cy + t_top, r: right, b: cy + t_bot }.d2d(), color(pal.text, a));
    if let Some(sub) = &n.sub {
        r.text(sub, F::Hint, RectF { l: p.l + 54.0, t: cy, r: right, b: cy + 18.0 }.d2d(), color(pal.muted, a));
    }
    if check {
        r.glyph('\u{E73E}', F::Icon16, p.l + n.w - 22.0, cy, color(pal.device, a));
    }
}

/// A section title at the left of `row` and its note (a hint, a count) at the right.
pub(super) fn heading(r: &Renderer, title: &str, note: &str, row: RectF, alpha: f32) {
    let pal = r.pal;
    r.text(title, F::Seg, RectF { l: row.l + 8.0, ..row }.d2d(), color(pal.muted, alpha));
    right_text(r, note, F::Hint, row.r - 12.0, row.t + 2.0, row.h() - 4.0, color(pal.muted, alpha));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(kind: Kind, battery: Option<u8>) -> Device {
        Device { id: "x".into(), name: "AirPods Pro".into(), kind, battery, addr: None }
    }

    #[test]
    fn only_earbuds_get_the_audio_row_and_notices_are_short() {
        assert_eq!(Card::Connected(dev(Kind::Audio, Some(82))).size(), (CARD_W, CARD_H_AUDIO));
        assert_eq!(Card::Connected(dev(Kind::Mouse, Some(41))).size(), (CARD_W, CARD_H), "a mouse is not an audio output");
        assert_eq!(Card::Connected(dev(Kind::Controller, None)).size(), (CARD_W, CARD_H));
        assert_eq!(Card::Low(dev(Kind::Audio, Some(20))).size(), (CARD_W, CARD_H), "the alert has no buttons");
        assert!(Card::Connected(dev(Kind::Audio, None)).has_buttons());
        assert!(!Card::Low(dev(Kind::Audio, Some(20))).has_buttons());
        assert!(!Card::Connected(dev(Kind::Keyboard, Some(90))).has_buttons());
        let n = Notice { kind: NoticeKind::Gone, icon: Kind::Audio, title: "x".into(), sub: None, w: 240.0 };
        assert_eq!(Card::Notice(n).size(), (240.0, NOTICE_H));
        // every capsule fits the window with room for the spring's overshoot
        for h in [CARD_H, CARD_H_AUDIO, NOTICE_H, SPARK_H] {
            assert!(TOP + h + 20.0 < WIN_H);
        }
        assert!(CARD_W + 40.0 < WIN_W && SPARK_W > COLLAPSED_W && NOTICE_MIN_W > COLLAPSED_W);
    }

    #[test]
    fn the_card_buttons_sit_inside_it_apart_and_only_work_once_it_is_open() {
        let open = pill_rect(CARD_W, CARD_H_AUDIO);
        let (u, c) = (use_btn(open), close_c(open));
        assert!(u.l >= open.l && u.b <= open.b - 8.0, "the button is inside the card");
        assert!(c.0 + CLOSE_R <= open.r && c.1 + CLOSE_R <= open.b - 8.0, "so is the x");
        assert!(u.r < c.0 - CLOSE_R, "the two do not touch");
        assert!(card_tile(open).b < u.t, "the button is under the device's row");
        assert_eq!(card_hit(open, u.cx(), u.cy(), true), Hit::DeviceUse);
        assert_eq!(card_hit(open, c.0, c.1, true), Hit::DeviceClose);
        assert_eq!(card_hit(open, u.cx(), u.cy(), false), Hit::Pill, "a card without buttons has none to press");
        assert_eq!(card_hit(open, open.cx(), open.t + 10.0, true), Hit::Pill, "elsewhere: the card (opens the Devices tab)");
        // the round x is round: the corner of the box around it is not a hit
        assert_eq!(card_hit(open, c.0 + CLOSE_R * 0.9, c.1 + CLOSE_R * 0.9, true), Hit::Pill);
        // still opening: nothing to press where the buttons are not yet
        let small = pill_rect(SPARK_W, SPARK_H);
        assert_eq!(card_hit(small, small.l + u.cx() - open.l, small.t + 94.0, true), Hit::Pill);
    }
}
