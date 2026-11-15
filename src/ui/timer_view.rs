//! The timer's drawing: the Timer tab (the "set a time" form, the running view and the "time's up"
//! view) and the small pieces of it shown elsewhere: the header segment, the resting pill, the copy
//! peek's right-hand slot and the two capsules (hover and time's up).
//!
//! Geometry is in DIPs, as offsets from the expanded pill like the other tabs.

use super::pill::{self, smoothstep};
use super::*;
use crate::render::{F, Renderer, color, mix};
use crate::timer::{Phase, clock, parse};

/// The quick-pick chips: what is shown, and what it puts in the time box.
pub const PRESETS: [(&str, &str); 5] = [("5 min", "5m"), ("10 min", "10m"), ("25 min", "25m"), ("45 min", "45m"), ("1 hour", "1h")];

// ---------------------------------------------------------------- panel geometry

pub fn time_field() -> RectF {
    at(14.0, 80.0, 412.0, 68.0)
}
pub fn label_field() -> RectF {
    at(14.0, 254.0, 412.0, 37.4)
}
pub fn chip(i: usize) -> RectF {
    at(17.0 + i as f32 * 74.0, 185.0, 66.0, 36.0)
}
pub fn start_btn() -> RectF {
    at(14.0, 356.0, 412.0, 47.0)
}

const RING_R: f32 = 95.0;
const RING_W: f32 = 10.0;
fn ring_centre() -> (f32, f32) {
    let p = exp();
    (p.cx(), p.t + 178.0)
}
pub fn btn_stop() -> RectF {
    at(117.0, 339.0, 44.0, 44.0)
}
pub fn btn_pause() -> RectF {
    at(174.5, 330.5, 61.0, 61.0)
}
pub fn btn_plus() -> RectF {
    at(249.0, 339.0, 74.5, 44.0)
}
/// After the time ran out: "Done" and "+5 min" replace the three round buttons.
pub fn btn_done() -> RectF {
    at(126.0, 339.0, 92.0, 44.0)
}
pub fn btn_add5() -> RectF {
    at(230.0, 339.0, 84.0, 44.0)
}

fn round(r: RectF, x: f32, y: f32) -> bool {
    (x - r.cx()).hypot(y - r.cy()) <= r.w() / 2.0
}

/// Hit-test inside the Timer tab (`None` = not on a control).
pub fn hit(x: f32, y: f32, phase: Phase) -> Option<Hit> {
    match phase {
        Phase::Idle => {
            if time_field().contains(x, y) {
                Some(Hit::TimerField)
            } else if label_field().contains(x, y) {
                Some(Hit::TimerLabel)
            } else if start_btn().contains(x, y) {
                Some(Hit::TimerStart)
            } else {
                (0..PRESETS.len()).find(|&i| chip(i).contains(x, y)).map(Hit::TimerPreset)
            }
        }
        Phase::Running | Phase::Paused => {
            if round(btn_stop(), x, y) {
                Some(Hit::TimerStop)
            } else if round(btn_pause(), x, y) {
                Some(Hit::TimerPause)
            } else if btn_plus().contains(x, y) {
                Some(Hit::TimerPlus)
            } else {
                None
            }
        }
        Phase::Finished => {
            if btn_done().contains(x, y) {
                Some(Hit::TimerDone)
            } else if btn_add5().contains(x, y) {
                Some(Hit::TimerAdd5)
            } else {
                None
            }
        }
    }
}

// ---------------------------------------------------------------- capsule geometry

/// The hover capsule, laid out from the capsule's own rectangle `p`. With a timer and nothing else:
/// notepad button, pause / resume ring, the time, clipboard button; with files on the shelf the
/// capsule is wider and reads notepad, clipboard, shelf, ring, time. Media adds its buttons at the
/// right end (the cover, which opens the Media tab, and play / pause); with no timer it is notepad,
/// clipboard, shelf if any, then the cover, the title and artist, and play / pause.
struct HoverLayout {
    notes: (f32, f32),
    clip: (f32, f32),
    shelf: Option<(f32, f32)>,
    /// The timer's ring and its time (only while a timer runs).
    ring: Option<(f32, f32)>,
    time: Option<RectF>,
    media: Option<MediaLayout>,
}

struct MediaLayout {
    art: (f32, f32),
    /// Where the title and artist fit (not beside a timer).
    words: Option<RectF>,
    play: (f32, f32),
}
const BTN_R: f32 = 17.0;

fn hover_layout(p: RectF, shelf: bool, timer: bool, media: bool) -> HoverLayout {
    let cy = p.cy();
    let at = |dx: f32| (p.l + dx, cy);
    match (timer, media) {
        (true, false) if shelf => HoverLayout {
            notes: at(23.0),
            clip: at(63.0),
            shelf: Some(at(103.0)),
            ring: Some(at(143.0)),
            time: Some(RectF { l: p.l + 164.0, t: p.t, r: p.r - 6.0, b: p.b }),
            media: None,
        },
        (true, false) => HoverLayout {
            notes: at(23.0),
            ring: Some(at(63.0)),
            time: Some(RectF { l: p.l + 84.0, t: p.t, r: p.l + 166.0, b: p.b }),
            clip: (p.r - 23.0, cy),
            shelf: None,
            media: None,
        },
        _ => {
            // notepad, clipboard, then the shelf button if there are files
            let mut dx = 103.0;
            let shelf_c = shelf.then(|| at(dx));
            if shelf {
                dx += 40.0;
            }
            let (ring, time, art, words) = if timer {
                // the ring and its time (room for "1:24:58"), then the cover; there is no room for the title
                let time = RectF { l: p.l + dx + 21.0, t: p.t, r: p.l + dx + 77.0, b: p.b };
                (Some(at(dx)), Some(time), at(dx + 77.0 + 22.0), None)
            } else {
                (None, None, at(dx), Some(RectF { l: p.l + dx + 23.0, t: p.t, r: p.r - 23.0 - BTN_R - 6.0, b: p.b }))
            };
            let play = if timer { (art.0 + 40.0, cy) } else { (p.r - 23.0, cy) };
            HoverLayout {
                notes: at(23.0),
                clip: at(63.0),
                shelf: shelf_c,
                ring,
                time,
                media: Some(MediaLayout { art, words, play }),
            }
        }
    }
}

fn within(c: (f32, f32), r: f32, x: f32, y: f32) -> bool {
    (x - c.0).hypot(y - c.1) <= r
}

pub fn hover_hit(p: RectF, x: f32, y: f32, shelf: bool, timer: bool, media: bool) -> Hit {
    let l = hover_layout(p, shelf, timer, media);
    if within(l.notes, BTN_R, x, y) {
        Hit::PeekNotes
    } else if l.shelf.is_some_and(|c| within(c, BTN_R, x, y)) {
        Hit::PeekShelf
    } else if l.ring.is_some_and(|c| within(c, BTN_R, x, y)) {
        Hit::PeekRing
    } else if within(l.clip, BTN_R, x, y) {
        Hit::PeekClip
    } else if l.time.is_some_and(|t| t.contains(x, y)) {
        Hit::PeekTime
    } else if let Some(m) = &l.media {
        if within(m.play, BTN_R, x, y) {
            Hit::PeekPlay
        } else if within(m.art, BTN_R, x, y) || m.words.is_some_and(|w| w.contains(x, y)) {
            Hit::PeekArt
        } else {
            Hit::Pill
        }
    } else {
        Hit::Pill
    }
}

/// The time's-up capsule: a bell, the words, then "+5 min" and "Done" at the right end.
struct TimesUpLayout {
    bell: (f32, f32),
    text_x: f32,
    add5: RectF,
    done: RectF,
}

fn times_up_layout(p: RectF) -> TimesUpLayout {
    let cy = p.cy();
    let done = RectF { l: p.r - 5.0 - 64.0, t: cy - 18.0, r: p.r - 5.0, b: cy + 18.0 };
    let add5 = RectF { l: done.l - 6.0 - 74.0, t: cy - 18.0, r: done.l - 6.0, b: cy + 18.0 };
    TimesUpLayout { bell: (p.l + 23.0, cy), text_x: p.l + 50.0, add5, done }
}

pub fn times_up_hit(p: RectF, x: f32, y: f32) -> Hit {
    let l = times_up_layout(p);
    if l.done.contains(x, y) {
        Hit::TimerDone
    } else if l.add5.contains(x, y) {
        Hit::TimerAdd5
    } else {
        Hit::Pill
    }
}

// ---------------------------------------------------------------- drawing helpers

/// A section label such as "SET A TIME".
fn section(r: &Renderer, s: &str, at: RectF, alpha: f32) {
    r.text_spaced(s, F::Section, at.d2d(), color(r.pal.subtle, alpha), 1.0);
}

/// `s` centred in `rect`, in a left-aligned style.
pub fn centred(r: &Renderer, s: &str, f: F, rect: RectF, c: D2D1_COLOR_F) {
    let w = r.measure(f, s);
    let (l, rr) = (rect.cx() - w / 2.0 - 1.0, rect.cx() + w / 2.0 + 3.0);
    r.text(s, f, RectF { l, t: rect.t, r: rr, b: rect.b }.d2d(), c);
}

/// Lightens a fill a little while the pointer is on it.
pub fn lit(r: &Renderer, base: u32, hovered: bool) -> u32 {
    mix(base, r.pal.wash, if hovered { 0.09 } else { 0.0 })
}

fn caret(r: &Renderer, x: f32, cy: f32, half: f32, alpha: f32) {
    r.line(x, cy - half, x, cy + half, color(r.pal.timer, alpha * 0.9), 1.5);
}

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

// ---------------------------------------------------------------- the Timer tab

pub fn draw(r: &Renderer, v: &View, alpha: f32) {
    match v.timer.phase {
        Phase::Idle => draw_form(r, v, alpha),
        Phase::Running | Phase::Paused => draw_running(r, v, alpha),
        Phase::Finished => draw_finished(r, v, alpha),
    }
    let p = exp();
    let cy = p.t + FOOTER_CY;
    pill::hline(r, p.t + FOOTER_SEP, alpha);
    r.text(
        "Keeps running in the pill when you collapse",
        F::Hint,
        RectF { l: p.l + 20.0, t: cy - 9.0, r: p.l + 300.0, b: cy + 9.0 }.d2d(),
        color(r.pal.muted, alpha),
    );
    pill::collapse_hint(r, cy, alpha);
}

fn draw_form(r: &Renderer, v: &View, alpha: f32) {
    let (pal, t) = (r.pal, &v.timer);
    section(r, "SET A TIME", at(20.0, 58.0, 220.0, 20.0), alpha);

    // the time box: what is typed, and what it means
    let f = time_field();
    r.fill_rrect(f.d2d(), 22.0, color(pal.field, alpha));
    let typing = v.focused && t.focus == TimerField::Time;
    if typing {
        r.stroke_rrect(f.inset(0.6).d2d(), 21.4, color(pal.timer, 0.5 * alpha), 1.2);
    }
    let text = RectF { l: f.l + 26.0, t: f.t, r: f.r - 130.0, b: f.b };
    if t.input.is_empty() {
        r.text("25m", F::Field, text.d2d(), color(pal.faint, alpha));
    } else {
        r.text(t.input, F::Field, text.d2d(), color(pal.text, alpha));
    }
    if typing {
        caret(r, (text.l + r.measure(F::Field, t.input) + 1.0).min(text.r), f.cy(), 18.0, alpha);
    }
    if let Some(secs) = t.parsed {
        let s = format!("= {}", clock(secs));
        let w = r.measure(F::Title, &s);
        r.text(&s, F::Title, RectF { l: f.r - 26.0 - w - 4.0, t: f.t, r: f.r - 22.0, b: f.b }.d2d(), color(pal.muted, alpha));
    }
    let hint = match t.parsed {
        Some(_) => format!("Try 25m, 1h 30m or 90s. Ends at {}.", t.ends),
        None => "Try 25m, 1h 30m or 90s.".to_string(),
    };
    r.text(&hint, F::Tab, at(20.0, 152.0, 400.0, 22.0).d2d(), color(pal.subtle, alpha));

    // quick picks
    for (i, (label, typed)) in PRESETS.iter().enumerate() {
        let c = chip(i);
        let on = t.parsed.is_some() && t.parsed == parse(typed);
        let fill = if on { mix(pal.body, pal.timer, 0.22) } else { lit(r, pal.tile, v.hover == Hit::TimerPreset(i)) };
        r.fill_rrect(c.d2d(), 18.0, color(fill, alpha));
        if on {
            r.stroke_rrect(c.inset(0.6).d2d(), 17.4, color(pal.timer, 0.55 * alpha), 1.2);
        }
        r.text(label, F::Center, c.d2d(), color(if on { pal.timer } else { pal.text2 }, alpha));
    }

    // the label box
    section(r, "LABEL (OPTIONAL)", at(20.0, 232.0, 260.0, 20.0), alpha);
    let l = label_field();
    r.fill_rrect(l.d2d(), l.h() / 2.0, color(pal.field, alpha));
    let typing = v.focused && t.focus == TimerField::Label;
    if typing {
        r.stroke_rrect(l.inset(0.6).d2d(), l.h() / 2.0 - 0.6, color(pal.timer, 0.5 * alpha), 1.2);
    }
    let text = RectF { l: l.l + 18.0, t: l.t, r: l.r - 16.0, b: l.b };
    if t.label_input.is_empty() {
        r.text("Name this timer", F::Title, text.d2d(), color(pal.faint, alpha));
    } else {
        r.text(t.label_input, F::Title, text.d2d(), color(pal.text, alpha));
    }
    if typing {
        caret(r, (text.l + r.measure(F::Title, t.label_input) + 1.0).min(text.r), l.cy(), 8.5, alpha);
    }

    // Start
    let b = start_btn();
    match t.parsed {
        Some(secs) => {
            r.fill_rrect(b.d2d(), b.h() / 2.0, color(lit(r, pal.timer, v.hover == Hit::TimerStart), alpha));
            let label = format!("Start {}", clock(secs));
            let (tw, key_w) = (r.measure(F::Seg, &label), pill::keycap_width(r, "Enter"));
            let x = b.cx() - (24.0 + tw + 10.0 + key_w) / 2.0;
            r.glyph('\u{F5B0}', F::Icon14, x + 7.0, b.cy(), color(pal.ink, alpha));
            r.text(&label, F::Seg, RectF { l: x + 24.0, t: b.t, r: x + 28.0 + tw, b: b.b }.d2d(), color(pal.ink, alpha));
            let k = RectF { l: x + 34.0 + tw, t: b.cy() - 10.0, r: x + 34.0 + tw + key_w, b: b.cy() + 10.0 };
            r.fill_rrect(k.d2d(), 6.0, color(mix(pal.timer, pal.ink, 0.22), alpha));
            r.text("Enter", F::Keycap, k.d2d(), color(pal.ink, alpha));
        }
        None => {
            r.fill_rrect(b.d2d(), b.h() / 2.0, color(pal.tile, alpha));
            centred(r, "Start", F::Seg, b, color(pal.faint, alpha));
        }
    }
}

fn draw_running(r: &Renderer, v: &View, alpha: f32) {
    let (pal, t) = (r.pal, &v.timer);
    let paused = t.phase == Phase::Paused;
    let dim = if paused { 0.6 } else { 1.0 };
    let (cx, cy) = ring_centre();
    r.ring(cx, cy, RING_R, RING_W, color(mix(pal.body, pal.wash, 0.09), alpha));
    r.arc(cx, cy, RING_R, RING_W, color(pal.timer, alpha * dim), t.frac * 360.0);
    big_clock(r, &clock(t.left), (cx, cy), color(pal.text, alpha * dim));
    if !t.label.is_empty() {
        r.text(t.label, F::Center, RectF { l: cx - 80.0, t: cy + 19.0, r: cx + 80.0, b: cy + 41.0 }.d2d(), color(pal.muted, alpha));
    }
    let sub = if paused { "Paused".to_string() } else { format!("Ends at {}", t.ends) };
    r.text(&sub, F::Center, at(70.0, 296.0, 300.0, 22.0).d2d(), color(pal.muted, alpha));

    let (stop, pause, plus) = (btn_stop(), btn_pause(), btn_plus());
    r.fill_circle(stop.cx(), stop.cy(), stop.w() / 2.0, color(lit(r, pal.tile, v.hover == Hit::TimerStop), alpha));
    r.glyph('\u{E711}', F::Icon16, stop.cx(), stop.cy(), color(pal.text, alpha));
    r.fill_circle(pause.cx(), pause.cy(), pause.w() / 2.0, color(lit(r, pal.timer, v.hover == Hit::TimerPause), alpha));
    r.glyph(if paused { '\u{F5B0}' } else { '\u{F8AE}' }, F::Icon16, pause.cx(), pause.cy(), color(pal.ink, alpha));
    r.fill_rrect(plus.d2d(), plus.h() / 2.0, color(lit(r, pal.tile, v.hover == Hit::TimerPlus), alpha));
    centred(r, "+1 min", F::Seg, plus, color(pal.text, alpha));
}

fn draw_finished(r: &Renderer, v: &View, alpha: f32) {
    let (pal, t) = (r.pal, &v.timer);
    let (cx, cy) = ring_centre();
    r.ring(cx, cy, RING_R, RING_W + 8.0, color(pal.timer, 0.12 * alpha)); // glow
    r.ring(cx, cy, RING_R, RING_W, color(pal.timer, alpha));
    big_clock(r, &clock(0), (cx, cy), color(pal.text, alpha));
    let sub = if t.label.is_empty() { "Time's up".to_string() } else { format!("{} \u{00B7} Time's up", t.label) };
    r.text(&sub, F::Center, RectF { l: cx - 90.0, t: cy + 19.0, r: cx + 90.0, b: cy + 41.0 }.d2d(), color(pal.timer, alpha));

    let (done, add5) = (btn_done(), btn_add5());
    r.fill_rrect(done.d2d(), done.h() / 2.0, color(lit(r, pal.timer, v.hover == Hit::TimerDone), alpha));
    centred(r, "Done", F::Seg, done, color(pal.ink, alpha));
    r.fill_rrect(add5.d2d(), add5.h() / 2.0, color(lit(r, pal.tile, v.hover == Hit::TimerAdd5), alpha));
    centred(r, "+5 min", F::Seg, add5, color(pal.text, alpha));
}

/// The big countdown, centred a little above the ring's centre `c`.
fn big_clock(r: &Renderer, s: &str, c: (f32, f32), colr: D2D1_COLOR_F) {
    let w = r.clock_width(F::Clock, s);
    r.clock_text(s, F::Clock, c.0 - w / 2.0, c.1 - 11.0, colr);
}

// ---------------------------------------------------------------- small readouts

/// Width of the small readout (ring or glyph, then the digits).
pub fn chip_width(r: &Renderer, t: &TimerView) -> f32 {
    21.0 + r.clock_width(F::ClockSm, &clock(t.left))
}

/// The small readout with its left edge at `x`, centred on `cy`: a ring showing the time left (a
/// play mark while paused, a bell once the time ran out), then the digits.
pub fn draw_chip(r: &Renderer, t: &TimerView, x: f32, cy: f32, ring: u32, digits: u32, alpha: f32) {
    let c = (x + 7.0, cy);
    match t.phase {
        Phase::Paused => r.glyph('\u{F5B0}', F::Icon12, c.0, cy, color(ring, alpha * 0.8)),
        Phase::Finished => r.glyph('\u{EA8F}', F::Icon12, c.0, cy, color(ring, alpha)),
        _ => {
            r.ring(c.0, cy, 5.5, 2.0, color(r.pal.wash, 0.18 * alpha));
            r.arc(c.0, cy, 5.5, 2.0, color(ring, alpha), t.frac * 360.0);
        }
    }
    let digits = if t.phase == Phase::Paused { r.pal.muted } else { digits };
    r.clock_text(&clock(t.left), F::ClockSm, x + 21.0, cy, color(digits, alpha));
}

/// The Timer segment of the header: "Timer" while idle, the readout while timing (on any tab).
pub fn draw_segment(r: &Renderer, v: &View, rect: RectF, active: bool, hovered: bool, alpha: f32) {
    let pal = r.pal;
    if v.timer.phase == Phase::Idle {
        let fg = if active { pal.timer } else if hovered { pal.text2 } else { pal.muted };
        // only the open tab shows its name; the others are just the icon
        r.glyph('\u{E916}', F::Icon16, if active { rect.l + 22.0 } else { rect.cx() }, rect.cy(), color(fg, alpha));
        if active {
            r.text("Timer", F::Seg, RectF { l: rect.l + 37.8, t: rect.t, r: rect.r - 4.0, b: rect.b }.d2d(), color(fg, alpha));
        }
    } else {
        let digits = if active { pal.timer } else if hovered { pal.text } else { pal.text2 };
        draw_chip(r, &v.timer, rect.l + 15.5, rect.cy(), pal.timer, digits, alpha);
    }
}

/// The copy peek's right-hand slot: a divider, then the readout.
pub fn draw_peek_slot(r: &Renderer, v: &View, pill: RectF, alpha: f32) {
    let t = &v.timer;
    if !t.active() || t.peek_slot <= 0.0 {
        return;
    }
    let x = pill.r - 18.0 - chip_width(r, t);
    r.line(x - 12.0, pill.cy() - 10.0, x - 12.0, pill.cy() + 10.0, color(r.pal.wash, 0.16 * alpha), 1.0);
    draw_chip(r, t, x, pill.cy(), r.pal.timer, r.pal.text, alpha);
}

// ---------------------------------------------------------------- the capsules

/// How far the capsule has widened towards `target` (0..1), eased: its content fades in with it.
pub fn grown(pill: RectF, target: f32) -> f32 {
    smoothstep(0.5, 1.0, (pill.w() - COLLAPSED_W) / (target - COLLAPSED_W))
}

/// Hover capsule: notepad button, pause / resume ring, the time, clipboard button; media's buttons
/// join them (see `HoverLayout`).
pub fn draw_hover(r: &Renderer, v: &View, pill: RectF) {
    let shelf = v.shelf.items.len();
    let (pal, t) = (r.pal, &v.timer);
    let a = grown(pill, hover_w(t.active(), shelf > 0, v.media.is_some()));
    if a < 0.02 {
        return;
    }
    let l = hover_layout(pill, shelf > 0, t.active(), v.media.is_some());
    let tint = |accent: u32, hovered: bool| lit(r, mix(pal.rest, accent, 0.22), hovered);

    r.fill_circle(l.notes.0, l.notes.1, BTN_R, color(tint(pal.notes, v.hover == Hit::PeekNotes), a));
    r.glyph('\u{E8A5}', F::Icon14, l.notes.0, l.notes.1, color(pal.notes, a));

    if let (Some(ring), Some(time)) = (l.ring, l.time) {
        r.fill_circle(ring.0, ring.1, BTN_R, color(lit(r, pal.tile, v.hover == Hit::PeekRing), a));
        r.ring(ring.0, ring.1, 15.0, 2.2, color(pal.wash, 0.16 * a));
        r.arc(ring.0, ring.1, 15.0, 2.2, color(pal.timer, a), t.frac * 360.0);
        r.glyph(if t.phase == Phase::Paused { '\u{F5B0}' } else { '\u{F8AE}' }, F::Icon12, ring.0, ring.1, color(pal.text, a));

        let digits = if t.phase == Phase::Paused { pal.muted } else { pal.text };
        let s = clock(t.left);
        r.clock_text(&s, F::ClockSm, time.l + 4.0, pill.cy(), color(digits, a));
    }

    if let (Some(m), Some(ml)) = (&v.media, &l.media) {
        media_view::draw_hover_art(r, ml.art, matches!(v.hover, Hit::PeekArt), a);
        if let Some(w) = ml.words {
            let cy = pill.cy();
            let line = |dy: f32| RectF { t: cy + dy, b: cy + dy + 18.0, ..w };
            r.text(&m.track.title, F::Seg, line(-19.0).d2d(), color(pal.text, a));
            let sub = if m.track.artist.is_empty() { &m.track.source } else { &m.track.artist };
            r.text(sub, F::Hint, line(0.0).d2d(), color(pal.muted, a));
        }
        r.fill_circle(ml.play.0, ml.play.1, BTN_R, color(tint(pal.media, v.hover == Hit::PeekPlay), a));
        let glyph = if m.track.playing { '\u{F8AE}' } else { '\u{F5B0}' };
        r.glyph(glyph, F::Icon12, ml.play.0, ml.play.1, color(pal.media, a));
    }

    r.fill_circle(l.clip.0, l.clip.1, BTN_R, color(tint(pal.clip, v.hover == Hit::PeekClip), a));
    r.glyph('\u{E77F}', F::Icon14, l.clip.0, l.clip.1, color(pal.clip, a));

    if let Some(c) = l.shelf {
        r.fill_circle(c.0, c.1, BTN_R, color(tint(pal.shelf, v.hover == Hit::PeekShelf), a));
        r.tray(c.0, c.1, 15.0, color(pal.shelf, a));
        let (bx, by) = (c.0 + 11.0, c.1 - 11.0); // how many files it holds
        r.fill_circle(bx, by, 7.5, color(pal.shelf, a));
        r.text(&shelf.to_string(), F::Keycap, RectF { l: bx - 8.0, t: by - 8.0, r: bx + 8.0, b: by + 8.0 }.d2d(), color(pal.ink, a));
    }
}

/// Time's-up capsule: a glowing rim, a bell, "Time's up" (and the label), "+5 min" and "Done".
pub fn draw_times_up(r: &Renderer, v: &View, pill: RectF) {
    let (pal, t) = (r.pal, &v.timer);
    let rad = pill::radius(pill.h());
    r.stroke_rrect(pill.inset(1.5).d2d(), rad - 1.5, color(pal.timer, 0.28), 3.0); // glow
    r.stroke_rrect(pill.inset(0.5).d2d(), rad - 0.5, color(pal.timer, 0.85), 1.0);
    let a = grown(pill, TIMES_UP_W);
    if a < 0.02 {
        return;
    }
    let l = times_up_layout(pill);
    let cy = pill.cy();
    r.fill_circle(l.bell.0, l.bell.1, 18.0, color(mix(pal.rest, pal.timer, 0.28), a));
    r.glyph('\u{EA8F}', F::Icon14, l.bell.0, l.bell.1, color(pal.timer, a));

    let words = RectF { l: l.text_x, t: cy - 9.0, r: l.add5.l - 6.0, b: cy + 9.0 };
    if t.label.is_empty() {
        r.text("Time's up", F::Seg, words.d2d(), color(pal.text, a));
    } else {
        let up = |dy: f32| RectF { t: words.t + dy, b: words.b + dy, ..words };
        r.text("Time's up", F::Seg, up(-9.0).d2d(), color(pal.text, a));
        r.text(t.label, F::Hint, up(9.0).d2d(), color(pal.muted, a));
    }
    r.fill_rrect(l.add5.d2d(), 18.0, color(lit(r, pal.tile, v.hover == Hit::TimerAdd5), a));
    centred(r, "+5 min", F::Seg, l.add5, color(pal.text, a));
    r.fill_rrect(l.done.d2d(), 18.0, color(lit(r, pal.timer, v.hover == Hit::TimerDone), a));
    centred(r, "Done", F::Seg, l.done, color(pal.ink, a));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_has_a_control_under_each_point_and_nothing_under_the_gaps() {
        let c = |r: RectF| (r.cx(), r.cy());
        let (x, y) = c(time_field());
        assert_eq!(hit(x, y, Phase::Idle), Some(Hit::TimerField));
        let (x, y) = c(label_field());
        assert_eq!(hit(x, y, Phase::Idle), Some(Hit::TimerLabel));
        let (x, y) = c(start_btn());
        assert_eq!(hit(x, y, Phase::Idle), Some(Hit::TimerStart));
        for i in 0..PRESETS.len() {
            let (x, y) = c(chip(i));
            assert_eq!(hit(x, y, Phase::Idle), Some(Hit::TimerPreset(i)));
        }
        let (x, y) = c(chip(0));
        assert_eq!(hit(x, y + 20.0, Phase::Idle), None, "between the chips and the label box");
        // the form's controls never overlap each other
        let boxes = [time_field(), label_field(), start_btn(), chip(0), chip(4)];
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                assert!(a.r <= b.l || b.r <= a.l || a.b <= b.t || b.b <= a.t, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn running_and_finished_views_expose_only_their_own_buttons() {
        let (x, y) = (btn_stop().cx(), btn_stop().cy());
        assert_eq!(hit(x, y, Phase::Running), Some(Hit::TimerStop));
        assert_eq!(hit(x, y, Phase::Paused), Some(Hit::TimerStop));
        assert_eq!(hit(x, y, Phase::Finished), Some(Hit::TimerDone), "Done takes the place of the stop button");
        assert_eq!(hit(btn_done().cx(), btn_done().cy(), Phase::Running), None, "Done does not exist while timing");
        assert_eq!(hit(btn_pause().cx(), btn_pause().cy(), Phase::Paused), Some(Hit::TimerPause));
        assert_eq!(hit(btn_plus().cx(), btn_plus().cy(), Phase::Running), Some(Hit::TimerPlus));
        assert_eq!(hit(btn_done().cx(), btn_done().cy(), Phase::Finished), Some(Hit::TimerDone));
        assert_eq!(hit(btn_add5().cx(), btn_add5().cy(), Phase::Finished), Some(Hit::TimerAdd5));
        // the round buttons are round: a corner of the bounding box is not a hit
        let s = btn_stop();
        assert_eq!(hit(s.l + 1.0, s.t + 1.0, Phase::Running), None);
        // while idle the form's Start button is under that same spot, which is why a double-click on
        // stop must not count its second click (see `last_timer_stop`)
        assert_eq!(hit(x, y, Phase::Idle), Some(Hit::TimerStart));
        // the three buttons sit side by side without touching
        assert!(btn_stop().r < btn_pause().l && btn_pause().r < btn_plus().l);
    }

    #[test]
    fn the_capsule_buttons_are_where_they_are_drawn() {
        let p = pill_rect(HOVER_W, PEEK_H);
        let l = hover_layout(p, false, true, false);
        let (ring, time) = (l.ring.expect("a ring"), l.time.expect("a time"));
        assert_eq!(hover_hit(p, l.notes.0, l.notes.1, false, true, false), Hit::PeekNotes);
        assert_eq!(hover_hit(p, ring.0, ring.1, false, true, false), Hit::PeekRing);
        assert_eq!(hover_hit(p, l.clip.0, l.clip.1, false, true, false), Hit::PeekClip);
        assert_eq!(hover_hit(p, time.cx(), p.cy(), false, true, false), Hit::PeekTime);
        assert_eq!(hover_hit(p, time.r + 5.0, p.cy(), false, true, false), Hit::Pill, "elsewhere on the capsule opens the panel");
        // everything fits inside the capsule, with the time clear of both buttons
        assert!(l.notes.0 - BTN_R >= p.l && l.clip.0 + BTN_R <= p.r);
        assert!(ring.0 + BTN_R < time.l && time.r < l.clip.0 - BTN_R);

        // with files on the shelf the capsule has a shelf button and reads notepad, clipboard, shelf, ring, time
        let p = pill_rect(HOVER_SHELF_W, PEEK_H);
        let l = hover_layout(p, true, true, false);
        let (ring, time) = (l.ring.expect("a ring"), l.time.expect("a time"));
        let shelf = l.shelf.expect("a shelf button");
        assert_eq!(hover_hit(p, shelf.0, shelf.1, true, true, false), Hit::PeekShelf);
        assert_eq!(hover_hit(p, l.clip.0, l.clip.1, true, true, false), Hit::PeekClip);
        assert_eq!(hover_hit(p, ring.0, ring.1, true, true, false), Hit::PeekRing);
        assert_ne!(hover_hit(p, shelf.0, shelf.1, false, true, false), Hit::PeekShelf, "no shelf button without files");
        let xs = [l.notes.0, l.clip.0, shelf.0, ring.0];
        assert!(xs.windows(2).all(|w| w[0] + 2.0 * BTN_R < w[1]), "the four buttons do not touch");
        assert!(l.notes.0 - BTN_R >= p.l && ring.0 + BTN_R < time.l && time.r <= p.r);

        let p = pill_rect(TIMES_UP_W, PEEK_H);
        let l = times_up_layout(p);
        assert_eq!(times_up_hit(p, l.done.cx(), l.done.cy()), Hit::TimerDone);
        assert_eq!(times_up_hit(p, l.add5.cx(), l.add5.cy()), Hit::TimerAdd5);
        assert_eq!(times_up_hit(p, l.bell.0, l.bell.1), Hit::Pill);
        assert!(l.done.r <= p.r && l.add5.r < l.done.l && l.text_x + 80.0 < l.add5.l, "room for the words");
    }

    #[test]
    fn media_in_the_hover_capsule_has_its_buttons_apart_and_inside() {
        for (timer, shelf) in [(true, false), (true, true), (false, false), (false, true)] {
            let p = pill_rect(hover_w(timer, shelf, true), PEEK_H);
            let l = hover_layout(p, shelf, timer, true);
            let m = l.media.as_ref().expect("media buttons");
            let hit = |c: (f32, f32)| hover_hit(p, c.0, c.1, shelf, timer, true);
            assert_eq!(hit(m.art), Hit::PeekArt, "timer={timer} shelf={shelf}");
            assert_eq!(hit(m.play), Hit::PeekPlay, "timer={timer} shelf={shelf}");
            assert_eq!(hit(l.notes), Hit::PeekNotes);
            assert_eq!(hit(l.clip), Hit::PeekClip);
            if let Some(c) = l.shelf {
                assert_eq!(hit(c), Hit::PeekShelf);
            }
            if let Some(c) = l.ring {
                assert_eq!(hit(c), Hit::PeekRing);
            }
            // left to right with room between: notepad, clipboard, [shelf], [ring, time], cover, [words], play
            let mut xs = vec![l.notes.0, l.clip.0];
            xs.extend(l.shelf.map(|c| c.0));
            xs.extend(l.ring.map(|c| c.0));
            xs.push(m.art.0);
            xs.push(m.play.0);
            assert!(xs.windows(2).all(|w| w[0] + BTN_R < w[1] - BTN_R + 0.1), "{xs:?} do not touch");
            assert!(l.notes.0 - BTN_R >= p.l && m.play.0 + BTN_R <= p.r, "inside the capsule");
            if let Some(t) = l.time {
                assert!(l.ring.unwrap().0 + BTN_R < t.l && t.r <= m.art.0 - BTN_R, "the time clears the ring and the cover");
            }
            if let Some(w) = m.words {
                assert!(w.l >= m.art.0 + BTN_R && w.r <= m.play.0 - BTN_R && w.w() >= 100.0, "room for the title: {w:?}");
                assert_eq!(hit((w.cx(), p.cy())), Hit::PeekArt, "the title opens the Media tab too");
            } else {
                assert!(timer, "only beside a timer is there no room for the title");
            }
        }
        // without media the capsule is the timer's and has nothing to press for it
        let p = pill_rect(HOVER_W, PEEK_H);
        assert!(hover_layout(p, false, true, false).media.is_none());
    }

    #[test]
    fn the_header_has_room_for_five_tabs_and_the_buttons() {
        let expect = [Hit::TabNotes, Hit::TabClip, Hit::TabTimer, Hit::TabShelf, Hit::TabMedia];
        for open in Tab::ALL {
            for timer_on in [false, true] {
                let h = header(open, timer_on);
                assert!(h.seg.windows(2).all(|w| w[0].r < w[1].l), "{open:?}: tabs do not touch");
                assert!(h.seg[4].r <= h.bg.r && h.bg.r < btn_pin().l, "{open:?}: the track ends before the pin");
                for (i, t) in Tab::ALL.into_iter().enumerate() {
                    let c = h.of(t);
                    assert_eq!(hit_test_tab(open, timer_on, c.cx(), c.cy()), expect[i], "{open:?} timer_on={timer_on} {t:?}");
                }
                // the open tab is wide enough for its name, a running timer for "1:24:58"
                assert!(h.of(open).w() >= 82.0 && (!timer_on || h.of(Tab::Timer).w() >= 96.0));
            }
        }
    }

    fn hit_test_tab(open: Tab, timer_on: bool, x: f32, y: f32) -> Hit {
        let (list, tabs) = (ListLayout::default(), NoteTabs::empty());
        let ctx = HitCtx {
            w: EXPANDED_W,
            h: EXPANDED_H,
            expanded: true,
            tab: open,
            scroll: 0.0,
            list: &list,
            tabs: &tabs,
            timer: if timer_on { Phase::Running } else { Phase::Idle },
            capsule: Capsule::None,
            shelf_n: 0,
            media: false,
            seekable: false,
        };
        hit_test(x, y, &ctx)
    }
}
