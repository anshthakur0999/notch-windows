//! Spring physics used for the pill's morphing animation.
//!
//! A spring keeps its velocity when the target changes mid-flight, so interrupting an
//! expand with a collapse (or the reverse) never produces a visual jump.

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub pos: f32,
    pub vel: f32,
    pub target: f32,
}

/// Natural frequency (Hz) and damping ratio. Slightly under-damped gives the small
/// "settle" overshoot that makes the motion feel alive.
pub const FREQ: f32 = 3.4;
pub const ZETA: f32 = 0.78;

/// Closing is stiffer and critically damped: the pill snaps shut quickly and, unlike the
/// opening spring, never bounces below its resting size.
pub const CLOSE_FREQ: f32 = 7.0;
pub const CLOSE_ZETA: f32 = 1.0;

/// The copy-peek capsule going back to the resting pill. Also critically damped (it must not dip
/// below the resting size), but softer than closing the panel: with `CLOSE_FREQ` the capsule
/// jumped ~25 px in its first frame and was shut in 8 frames, which read as a snap.
pub const PEEK_OUT_FREQ: f32 = 4.0;
pub const PEEK_OUT_ZETA: f32 = 1.0;

const MAX_STEP: f32 = 1.0 / 240.0;

impl Spring {
    pub fn new(v: f32) -> Self {
        Self { pos: v, vel: 0.0, target: v }
    }

    pub fn set_target(&mut self, t: f32) {
        self.target = t;
    }

    /// Advances the simulation by `dt` seconds with the opening spring (the app picks the
    /// spring itself via `step_with`; this is the shorthand the tests use).
    #[cfg(test)]
    pub fn step(&mut self, dt: f32) {
        self.step_with(dt, FREQ, ZETA);
    }

    /// Advances the simulation by `dt` seconds using small, fixed sub-steps so the result
    /// is stable regardless of the frame rate. Position and velocity carry over between
    /// calls, so switching `freq`/`zeta` mid-flight (open <-> close) never makes a jump.
    pub fn step_with(&mut self, dt: f32, freq: f32, zeta: f32) {
        let dt = dt.clamp(0.0, 0.05);
        let w = 2.0 * std::f32::consts::PI * freq;
        let k = w * w;
        let c = 2.0 * zeta * w;
        let mut left = dt;
        while left > 1e-6 {
            let h = left.min(MAX_STEP);
            let acc = -k * (self.pos - self.target) - c * self.vel;
            self.vel += acc * h;
            self.pos += self.vel * h;
            left -= h;
        }
    }

    pub fn settled(&self) -> bool {
        // Sub-pixel and slow enough that snapping to the target is invisible; ends the
        // frame loop (and its CPU use) as soon as the motion is visually finished.
        (self.pos - self.target).abs() < 0.2 && self.vel.abs() < 3.0
    }

    pub fn snap(&mut self) {
        self.pos = self.target;
        self.vel = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &mut Spring, secs: f32) {
        let mut t = 0.0;
        while t < secs {
            s.step(1.0 / 60.0);
            t += 1.0 / 60.0;
        }
    }

    #[test]
    fn settles_on_target() {
        let mut s = Spring::new(34.0);
        s.set_target(320.0);
        run(&mut s, 2.0);
        assert!(s.settled(), "pos={} vel={}", s.pos, s.vel);
        assert!((s.pos - 320.0).abs() < 0.1);
    }

    #[test]
    fn overshoot_is_small() {
        let mut s = Spring::new(0.0);
        s.set_target(100.0);
        let mut max = 0.0f32;
        for _ in 0..240 {
            s.step(1.0 / 120.0);
            max = max.max(s.pos);
        }
        assert!(max < 110.0, "overshoot too large: {max}");
    }

    #[test]
    fn retarget_mid_flight_is_continuous() {
        let mut s = Spring::new(34.0);
        s.set_target(320.0);
        run(&mut s, 0.12);
        let before = s.pos;
        let vel_before = s.vel;
        s.set_target(34.0); // interrupt
        assert_eq!(s.pos, before, "position must not jump");
        assert_eq!(s.vel, vel_before, "velocity must be preserved");
        s.step(1.0 / 60.0);
        assert!((s.pos - before).abs() < 20.0, "single step moved too far");
        run(&mut s, 2.0);
        assert!(s.settled());
        assert!((s.pos - 34.0).abs() < 0.1);
    }

    #[test]
    fn frame_rate_independent() {
        let mut a = Spring::new(0.0);
        let mut b = Spring::new(0.0);
        a.set_target(100.0);
        b.set_target(100.0);
        for _ in 0..30 {
            a.step(1.0 / 60.0);
        }
        for _ in 0..120 {
            b.step(1.0 / 240.0);
        }
        assert!((a.pos - b.pos).abs() < 1.0, "a={} b={}", a.pos, b.pos);
    }

    #[test]
    fn huge_dt_is_clamped_and_stable() {
        let mut s = Spring::new(0.0);
        s.set_target(100.0);
        s.step(10.0);
        assert!(s.pos.is_finite() && s.pos.abs() < 1000.0);
    }

    #[test]
    fn closing_is_faster_and_never_undershoots() {
        // time (s) until the height is within 1 px of its target, and the lowest value reached
        fn close(freq: f32, zeta: f32) -> (f32, f32) {
            let mut s = Spring::new(456.0);
            s.set_target(34.0);
            let (mut t, mut min, mut within) = (0.0f32, f32::MAX, None);
            while t < 2.0 {
                s.step_with(1.0 / 60.0, freq, zeta);
                t += 1.0 / 60.0;
                min = min.min(s.pos);
                if within.is_none() && (s.pos - 34.0).abs() < 1.0 {
                    within = Some(t);
                }
            }
            (within.unwrap_or(f32::MAX), min)
        }
        let (t_open_spring, _) = close(FREQ, ZETA);
        let (t_close, min) = close(CLOSE_FREQ, CLOSE_ZETA);
        assert!(min >= 34.0 - 0.01, "closing must not bounce below the pill's size: {min}");
        assert!(t_close < 0.22, "close took {t_close}s");
        assert!(t_close < t_open_spring * 0.7, "close {t_close}s vs open spring {t_open_spring}s");
    }

    #[test]
    fn the_peek_leaves_gently_but_still_briskly() {
        // the capsule's widest case: 340 -> 148, one 60 Hz frame at a time
        fn leave(freq: f32, zeta: f32) -> (f32, f32, f32) {
            let mut s = Spring::new(340.0);
            s.set_target(148.0);
            let (mut t, mut min, mut first, mut done) = (0.0f32, f32::MAX, 0.0f32, None);
            let mut prev = s.pos;
            while t < 1.5 {
                s.step_with(1.0 / 60.0, freq, zeta);
                t += 1.0 / 60.0;
                if first == 0.0 {
                    first = prev - s.pos;
                }
                min = min.min(s.pos);
                if done.is_none() && (s.pos - 148.0).abs() < 2.0 {
                    done = Some(t);
                }
                prev = s.pos;
            }
            (first, min, done.unwrap_or(f32::MAX))
        }
        let (first_close, _, t_close) = leave(CLOSE_FREQ, CLOSE_ZETA);
        let (first, min, t) = leave(PEEK_OUT_FREQ, PEEK_OUT_ZETA);
        assert!(min >= 148.0 - 0.01, "must not dip below the resting width: {min}");
        assert!(first < first_close * 0.6, "first frame moved {first:.1}px vs {first_close:.1}px when closing");
        assert!(first / 192.0 < 0.10, "first frame covered {:.0}% of the way", first / 1.92);
        assert!(t > t_close, "softer than the panel's close ({t}s vs {t_close}s)");
        assert!(t < 0.30, "still brisk: {t}s");
    }

    #[test]
    fn switching_springs_mid_flight_is_continuous() {
        let mut s = Spring::new(34.0);
        s.set_target(456.0);
        for _ in 0..8 {
            s.step(1.0 / 60.0); // opening...
        }
        let (pos, vel) = (s.pos, s.vel);
        s.set_target(34.0); // ...interrupted by a close
        assert_eq!((s.pos, s.vel), (pos, vel), "retargeting must not touch position or velocity");
        let mut min = f32::MAX;
        for _ in 0..120 {
            s.step_with(1.0 / 60.0, CLOSE_FREQ, CLOSE_ZETA);
            min = min.min(s.pos);
        }
        assert!(s.settled(), "pos={} vel={}", s.pos, s.vel);
        assert!(min >= 34.0 - 0.01, "bounced below the pill's size: {min}");
    }

    #[test]
    fn snap_settles_immediately() {
        let mut s = Spring::new(0.0);
        s.set_target(50.0);
        s.step(0.016);
        s.snap();
        assert!(s.settled());
        assert_eq!(s.pos, 50.0);
    }
}
