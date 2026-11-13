//! Copy peek: when something is copied, the collapsed pill briefly widens into a capsule that
//! says "Copied" and shows what it was, then shrinks back.
//!
//! This module is the pure part (state, timing, sizes, opacities) so it can be unit tested.
//! `app.rs` drives it with the existing springs and one one-shot timer, and `ui/pill.rs`
//! draws it. Nothing runs while you are not copying.
//!
//! Motion, in order:
//! * **In**: the width springs out. The badge pops in and its check mark draws itself, then
//!   "Copied" and the preview slide out from behind the badge as they fade in (each on its own
//!   short timer, so it reads as a sequence rather than one fade).
//! * **Hold**: about 2 s, longer while the cursor is on it.
//! * **Out**: nothing waits for anything else. The text fades quickly while the capsule is
//!   already easing back with a softer spring, and the badge shrinks away with it.
//! * **Copy again while it is up**: the old preview cross-fades into the new one while the width
//!   springs on to the new size; the capsule is not replayed.
//!
//! With Windows animation effects off, nothing moves: the capsule cross-fades instead.

use crate::store::make_preview;
use crate::ui::{COLLAPSED_W, PEEK_MAX_W, PEEK_MIN_W};
use std::time::Instant;

/// Reduced motion: the capsule fades out over this long when it leaves.
pub const FADE_OUT_MS: f32 = 130.0;
/// Reduced motion: the capsule cross-fades in over this long.
pub const FADE_IN_MS: f32 = 140.0;

/// Leaving: the text is gone after this long (it fades out fast at first, so the right-hand clip
/// that comes in with the shrinking capsule never slices through visible text), the badge after
/// `LEAVE_ICON_MS`.
pub const LEAVE_TEXT_MS: f32 = 70.0;
pub const LEAVE_ICON_MS: f32 = 120.0;
/// The badge shrinks to this fraction of its size as it leaves.
const LEAVE_POP: f32 = 0.7;

/// Entrance: the badge pops from `POP_FROM` to full size (with a hint of overshoot) over `POP_MS`,
/// the check mark draws itself from `CHECK_FROM_MS` to `CHECK_TO_MS`, and the text slides out
/// from behind the badge by `SLIDE_PX` between `SLIDE_FROM_MS` and `SLIDE_FROM_MS + SLIDE_MS`.
const POP_FROM: f32 = 0.6;
const POP_MS: f32 = 250.0;
const CHECK_FROM_MS: f32 = 40.0;
const CHECK_TO_MS: f32 = 210.0;
const SLIDE_PX: f32 = 10.0;
const SLIDE_FROM_MS: f32 = 40.0;
const SLIDE_MS: f32 = 260.0;
/// "Copied" fades in, then the preview (which is also held back until the capsule is wide enough
/// for it, see `TEXT_FROM`): badge, then label, then preview.
const LABEL_FROM_MS: f32 = 50.0;
const LABEL_TO_MS: f32 = 170.0;
const LINE_FROM_MS: f32 = 90.0;
const LINE_TO_MS: f32 = 230.0;
/// Everything above is over by then, so the frame loop can go back to sleep.
const ENTER_MS: f32 = 320.0;

/// Copying again while the capsule is up: the old preview drops out quickly (over `SWAP_OUT_MS`),
/// and only once it is gone the new one fades in (`SWAP_IN_FROM`..`SWAP_IN_TO`): two lines of text
/// at half strength on top of each other just look garbled. Positions blend over `SHIFT_MS`.
const SWAP_OUT_MS: f32 = 80.0;
const SWAP_IN_FROM: f32 = 90.0;
const SWAP_IN_TO: f32 = 220.0;
const SHIFT_MS: f32 = 180.0;
const SWAP_BUSY_MS: f32 = 230.0;

/// The preview is revealed once the capsule is this far (0..1) from the resting width to its own.
const TEXT_FROM: f32 = 0.45;

/// Width of the leading slot (check badge or image tile) and the gaps around the text.
pub const LEAD: f32 = 38.0;
const PAD_L: f32 = 4.0;
const PAD_R: f32 = 18.0;
const GAP_LEAD: f32 = 10.0;
const GAP_TEXT: f32 = 12.0;
/// Spare room after the preview text. A text box exactly as wide as its text is trimmed with an
/// ellipsis or not depending on float rounding, which flickered while the width settled.
const TEXT_SLACK: f32 = 4.0;
/// A copied text is cut to this many characters before measuring; the capsule trims it with
/// an ellipsis to whatever width is left.
const MAX_LINE_CHARS: usize = 80;

/// What was just copied.
#[derive(Clone, Copy, Debug)]
pub enum Copied<'a> {
    Text(&'a str),
    Image { id: u64, w: u32, h: u32 },
}

/// What the capsule shows, and how wide it needs to be for that.
#[derive(Clone, Debug, PartialEq)]
pub struct Content {
    /// One line after "Copied". Empty when the preview is switched off.
    pub line: String,
    /// Clip id whose thumbnail fills the leading tile instead of the check badge (images only).
    pub thumb: Option<u64>,
    pub target_w: f32,
    /// Spare width on a short copy, shared evenly left and right of the content.
    pub slack: f32,
}

/// Capsule width for a label and a preview of the given widths, and the slack left over.
pub fn width(label_w: f32, text_w: f32, has_text: bool) -> (f32, f32) {
    let content = PAD_L + LEAD + GAP_LEAD + label_w + if has_text { GAP_TEXT + text_w + TEXT_SLACK } else { 0.0 } + PAD_R;
    let w = content.clamp(PEEK_MIN_W, PEEK_MAX_W);
    (w, (w - content).max(0.0))
}

/// Works out the line to show and the capsule width. `measure` returns the width of a string in
/// the preview font. With `preview` off only "Copied" is shown (nothing about the content).
pub fn compose(copied: Copied, preview: bool, label_w: f32, measure: impl Fn(&str) -> f32) -> Content {
    let (line, thumb) = if !preview {
        (String::new(), None)
    } else {
        match copied {
            Copied::Text(t) => (make_preview(t, MAX_LINE_CHARS), None),
            Copied::Image { id, w, h } => (format!("Image \u{00B7} {w} \u{00D7} {h}"), Some(id)),
        }
    };
    let has_text = !line.is_empty();
    let text_w = if has_text { measure(&line) } else { 0.0 };
    let (target_w, slack) = width(label_w, text_w, has_text);
    Content { line, thumb, target_w, slack }
}

/// Where things sit, as offsets from the capsule's left edge at its final width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub lead_x: f32,
    pub label_x: f32,
    pub text_x: f32,
    /// Room for the preview (it is trimmed with an ellipsis to fit).
    pub text_w: f32,
}

pub fn layout(target_w: f32, slack: f32, label_w: f32) -> Layout {
    let lead_x = PAD_L + slack / 2.0;
    let label_x = lead_x + LEAD + GAP_LEAD;
    let text_x = label_x + label_w + GAP_TEXT;
    let text_w = (target_w - slack / 2.0 - PAD_R - text_x).max(0.0);
    Layout { lead_x, label_x, text_x, text_w }
}

/// Blends two layouts (`t` = 0 gives `a`, 1 gives `b`): the badge and label glide to where the
/// new content wants them when a copy replaces another.
pub fn lerp_layout(a: Layout, b: Layout, t: f32) -> Layout {
    Layout {
        lead_x: lerp(a.lead_x, b.lead_x, t),
        label_x: lerp(a.label_x, b.label_x, t),
        text_x: lerp(a.text_x, b.text_x, t),
        text_w: lerp(a.text_w, b.text_w, t),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// Showing (widening, then holding).
    Hold,
    /// On its way out since this instant: the text fades while the width eases back.
    Leaving(Instant),
}

/// What `Peek::advance` wants the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Advance {
    Nothing,
    /// Reduced motion: the fade-out is over; drop the peek and snap back.
    Finished,
}

/// How opaque / how far along each part of the capsule is right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// The capsule's own body (0..=1). Below 1 only with reduced motion (cross-fade over the pill).
    pub body: f32,
    /// The check badge / image tile.
    pub icon: f32,
    /// The previous badge while it cross-fades out (only when the kind of badge changed).
    pub old_icon: f32,
    /// "Copied".
    pub label: f32,
    /// The preview line.
    pub line: f32,
    /// The previous preview line while it cross-fades out.
    pub old_line: f32,
    /// Badge scale (about 0.6..1.03; below 1 while it pops in or leaves).
    pub pop: f32,
    /// How much of the check mark is drawn (0..=1).
    pub check: f32,
    /// Horizontal offset in DIPs of "Copied" and the preview (<= 0, easing to 0 as they slide in).
    pub slide: f32,
    /// 0 = positions of the previous content, 1 = positions of the current one.
    pub shift: f32,
}

#[derive(Clone, Debug)]
pub struct Peek {
    pub content: Content,
    /// What the capsule showed before the last copy, while it cross-fades out.
    pub prev: Option<Content>,
    /// When the content last changed (`None` for a peek that has not been swapped).
    pub swapped: Option<Instant>,
    pub phase: Phase,
    /// Windows animation effects are off: cross-fade instead of widening.
    pub reduced: bool,
    pub shown: Instant,
}

fn ms(from: Instant, to: Instant) -> f32 {
    to.saturating_duration_since(from).as_secs_f32() * 1000.0
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Rises to a hint above 1 and settles on it (0 -> 0, 1 -> 1).
fn ease_out_back(t: f32) -> f32 {
    const C1: f32 = 1.4;
    let u = t - 1.0;
    1.0 + (C1 + 1.0) * u * u * u + C1 * u * u
}

fn ease_out_cubic(t: f32) -> f32 {
    let u = 1.0 - t;
    1.0 - u * u * u
}

/// Fraction of the way from the resting width to `target_w`, for current width `w`.
fn progress_of(target_w: f32, w: f32) -> f32 {
    let span = (target_w - COLLAPSED_W).max(1.0);
    ((w - COLLAPSED_W) / span).clamp(0.0, 1.0)
}

impl Peek {
    pub fn new(content: Content, reduced: bool, now: Instant) -> Peek {
        Peek { content, prev: None, swapped: None, phase: Phase::Hold, reduced, shown: now }
    }

    /// Another copy while the capsule is up: show the new content and hold again. The capsule
    /// is not replayed (`shown` stays), so the widen does not restart.
    ///
    /// The same content again (most apps report one copy twice) changes nothing. While it is
    /// showing, the old content cross-fades into the new; if it was already leaving, the old
    /// content is as good as gone and the new one just fades in.
    pub fn swap(&mut self, content: Content, now: Instant) {
        let leaving = self.phase != Phase::Hold;
        self.phase = Phase::Hold;
        if content == self.content {
            if leaving {
                self.prev = None;
                self.swapped = Some(now);
            }
            return;
        }
        let old = std::mem::replace(&mut self.content, content);
        self.prev = if leaving { None } else { Some(old) };
        self.swapped = Some(now);
    }

    /// The capsule is opening or open (it uses the springy opening spring); false once it is
    /// on its way back (it uses the softer leaving spring).
    pub fn growing(&self) -> bool {
        self.phase == Phase::Hold
    }

    /// Starts leaving: the text starts fading while the width eases back.
    pub fn begin_leave(&mut self, now: Instant) {
        if self.phase == Phase::Hold {
            self.phase = Phase::Leaving(now);
        }
    }

    /// True while something moves that the springs do not drive (a fade, the badge's pop), so
    /// the frame loop has to keep running.
    pub fn busy(&self, now: Instant) -> bool {
        match self.phase {
            Phase::Hold => {
                let entering = ms(self.shown, now) < if self.reduced { FADE_IN_MS } else { ENTER_MS };
                entering || self.swapped.is_some_and(|t| ms(t, now) < SWAP_BUSY_MS)
            }
            // reduced motion: `advance` ends it; otherwise it is over once the badge is gone
            Phase::Leaving(t0) => self.reduced || ms(t0, now) < LEAVE_ICON_MS,
        }
    }

    pub fn advance(&mut self, now: Instant) -> Advance {
        match self.phase {
            Phase::Leaving(t0) if self.reduced && ms(t0, now) >= FADE_OUT_MS => Advance::Finished,
            _ => Advance::Nothing,
        }
    }

    pub fn look(&self, w: f32, now: Instant) -> Look {
        let e = ms(self.shown, now);
        let (t_leave, leaving) = match self.phase {
            Phase::Hold => (0.0, false),
            Phase::Leaving(t0) => (ms(t0, now), true),
        };

        // how far the previous content has been replaced by the current one
        let (s_in, s_out, shift) = match self.swapped {
            Some(t) => {
                let d = ms(t, now);
                (
                    smoothstep(SWAP_IN_FROM, SWAP_IN_TO, d),
                    1.0 - ease_out_cubic((d / SWAP_OUT_MS).clamp(0.0, 1.0)),
                    smoothstep(0.0, SHIFT_MS, d),
                )
            }
            None => (1.0, 0.0, 1.0),
        };
        // a different kind of badge (check <-> picture) cross-fades; the same kind stays put
        let kind_changed = self.prev.as_ref().is_some_and(|o| o.thumb != self.content.thumb);

        if self.reduced {
            // no widening or moving: the whole capsule fades in, holds, fades out
            let leave = if leaving { 1.0 - (t_leave / FADE_OUT_MS).clamp(0.0, 1.0) } else { 1.0 };
            let f = (e / FADE_IN_MS).clamp(0.0, 1.0).min(leave);
            return Look {
                body: f,
                icon: if kind_changed { f * s_in } else { f },
                old_icon: if kind_changed { f * s_out } else { 0.0 },
                label: f,
                line: f * s_in,
                old_line: if self.prev.is_some() { f * s_out } else { 0.0 },
                pop: 1.0,
                check: 1.0,
                slide: 0.0,
                shift: 1.0,
            };
        }

        let text_out = if leaving { 1.0 - ease_out_cubic((t_leave / LEAVE_TEXT_MS).clamp(0.0, 1.0)) } else { 1.0 };
        let icon_out = if leaving { 1.0 - smoothstep(0.0, LEAVE_ICON_MS, t_leave) } else { 1.0 };
        // The preview is revealed as the capsule gets wide enough for *its* text (and goes again as
        // it narrows). The badge and "Copied" are not tied to the content's width: they are the
        // same for every copy, so a longer copy replacing a shorter one must not make them dip.
        let reveal = |target_w: f32| smoothstep(TEXT_FROM, 1.0, progress_of(target_w, w)) * text_out;
        let label = smoothstep(LABEL_FROM_MS, LABEL_TO_MS, e) * text_out;
        let line_in = smoothstep(LINE_FROM_MS, LINE_TO_MS, e);
        // up as soon as the capsule has grown a few pixels
        let icon = smoothstep(0.0, 0.3, progress_of(PEEK_MIN_W, w)) * icon_out;

        let pop_in = lerp(POP_FROM, 1.0, ease_out_back((e / POP_MS).clamp(0.0, 1.0)));
        let pop_out = if leaving { lerp(1.0, LEAVE_POP, smoothstep(0.0, LEAVE_ICON_MS, t_leave)) } else { 1.0 };

        Look {
            body: 1.0,
            icon: if kind_changed { icon * s_in } else { icon },
            old_icon: if kind_changed { icon * s_out } else { 0.0 },
            label,
            line: reveal(self.content.target_w) * line_in * s_in,
            old_line: self.prev.as_ref().map_or(0.0, |o| reveal(o.target_w) * s_out),
            pop: pop_in * pop_out,
            check: smoothstep(CHECK_FROM_MS, CHECK_TO_MS, e),
            slide: -SLIDE_PX * (1.0 - ease_out_cubic(((e - SLIDE_FROM_MS) / SLIDE_MS).clamp(0.0, 1.0))),
            shift,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn content(w: f32) -> Content {
        Content { line: "Call at 4 pm".into(), thumb: None, target_w: w, slack: 0.0 }
    }

    fn at(t: Instant, ms: u64) -> Instant {
        t + Duration::from_millis(ms)
    }

    #[test]
    fn short_copies_get_the_minimum_width_with_slack() {
        let (w, slack) = width(50.0, 10.0, true);
        assert_eq!(w, PEEK_MIN_W);
        assert!(slack > 0.0);
        let (w, slack) = width(50.0, 0.0, false);
        assert_eq!(w, PEEK_MIN_W);
        assert!(slack > 0.0);
    }

    #[test]
    fn long_copies_stop_at_the_maximum_width() {
        let (w, slack) = width(50.0, 900.0, true);
        assert_eq!(w, PEEK_MAX_W);
        assert_eq!(slack, 0.0);
    }

    #[test]
    fn medium_copies_fit_with_a_little_room_to_spare() {
        let text = 100.0;
        let (w, slack) = width(50.0, text, true);
        assert!(w > PEEK_MIN_W && w < PEEK_MAX_W, "{w}");
        assert_eq!(slack, 0.0);
        assert_eq!(w, PAD_L + LEAD + GAP_LEAD + 50.0 + GAP_TEXT + text + TEXT_SLACK + PAD_R);
    }

    #[test]
    fn layout_gives_the_preview_the_room_that_is_left() {
        let (w, slack) = width(50.0, 100.0, true);
        let l = layout(w, slack, 50.0);
        // never exactly the text's width: that tips into an ellipsis on float rounding
        assert!(l.text_w >= 100.0 + TEXT_SLACK - 0.01, "{l:?}");
        // a long copy: the preview gets less than it wants, never a negative amount
        let (w, slack) = width(50.0, 900.0, true);
        let l = layout(w, slack, 50.0);
        assert!(l.text_w > 0.0 && l.text_w < 900.0);
        assert!(layout(PEEK_MIN_W, 0.0, 400.0).text_w >= 0.0);
        // content sits left-to-right and inside the capsule
        assert!(l.lead_x < l.label_x && l.label_x < l.text_x && l.text_x + l.text_w <= w);
    }

    #[test]
    fn layouts_blend_between_two_contents() {
        let a = layout(180.0, 12.0, 50.0);
        let b = layout(340.0, 0.0, 50.0);
        assert_eq!(lerp_layout(a, b, 0.0), a);
        assert_eq!(lerp_layout(a, b, 1.0), b);
        let m = lerp_layout(a, b, 0.5);
        assert!(m.lead_x > b.lead_x && m.lead_x < a.lead_x);
    }

    #[test]
    fn compose_text_collapses_whitespace_and_measures_the_line() {
        let c = compose(Copied::Text("Meeting\n   moved to\t4 pm"), true, 50.0, |s| s.chars().count() as f32 * 7.0);
        assert_eq!(c.line, "Meeting moved to 4 pm");
        assert_eq!(c.thumb, None);
        assert!(c.target_w >= PEEK_MIN_W);
    }

    #[test]
    fn compose_long_text_is_cut_and_capped() {
        let long = "word ".repeat(200);
        let c = compose(Copied::Text(&long), true, 50.0, |s| s.chars().count() as f32 * 7.0);
        assert!(c.line.chars().count() <= MAX_LINE_CHARS);
        assert_eq!(c.target_w, PEEK_MAX_W);
    }

    #[test]
    fn compose_image_shows_size_and_thumbnail() {
        let c = compose(Copied::Image { id: 7, w: 1280, h: 720 }, true, 50.0, |s| s.chars().count() as f32 * 7.0);
        assert_eq!(c.line, "Image \u{00B7} 1280 \u{00D7} 720");
        assert_eq!(c.thumb, Some(7));
    }

    #[test]
    fn privacy_setting_shows_only_copied() {
        let t = compose(Copied::Text("hunter2"), false, 50.0, |_| 999.0);
        assert_eq!((t.line.as_str(), t.thumb), ("", None));
        let i = compose(Copied::Image { id: 7, w: 10, h: 10 }, false, 50.0, |_| 999.0);
        assert_eq!((i.line.as_str(), i.thumb), ("", None));
        assert_eq!(t.target_w, PEEK_MIN_W, "no preview, so no room for one");
    }

    #[test]
    fn icon_leads_and_the_preview_follows_the_widening() {
        let t0 = Instant::now();
        let p = Peek::new(content(300.0), false, t0);
        let later = at(t0, ENTER_MS as u64 + 100); // past the timed part of the entrance
        let rest = p.look(COLLAPSED_W, later);
        assert_eq!((rest.icon, rest.line, rest.body), (0.0, 0.0, 1.0));
        // a third of the way: the badge is up, the preview is not
        let mid = p.look(COLLAPSED_W + (300.0 - COLLAPSED_W) * 0.35, later);
        assert!(mid.icon > 0.9 && mid.line == 0.0, "{mid:?}");
        let full = p.look(300.0, later);
        assert_eq!((full.icon, full.label, full.line), (1.0, 1.0, 1.0));
        // monotonic while widening
        let mut last = (0.0f32, 0.0f32);
        for i in 0..=20 {
            let l = p.look(COLLAPSED_W + (300.0 - COLLAPSED_W) * i as f32 / 20.0, later);
            assert!(l.icon >= last.0 && l.line >= last.1);
            last = (l.icon, l.line);
        }
    }

    #[test]
    fn entrance_is_a_sequence_badge_then_label_then_preview() {
        let t0 = Instant::now();
        let p = Peek::new(content(300.0), false, t0);
        // even at full width, nothing but the badge is there at first
        let first = p.look(300.0, t0);
        assert_eq!((first.label, first.line), (0.0, 0.0));
        let (mut label_at, mut line_at) = (None, None);
        for i in 0..=ENTER_MS as u64 {
            let l = p.look(300.0, at(t0, i));
            if label_at.is_none() && l.label > 0.5 {
                label_at = Some(i);
            }
            if line_at.is_none() && l.line > 0.5 {
                line_at = Some(i);
            }
        }
        let (label_at, line_at) = (label_at.unwrap(), line_at.unwrap());
        assert!(label_at < line_at, "label at {label_at} ms, preview at {line_at} ms");
        let end = p.look(300.0, at(t0, ENTER_MS as u64));
        assert_eq!((end.label, end.line), (1.0, 1.0));
    }

    #[test]
    fn entrance_pops_the_badge_draws_the_check_and_slides_the_text() {
        let t0 = Instant::now();
        let p = Peek::new(content(300.0), false, t0);
        let start = p.look(300.0, t0);
        assert!((start.pop - POP_FROM).abs() < 1e-3, "{start:?}");
        assert_eq!(start.check, 0.0);
        assert!(start.slide <= -SLIDE_PX * 0.9, "text starts tucked behind the badge: {start:?}");
        // the pop never grows wildly past full size and ends exactly on it
        let mut peak = 0.0f32;
        let (mut last_check, mut last_slide) = (0.0f32, start.slide);
        for i in 0..=ENTER_MS as u64 {
            let l = p.look(300.0, at(t0, i));
            peak = peak.max(l.pop);
            assert!(l.pop >= POP_FROM - 1e-3 && l.pop < 1.06, "{l:?}");
            assert!(l.check >= last_check && l.slide >= last_slide, "check and slide only move forwards");
            (last_check, last_slide) = (l.check, l.slide);
        }
        assert!(peak > 1.0, "a hint of overshoot");
        let end = p.look(300.0, at(t0, ENTER_MS as u64));
        assert_eq!((end.pop, end.check, end.slide), (1.0, 1.0, 0.0));
        // everything that moves on its own timer is done before the frame loop is let go
        assert!(POP_MS <= ENTER_MS && CHECK_TO_MS <= ENTER_MS && SLIDE_FROM_MS + SLIDE_MS <= ENTER_MS);
        assert!(p.busy(at(t0, 100)), "the pop needs the frame loop even at full width");
        assert!(!p.busy(at(t0, ENTER_MS as u64 + 1)));
    }

    #[test]
    fn leaving_fades_the_text_while_the_width_is_already_moving() {
        let t0 = Instant::now();
        let mut p = Peek::new(content(300.0), false, t0);
        p.begin_leave(at(t0, 2000));
        let now = |d| at(t0, 2000 + d);
        assert_eq!(p.advance(now(10)), Advance::Nothing, "there is no separate fade step to wait for");
        assert!(!p.growing(), "leaving uses its own, softer spring");
        assert!(p.busy(now(10)), "the fades need the frame loop");
        // the text starts going at once, and is mostly gone before the capsule has narrowed much;
        // the badge is still up
        let early = p.look(300.0, now(20));
        assert!(early.line > 0.2 && early.line < 0.8, "{early:?}");
        assert!(early.icon > 0.8, "{early:?}");
        let later = p.look(300.0, now(45));
        assert!(later.line < 0.1, "{later:?}");
        // the text is gone even though the capsule has barely moved
        let gone = p.look(300.0, now(LEAVE_TEXT_MS as u64 + 5));
        assert_eq!((gone.label, gone.line), (0.0, 0.0));
        // the badge shrinks and fades with the capsule
        let mid = p.look(220.0, now(100));
        assert!(mid.pop < 1.0 && mid.icon < 1.0, "{mid:?}");
        assert!(!p.busy(now(LEAVE_ICON_MS as u64 + 5)), "the springs drive the rest");
        // back at the resting width nothing is left
        let rest = p.look(COLLAPSED_W, now(400));
        assert_eq!((rest.icon, rest.label, rest.line), (0.0, 0.0, 0.0));
    }

    #[test]
    fn copying_the_same_thing_twice_changes_nothing() {
        let t0 = Instant::now();
        let mut p = Peek::new(content(300.0), false, t0);
        p.swap(content(300.0), at(t0, 8)); // the second clipboard event for one copy
        assert_eq!(p.phase, Phase::Hold);
        assert!(p.prev.is_none() && p.swapped.is_none(), "nothing to cross-fade");
        let l = p.look(300.0, at(t0, 500));
        assert_eq!((l.line, l.old_line), (1.0, 0.0));
    }

    #[test]
    fn copying_while_showing_cross_fades_old_into_new() {
        let t0 = Instant::now();
        let mut p = Peek::new(content(180.0), false, t0);
        let mut next = content(340.0);
        next.line = "Another, longer one".into();
        p.swap(next.clone(), at(t0, 1000));
        assert_eq!(p.phase, Phase::Hold);
        assert_eq!(p.content, next);
        assert_eq!(p.prev.as_ref().map(|c| c.target_w), Some(180.0));
        assert_eq!(p.shown, t0, "the widen is not replayed");
        assert!(p.growing());
        let now = |d| at(t0, 1000 + d);
        // at the instant of the copy the old preview is still fully there and the new one is not
        let a = p.look(180.0, now(0));
        assert_eq!((a.old_line, a.line, a.shift), (1.0, 0.0, 0.0), "{a:?}");
        assert_eq!(a.icon, 1.0, "same kind of badge: it stays put");
        assert_eq!(a.old_icon, 0.0);
        assert!(p.busy(now(50)));
        // later: the old one is gone, the new one is in (once the width has caught up)
        let b = p.look(340.0, now(300));
        assert_eq!((b.old_line, b.line, b.shift), (0.0, 1.0, 1.0), "{b:?}");
        assert!(!p.busy(now(SWAP_BUSY_MS as u64 + 5)));
        // two previews on top of each other just look garbled: the old one is gone before the new
        // one starts, so there is never a moment with both showing
        for d in 0..300 {
            let l = p.look(340.0, now(d));
            assert!(l.old_line.min(l.line) < 0.02, "{d} ms: {l:?}");
        }
    }

    #[test]
    fn a_longer_copy_replacing_a_shorter_one_never_dips_the_badge_or_the_label() {
        // The capsule is 180 wide; the new copy wants 340. The badge and "Copied" are the same for
        // every copy, so they must stay at full strength while the width catches up (they used to
        // dip because they were measured against the new, wider target).
        let t0 = Instant::now();
        let mut p = Peek::new(content(180.0), false, t0);
        let mut next = content(340.0);
        next.line = "A much longer line of text".into();
        p.swap(next, at(t0, 1000));
        for d in 0..400u64 {
            // the width eases from 180 to 340 over the 400 ms
            let w = 180.0 + 160.0 * (d as f32 / 400.0);
            let l = p.look(w, at(t0, 1000 + d));
            assert_eq!((l.icon, l.label), (1.0, 1.0), "{d} ms at w={w}: {l:?}");
        }
        // and a shorter copy replacing a long one: the width shrinks, the badge stays
        let mut q = Peek::new(content(340.0), false, t0);
        q.swap(content(180.0), at(t0, 1000));
        for d in 0..400u64 {
            let w = 340.0 - 160.0 * (d as f32 / 400.0);
            let l = q.look(w, at(t0, 1000 + d));
            assert_eq!((l.icon, l.label), (1.0, 1.0), "{d} ms at w={w}: {l:?}");
        }
    }

    #[test]
    fn a_new_kind_of_badge_cross_fades_too() {
        let t0 = Instant::now();
        let mut p = Peek::new(content(300.0), false, t0);
        let mut img = content(300.0);
        img.line = "Image \u{00B7} 10 \u{00D7} 10".into();
        img.thumb = Some(3);
        p.swap(img, at(t0, 1000));
        let a = p.look(300.0, at(t0, 1000));
        assert_eq!((a.icon, a.old_icon), (0.0, 1.0), "{a:?}");
        let b = p.look(300.0, at(t0, 1300));
        assert_eq!((b.icon, b.old_icon), (1.0, 0.0), "{b:?}");
    }

    #[test]
    fn copying_while_leaving_brings_the_capsule_back_with_the_new_content_fading_in() {
        let t0 = Instant::now();
        let mut p = Peek::new(content(300.0), false, t0);
        p.begin_leave(at(t0, 2000));
        let mut next = content(220.0);
        next.line = "Another".into();
        p.swap(next.clone(), at(t0, 2040));
        assert_eq!(p.phase, Phase::Hold);
        assert_eq!(p.content, next);
        assert!(p.prev.is_none(), "the old content was already on its way out");
        assert_eq!(p.shown, t0, "not replayed");
        assert!(p.growing());
        let start = p.look(260.0, at(t0, 2040));
        assert_eq!(start.line, 0.0, "the new preview fades in rather than popping: {start:?}");
        let later = p.look(220.0, at(t0, 2400));
        assert_eq!(later.line, 1.0);
        // the same content again while leaving also fades back in rather than popping
        let mut q = Peek::new(content(300.0), false, t0);
        q.begin_leave(at(t0, 2000));
        q.swap(content(300.0), at(t0, 2040));
        assert_eq!(q.phase, Phase::Hold);
        assert_eq!(q.look(300.0, at(t0, 2040)).line, 0.0);
    }

    #[test]
    fn begin_leave_only_starts_from_hold() {
        let t0 = Instant::now();
        let mut p = Peek::new(content(300.0), false, t0);
        p.begin_leave(t0);
        let first = p.phase;
        p.begin_leave(at(t0, 100)); // a stray second call must not restart the fade
        assert_eq!(p.phase, first);
    }

    #[test]
    fn reduced_motion_cross_fades_the_whole_capsule_without_moving_anything() {
        let t0 = Instant::now();
        let mut p = Peek::new(content(300.0), true, t0);
        // already at full size, fading in
        assert_eq!(progress_of(p.content.target_w, 300.0), 1.0);
        let start = p.look(300.0, t0);
        assert_eq!((start.body, start.icon, start.label, start.line), (0.0, 0.0, 0.0, 0.0));
        assert!(p.busy(at(t0, 50)));
        let mid = p.look(300.0, at(t0, 70));
        assert!(mid.body > 0.3 && mid.body < 0.7 && mid.body == mid.line, "{mid:?}");
        assert_eq!((mid.pop, mid.check, mid.slide, mid.shift), (1.0, 1.0, 0.0, 1.0), "nothing moves");
        let held = p.look(300.0, at(t0, 1000));
        assert_eq!((held.body, held.icon, held.label, held.line), (1.0, 1.0, 1.0, 1.0));
        assert!(!p.busy(at(t0, 1000)));
        // fading out: the body goes too, then the app drops the peek
        p.begin_leave(at(t0, 2000));
        assert!(p.busy(at(t0, 2050)));
        let out = p.look(300.0, at(t0, 2065));
        assert!(out.body > 0.3 && out.body < 0.7, "{out:?}");
        assert_eq!(p.advance(at(t0, 2200)), Advance::Finished);
    }

    #[test]
    fn full_size_peek_does_not_need_the_frame_loop_while_holding() {
        let t0 = Instant::now();
        let p = Peek::new(content(300.0), false, t0);
        assert!(!p.busy(at(t0, ENTER_MS as u64 + 1)));
        assert!(!p.busy(at(t0, 1000)));
    }
}
