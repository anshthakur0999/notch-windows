//! Copy peek: when something is copied, the collapsed pill briefly widens into a capsule that
//! says "Copied" and shows what it was, then shrinks back.
//!
//! This module is the pure part (state, timing, sizes, opacities) so it can be unit tested.
//! `app.rs` drives it with the existing springs and one one-shot timer, and `ui/pill.rs`
//! draws it. Nothing runs while you are not copying.
//!
//! Sequence (matches the design): rest -> widen (spring, the icon leads, the text fades in as
//! it settles) -> hold (about 2 s, longer while hovered) -> the text fades out first, then the
//! width springs back. With Windows animation effects off, the capsule cross-fades instead.

use crate::store::make_preview;
use crate::ui::{COLLAPSED_W, PEEK_MAX_W, PEEK_MIN_W};
use std::time::Instant;

/// The text fades out over this long before the capsule shrinks (and, with reduced motion,
/// the whole capsule fades out over it).
pub const FADE_OUT_MS: f32 = 130.0;
/// Reduced motion: the capsule cross-fades in over this long.
pub const FADE_IN_MS: f32 = 140.0;

/// Width of the leading slot (check badge or image tile) and the gaps around the text.
pub const LEAD: f32 = 38.0;
const PAD_L: f32 = 4.0;
const PAD_R: f32 = 18.0;
const GAP_LEAD: f32 = 10.0;
const GAP_TEXT: f32 = 12.0;
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
    let content = PAD_L + LEAD + GAP_LEAD + label_w + if has_text { GAP_TEXT + text_w } else { 0.0 } + PAD_R;
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// Showing (widening, then holding).
    Hold,
    /// The text is fading out; the width has not moved yet.
    FadeOut(Instant),
    /// The width is springing back to the resting pill.
    Shrink,
}

/// What `Peek::advance` wants the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Advance {
    Nothing,
    /// Start springing back to the resting size.
    Shrink,
    /// Reduced motion: the fade-out is over; drop the peek and snap back.
    Finished,
}

/// How opaque each part of the capsule is right now (0..=1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// The capsule's own body. Below 1 only with reduced motion (cross-fade over the pill).
    pub body: f32,
    /// The check badge / image tile.
    pub icon: f32,
    /// "Copied" and the preview line.
    pub text: f32,
}

#[derive(Clone, Debug)]
pub struct Peek {
    pub content: Content,
    pub phase: Phase,
    /// Windows animation effects are off: cross-fade instead of widening.
    pub reduced: bool,
    pub shown: Instant,
}

fn ms(from: Instant, to: Instant) -> f32 {
    to.saturating_duration_since(from).as_secs_f32() * 1000.0
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Peek {
    pub fn new(content: Content, reduced: bool, now: Instant) -> Peek {
        Peek { content, phase: Phase::Hold, reduced, shown: now }
    }

    /// Another copy while the capsule is up: swap what it shows and hold again. The capsule
    /// is not replayed (`shown` stays), so the widen does not restart.
    pub fn swap(&mut self, content: Content) {
        self.content = content;
        self.phase = Phase::Hold;
    }

    /// The capsule is opening or open (it uses the springy opening spring); false once it is
    /// on its way back (it uses the stiff closing spring).
    pub fn growing(&self) -> bool {
        !matches!(self.phase, Phase::Shrink)
    }

    /// Starts leaving: the text fades out first.
    pub fn begin_leave(&mut self, now: Instant) {
        if self.phase == Phase::Hold {
            self.phase = Phase::FadeOut(now);
        }
    }

    /// True while something moves that the springs do not drive (a fade), so the frame loop
    /// has to keep running.
    pub fn busy(&self, now: Instant) -> bool {
        match self.phase {
            Phase::Hold => self.reduced && ms(self.shown, now) < FADE_IN_MS,
            Phase::FadeOut(_) => true,
            Phase::Shrink => false,
        }
    }

    pub fn advance(&mut self, now: Instant) -> Advance {
        if let Phase::FadeOut(t0) = self.phase {
            if ms(t0, now) >= FADE_OUT_MS {
                if self.reduced {
                    return Advance::Finished;
                }
                self.phase = Phase::Shrink;
                return Advance::Shrink;
            }
        }
        Advance::Nothing
    }

    /// Fraction of the way from the resting width to the capsule's width, for current width `w`.
    pub fn progress(&self, w: f32) -> f32 {
        let span = (self.content.target_w - COLLAPSED_W).max(1.0);
        ((w - COLLAPSED_W) / span).clamp(0.0, 1.0)
    }

    pub fn look(&self, w: f32, now: Instant) -> Look {
        let leave = match self.phase {
            Phase::Hold => 1.0,
            Phase::FadeOut(t0) => 1.0 - (ms(t0, now) / FADE_OUT_MS).clamp(0.0, 1.0),
            Phase::Shrink => 0.0,
        };
        if self.reduced {
            // no widening: the whole capsule fades in, holds, fades out
            let f = (ms(self.shown, now) / FADE_IN_MS).clamp(0.0, 1.0).min(leave);
            return Look { body: f, icon: f, text: f };
        }
        let p = self.progress(w);
        // The icon leads the widening; the text fades in as it settles. On the way out the text
        // is already gone (`leave`), and the icon follows the width back down.
        Look { body: 1.0, icon: smoothstep(0.0, 0.3, p), text: smoothstep(0.55, 1.0, p) * leave }
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
    fn medium_copies_fit_exactly() {
        let text = 100.0;
        let (w, slack) = width(50.0, text, true);
        assert!(w > PEEK_MIN_W && w < PEEK_MAX_W, "{w}");
        assert_eq!(slack, 0.0);
        assert_eq!(w, PAD_L + LEAD + GAP_LEAD + 50.0 + GAP_TEXT + text + PAD_R);
    }

    #[test]
    fn layout_gives_the_preview_the_room_that_is_left() {
        let (w, slack) = width(50.0, 100.0, true);
        let l = layout(w, slack, 50.0);
        assert!((l.text_w - 100.0).abs() < 0.01, "{l:?}");
        // a long copy: the preview gets less than it wants, never a negative amount
        let (w, slack) = width(50.0, 900.0, true);
        let l = layout(w, slack, 50.0);
        assert!(l.text_w > 0.0 && l.text_w < 900.0);
        assert!(layout(PEEK_MIN_W, 0.0, 400.0).text_w >= 0.0);
        // content sits left-to-right and inside the capsule
        assert!(l.lead_x < l.label_x && l.label_x < l.text_x && l.text_x + l.text_w <= w);
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
    fn icon_leads_and_text_follows_the_widening() {
        let now = Instant::now();
        let p = Peek::new(content(300.0), false, now);
        let rest = p.look(COLLAPSED_W, now);
        assert_eq!((rest.icon, rest.text, rest.body), (0.0, 0.0, 1.0));
        // a third of the way: the icon is up, the text is not
        let mid = p.look(COLLAPSED_W + (300.0 - COLLAPSED_W) * 0.35, now);
        assert!(mid.icon > 0.9 && mid.text == 0.0, "{mid:?}");
        let full = p.look(300.0, now);
        assert_eq!((full.icon, full.text), (1.0, 1.0));
        // monotonic while widening
        let mut last = (0.0f32, 0.0f32);
        for i in 0..=20 {
            let l = p.look(COLLAPSED_W + (300.0 - COLLAPSED_W) * i as f32 / 20.0, now);
            assert!(l.icon >= last.0 && l.text >= last.1);
            last = (l.icon, l.text);
        }
    }

    #[test]
    fn text_fades_out_before_the_width_moves() {
        let t0 = Instant::now();
        let mut p = Peek::new(content(300.0), false, t0);
        p.begin_leave(t0);
        assert_eq!(p.advance(at(t0, 10)), Advance::Nothing);
        assert!(p.busy(at(t0, 10)), "the fade needs the frame loop");
        let half = p.look(300.0, at(t0, 65));
        assert!(half.text > 0.3 && half.text < 0.7, "{half:?}");
        assert_eq!(half.icon, 1.0, "the badge stays while the text fades");
        assert_eq!(p.advance(at(t0, 140)), Advance::Shrink);
        assert_eq!(p.phase, Phase::Shrink);
        assert!(!p.busy(at(t0, 140)), "the springs drive the shrink");
        let gone = p.look(300.0, at(t0, 140));
        assert_eq!(gone.text, 0.0);
        assert!(!p.growing(), "shrinking uses the stiff closing spring");
        // back at the resting width nothing is left
        let rest = p.look(COLLAPSED_W, at(t0, 400));
        assert_eq!((rest.icon, rest.text), (0.0, 0.0));
    }

    #[test]
    fn copying_while_showing_swaps_content_without_replaying() {
        let t0 = Instant::now();
        let mut p = Peek::new(content(300.0), false, t0);
        p.begin_leave(at(t0, 1900));
        // a new copy arrives mid-fade: hold again, with the new content
        let mut next = content(220.0);
        next.line = "Another".into();
        p.swap(next.clone());
        assert_eq!(p.phase, Phase::Hold);
        assert_eq!(p.content, next);
        assert_eq!(p.shown, t0, "the widen is not replayed");
        assert!(p.growing());
        // and also when it was already shrinking: it widens again
        p.phase = Phase::Shrink;
        p.swap(content(250.0));
        assert_eq!(p.phase, Phase::Hold);
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
    fn reduced_motion_cross_fades_the_whole_capsule() {
        let t0 = Instant::now();
        let mut p = Peek::new(content(300.0), true, t0);
        // already at full size, fading in
        assert_eq!(p.progress(300.0), 1.0);
        let start = p.look(300.0, t0);
        assert_eq!((start.body, start.icon, start.text), (0.0, 0.0, 0.0));
        assert!(p.busy(at(t0, 50)));
        let mid = p.look(300.0, at(t0, 70));
        assert!(mid.body > 0.3 && mid.body < 0.7 && mid.body == mid.text, "{mid:?}");
        let held = p.look(300.0, at(t0, 1000));
        assert_eq!((held.body, held.icon, held.text), (1.0, 1.0, 1.0));
        assert!(!p.busy(at(t0, 1000)));
        // fading out: the body goes too, then the app drops the peek
        p.begin_leave(at(t0, 2000));
        let out = p.look(300.0, at(t0, 2065));
        assert!(out.body > 0.3 && out.body < 0.7, "{out:?}");
        assert_eq!(p.advance(at(t0, 2200)), Advance::Finished);
    }

    #[test]
    fn full_size_peek_does_not_need_the_frame_loop_while_holding() {
        let t0 = Instant::now();
        let p = Peek::new(content(300.0), false, t0);
        assert!(!p.busy(t0));
        assert!(!p.busy(at(t0, 1000)));
    }
}
