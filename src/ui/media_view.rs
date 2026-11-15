//! Live media's drawing: the Media tab (the cover, what is playing, the visualizer, the seek bar and
//! the buttons, or the empty state) and the pieces of it on the pill: the chip on the resting pill
//! and the "now playing" banner. The hover capsule's media buttons are drawn by `timer_view`, which
//! lays that capsule out.
//!
//! Geometry is in DIPs, as offsets from the expanded pill like the other tabs.

use super::timer_view::{grown, lit};
use super::*;
use crate::render::{F, Renderer, color, mix};
use crate::viz::{BANDS, PILL_BARS, fold};
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

// ---------------------------------------------------------------- tab geometry

pub fn art_box() -> RectF {
    at(23.0, 57.0, 109.0, 109.0)
}
pub fn wave_box() -> RectF {
    at(23.0, 189.0, 394.0, 60.0)
}
/// The seek bar's hit area; the line and its knob are drawn inside it, inset by the knob's radius.
pub fn seek_bar() -> RectF {
    at(23.0, 270.0, 394.0, 28.0)
}
pub fn bars_btn() -> RectF {
    at(23.0, 354.0, 72.0, 38.0)
}
fn centre(dx: f32, dy: f32) -> (f32, f32) {
    let p = at(dx, dy, 0.0, 0.0);
    (p.l, p.t)
}
pub fn prev_c() -> (f32, f32) {
    centre(155.0, 373.0)
}
pub fn play_c() -> (f32, f32) {
    centre(220.0, 373.0)
}
pub fn next_c() -> (f32, f32) {
    centre(285.0, 373.0)
}
const SKIP_R: f32 = 24.0;
const PLAY_R: f32 = 31.0;
const KNOB_R: f32 = 6.5;

/// How far along the track a click at `x` on the seek bar is (0..1).
pub fn seek_frac(x: f32) -> f32 {
    let s = seek_bar();
    ((x - (s.l + KNOB_R)) / (s.w() - 2.0 * KNOB_R)).clamp(0.0, 1.0)
}

/// Hit-test inside the Media tab. With nothing playing there are no controls.
pub fn hit(x: f32, y: f32, has_track: bool, seekable: bool) -> Option<Hit> {
    if !has_track {
        return None;
    }
    let near = |c: (f32, f32), r: f32| (x - c.0).hypot(y - c.1) <= r;
    if near(play_c(), PLAY_R) {
        Some(Hit::MediaPlay)
    } else if near(prev_c(), SKIP_R) {
        Some(Hit::MediaPrev)
    } else if near(next_c(), SKIP_R) {
        Some(Hit::MediaNext)
    } else if bars_btn().contains(x, y) {
        Some(Hit::MediaStyle)
    } else if seekable && seek_bar().contains(x, y) {
        Some(Hit::MediaSeek)
    } else {
        None
    }
}

/// "2:11", or "1:02:11" from an hour up.
fn mmss(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

// ---------------------------------------------------------------- shared pieces

/// A music note on a 16 x 16 grid scaled to `size`, centred on (cx, cy): two hollow heads, their
/// stems and a beam.
pub fn note(r: &Renderer, cx: f32, cy: f32, size: f32, c: D2D1_COLOR_F) {
    let u = size / 16.0;
    let p = |x: f32, y: f32| (cx + (x - 8.0) * u, cy + (y - 8.0) * u);
    for (hx, hy) in [(4.6, 12.2), (11.8, 10.6)] {
        let h = p(hx, hy);
        r.ring(h.0, h.1, 2.3 * u, 1.6 * u, c);
    }
    let line = |a: (f32, f32), b: (f32, f32), w: f32| {
        let (a, b) = (p(a.0, a.1), p(b.0, b.1));
        r.line(a.0, a.1, b.0, b.1, c, w * u);
    };
    line((6.9, 12.0), (6.9, 3.6), 1.6);
    line((14.1, 10.4), (14.1, 2.2), 1.6);
    line((6.9, 3.6), (14.1, 2.2), 2.0);
}

/// The cover filling `rect` (cropped to fit), or a tinted tile with a note while there is none.
/// `base` is the colour behind it: the pill's, or the open panel's.
fn art(r: &Renderer, rect: RectF, radius: f32, note_size: f32, base: u32, alpha: f32) {
    let pal = r.pal;
    // the pill's covers (20 to 34 DIP) come from the small copy, the Media tab's from the big one
    let cover = if rect.w() <= 40.0 { r.media_small.as_ref() } else { r.media_art.as_ref() };
    match cover {
        Some(bmp) => {
            r.fill_rrect_art(bmp, rect.d2d(), radius, alpha);
            r.stroke_rrect(rect.inset(0.5).d2d(), radius - 0.5, color(pal.wash, 0.12 * alpha), 1.0);
        }
        None => {
            r.fill_rrect(rect.d2d(), radius, color(mix(base, pal.media, 0.2), alpha));
            r.stroke_rrect(rect.inset(0.5).d2d(), radius - 0.5, color(pal.media, 0.28 * alpha), 1.0);
            note(r, rect.cx(), rect.cy(), note_size, color(pal.media, alpha));
        }
    }
}

/// What the little bars show while the visualizer is off but the music plays: a fixed pattern.
const STILL: [f32; PILL_BARS] = [0.45, 0.8, 1.0, 0.65, 0.4];

/// The pill's equaliser: `PILL_BARS` thin bars with their left edge at `x`, centred on `cy`, 22 wide
/// and up to 15 tall; or the pause mark while paused.
fn mini_bars(r: &Renderer, m: &MediaView, x: f32, cy: f32, alpha: f32) {
    let pal = r.pal;
    if !m.track.playing {
        for dx in [3.0, 11.0] {
            r.fill_rrect(RectF { l: x + dx, t: cy - 6.0, r: x + dx + 3.5, b: cy + 6.0 }.d2d(), 1.5, color(pal.text2, alpha));
        }
        return;
    }
    let levels = if m.bars_on { fold(&m.levels, PILL_BARS) } else { STILL.to_vec() };
    for (i, level) in levels.iter().enumerate() {
        let h = 3.5 + level * 11.5;
        let l = x + i as f32 * 4.8;
        r.fill_rrect(RectF { l, t: cy - h / 2.0, r: l + 2.6, b: cy + h / 2.0 }.d2d(), 1.3, color(pal.media, alpha));
    }
}

// ---------------------------------------------------------------- on the pill

/// Width of the media chip on the resting pill: the cover, then the bars.
pub const CHIP_W: f32 = 51.0;

/// The chip with its left edge at `x`, centred on `cy`.
pub fn draw_chip(r: &Renderer, m: &MediaView, x: f32, cy: f32, alpha: f32) {
    art(r, RectF { l: x, t: cy - 10.0, r: x + 20.0, b: cy + 10.0 }, 10.0, 12.0, r.pal.rest, alpha);
    mini_bars(r, m, x + 29.0, cy, alpha);
}

/// The hover capsule's cover button: a circle the size of its neighbours (`timer_view::BTN_R`),
/// centred on `c`.
pub fn draw_hover_art(r: &Renderer, c: (f32, f32), hovered: bool, alpha: f32) {
    const R: f32 = 17.0;
    let rect = RectF { l: c.0 - R, t: c.1 - R, r: c.0 + R, b: c.1 + R };
    art(r, rect, R, 16.0, r.pal.rest, alpha);
    if hovered {
        r.fill_circle(c.0, c.1, R, color(r.pal.wash, 0.12 * alpha));
    }
}

/// The "now playing" banner the pill widens into when the track changes: the cover, the title and
/// artist, and the bars.
pub fn draw_banner(r: &Renderer, v: &View, pill: RectF) {
    let Some(m) = &v.media else { return };
    let a = grown(pill, TRACK_W);
    if a < 0.02 {
        return;
    }
    let (pal, t, cy) = (r.pal, m.track, pill.cy());
    art(r, RectF { l: pill.l + 7.0, t: cy - 16.0, r: pill.l + 39.0, b: cy + 16.0 }, 16.0, 16.0, pal.rest, a);
    let words = |dy: f32| RectF { l: pill.l + 50.0, t: cy + dy, r: pill.r - 54.0, b: cy + dy + 18.0 };
    r.text(&t.title, F::Seg, words(-19.0).d2d(), color(pal.text, a));
    let sub = if t.artist.is_empty() { &t.source } else { &t.artist };
    r.text(sub, F::Hint, words(0.0).d2d(), color(pal.muted, a));
    mini_bars(r, m, pill.r - 36.0, cy, a);
}

// ---------------------------------------------------------------- the Media tab

pub fn draw(r: &Renderer, v: &View, alpha: f32) {
    let p = exp();
    let cy = p.t + FOOTER_CY;
    pill::hline(r, p.t + FOOTER_SEP, alpha);
    let note = match &v.media {
        Some(m) => {
            draw_track(r, v, m, alpha);
            match (m.bars_on, m.track.playing) {
                (false, _) => "Visualizer is off",
                (true, true) => "Visualizer runs only while audio plays",
                (true, false) => "Paused: no audio is sampled",
            }
        }
        None => {
            draw_empty(r, alpha);
            "No audio is sampled right now"
        }
    };
    r.text(note, F::Hint, RectF { l: p.l + 20.0, t: cy - 9.0, r: p.l + 300.0, b: cy + 9.0 }.d2d(), color(r.pal.muted, alpha));
    pill::collapse_hint(r, cy, alpha);
}

fn draw_empty(r: &Renderer, alpha: f32) {
    let pal = r.pal;
    let b = at(13.0, 58.0, 414.0, 346.0);
    r.fill_rrect(b.d2d(), 22.0, color(pal.field, alpha));
    let (cx, cy) = (b.cx(), b.t + 108.0);
    r.fill_circle(cx, cy, 31.0, color(mix(pal.body, pal.media, 0.2), alpha));
    note(r, cx, cy, 30.0, color(pal.media, alpha));
    let line = |s: &str, f: F, dy: f32, c: u32| {
        // 16 either side: the title's descenders need more than the small lines do
        r.text(s, f, RectF { l: b.l, t: cy + dy - 16.0, r: b.r, b: cy + dy + 16.0 }.d2d(), color(c, alpha));
    };
    line("Nothing playing", F::Head, 58.0, pal.text);
    line("Start music or a video in any app or browser", F::Center, 90.0, pal.muted);
    line("tab. It shows up here and in the pill.", F::Center, 108.0, pal.muted);
    line("Works with most browsers, music apps and video", F::Center, 138.0, pal.subtle);
    line("players that report to Windows.", F::Center, 156.0, pal.subtle);
}

fn draw_track(r: &Renderer, v: &View, m: &MediaView, alpha: f32) {
    let (pal, t) = (r.pal, m.track);
    let hov = |h: Hit| v.hover == h;

    // the cover and who is playing what
    let cover = art_box();
    art(r, cover, 18.0, 44.0, pal.body, alpha);
    let (x, p) = (cover.r + 17.0, exp());
    let row = |dy: f32, h: f32| RectF { l: x, t: p.t + dy - h / 2.0, r: p.r - 23.0, b: p.t + dy + h / 2.0 };
    let dot = if t.playing { pal.media } else { pal.muted };
    r.fill_circle(x + 3.5, p.t + 85.0, 3.5, color(dot, alpha));
    r.text(&t.status_line(), F::Tab, RectF { l: x + 14.0, ..row(85.0, 20.0) }.d2d(), color(pal.muted, alpha));
    r.text(&t.title, F::Big, row(110.0, 30.0).d2d(), color(pal.text, alpha));
    r.text(&t.artist, F::Title, row(136.0, 22.0).d2d(), color(pal.text2, alpha));

    // the visualizer
    let w = wave_box();
    r.fill_rrect(w.d2d(), 16.0, color(pal.field, alpha));
    r.stroke_rrect(w.inset(0.5).d2d(), 15.5, color(pal.rule, alpha), 1.0);
    if m.bars_on {
        let inner = RectF { l: w.l + 16.0, r: w.r - 16.0, t: w.t + 7.0, b: w.b - 7.0 };
        let pitch = inner.w() / BANDS as f32;
        for (i, level) in m.levels.iter().enumerate() {
            // never quite flat, so a quiet moment still shows the row of bars
            let h = 5.0 + level * (inner.h() - 5.0);
            let l = inner.l + i as f32 * pitch + (pitch - 5.0) / 2.0;
            r.fill_rrect(
                RectF { l, t: w.cy() - h / 2.0, r: l + 5.0, b: w.cy() + h / 2.0 }.d2d(),
                2.5,
                color(pal.media, alpha * (0.5 + 0.5 * level)),
            );
        }
    } else {
        r.text("Visualizer off", F::Center, w.d2d(), color(pal.faint, alpha));
    }

    // where it is in the track
    let s = seek_bar();
    let (l, rr, cy) = (s.l + KNOB_R, s.r - KNOB_R, s.cy());
    let pos = t.position();
    let times = RectF { t: s.b + 2.0, b: s.b + 22.0, ..s };
    r.text(&mmss(pos), F::Hint, times.d2d(), color(pal.muted, alpha));
    // the line is always there: a player (Zen) names a new track seconds before it gives the length
    r.fill_rrect(RectF { l, t: cy - 2.0, r: rr, b: cy + 2.0 }.d2d(), 2.0, color(pal.tile, alpha));
    if t.len > 0.0 {
        let f = (pos / t.len).clamp(0.0, 1.0) as f32;
        let knob = l + (rr - l) * f;
        r.fill_rrect(RectF { l, t: cy - 2.0, r: knob, b: cy + 2.0 }.d2d(), 2.0, color(pal.media, alpha));
        r.fill_circle(knob, cy, if hov(Hit::MediaSeek) { KNOB_R + 1.0 } else { KNOB_R }, color(pal.text, alpha));
        let left = format!("-{}", mmss(t.len - pos));
        let lw = r.measure(F::Hint, &left);
        r.text(&left, F::Hint, RectF { l: s.r - lw - 2.0, ..times }.d2d(), color(pal.muted, alpha));
    }

    // the buttons
    for (c, radius, hit, glyph, size) in [
        (prev_c(), SKIP_R, Hit::MediaPrev, '\u{E892}', F::Icon16),
        (next_c(), SKIP_R, Hit::MediaNext, '\u{E893}', F::Icon16),
    ] {
        r.fill_circle(c.0, c.1, radius, color(lit(r, pal.tile, hov(hit)), alpha));
        r.glyph(glyph, size, c.0, c.1, color(pal.text, alpha));
    }
    let c = play_c();
    r.fill_circle(c.0, c.1, PLAY_R, color(lit(r, pal.media, hov(Hit::MediaPlay)), alpha));
    r.glyph(if t.playing { '\u{F8AE}' } else { '\u{F5B0}' }, F::Icon24, c.0, c.1, color(pal.ink, alpha));

    // the visualizer switch
    let b = bars_btn();
    r.fill_rrect(b.d2d(), b.h() / 2.0, color(lit(r, pal.tile, hov(Hit::MediaStyle)), alpha));
    for (i, h) in [6.0, 12.0, 8.0].into_iter().enumerate() {
        let l = b.l + 15.0 + i as f32 * 4.5;
        let fg = if m.bars_on { pal.media } else { pal.faint };
        r.fill_rrect(RectF { l, t: b.cy() - h / 2.0, r: l + 2.4, b: b.cy() + h / 2.0 }.d2d(), 1.2, color(fg, alpha));
    }
    r.text(if m.bars_on { "Bars" } else { "Off" }, F::Seg, RectF { l: b.l + 34.0, ..b }.d2d(), color(pal.text, alpha));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_control_is_under_its_point_and_the_empty_tab_has_none() {
        let c = |p: (f32, f32)| p;
        assert_eq!(hit(c(play_c()).0, c(play_c()).1, true, true), Some(Hit::MediaPlay));
        assert_eq!(hit(prev_c().0, prev_c().1, true, true), Some(Hit::MediaPrev));
        assert_eq!(hit(next_c().0, next_c().1, true, true), Some(Hit::MediaNext));
        assert_eq!(hit(bars_btn().cx(), bars_btn().cy(), true, true), Some(Hit::MediaStyle));
        assert_eq!(hit(seek_bar().cx(), seek_bar().cy(), true, true), Some(Hit::MediaSeek));
        assert_eq!(hit(seek_bar().cx(), seek_bar().cy(), true, false), None, "a player that cannot seek has no bar");
        assert_eq!(hit(play_c().0, play_c().1, false, false), None, "nothing playing: nothing to press");
        // the round buttons are round: a corner of the box around one is not a hit
        assert_eq!(hit(play_c().0 + PLAY_R * 0.9, play_c().1 + PLAY_R * 0.9, true, true), None);
    }

    #[test]
    fn the_controls_sit_apart_and_inside_the_panel() {
        let (p, q, n) = (prev_c(), play_c(), next_c());
        assert!(q.0 - p.0 > PLAY_R + SKIP_R && n.0 - q.0 > PLAY_R + SKIP_R, "the buttons do not touch");
        assert!(bars_btn().r < p.0 - SKIP_R, "the switch clears the first button");
        let e = exp();
        for r in [art_box(), wave_box(), seek_bar(), bars_btn()] {
            assert!(r.l >= e.l && r.r <= e.r && r.b < e.t + FOOTER_SEP, "{r:?} is inside the panel above the footer");
        }
        assert!(q.1 + PLAY_R < e.t + FOOTER_SEP, "the play button clears the footer rule");
        assert!(art_box().b < wave_box().t && wave_box().b < seek_bar().t && seek_bar().b + 22.0 < q.1 - PLAY_R);
    }

    #[test]
    fn a_click_on_the_bar_seeks_to_that_fraction_and_the_ends_clamp() {
        let s = seek_bar();
        assert_eq!(seek_frac(s.l - 50.0), 0.0);
        assert_eq!(seek_frac(s.r + 50.0), 1.0);
        assert!((seek_frac(s.cx()) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn times_read_as_clocks() {
        assert_eq!(mmss(0.0), "0:00");
        assert_eq!(mmss(61.0), "1:01");
        assert_eq!(mmss(130.9), "2:10");
        assert_eq!(mmss(3661.0), "1:01:01");
        assert_eq!(mmss(-4.0), "0:00");
    }
}
