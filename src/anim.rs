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

const MAX_STEP: f32 = 1.0 / 240.0;

impl Spring {
    pub fn new(v: f32) -> Self {
        Self { pos: v, vel: 0.0, target: v }
    }

    pub fn set_target(&mut self, t: f32) {
        self.target = t;
    }

    /// Advances the simulation by `dt` seconds using small, fixed sub-steps so the result
    /// is stable regardless of the frame rate.
    pub fn step(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.05);
        let w = 2.0 * std::f32::consts::PI * FREQ;
        let k = w * w;
        let c = 2.0 * ZETA * w;
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
    fn snap_settles_immediately() {
        let mut s = Spring::new(0.0);
        s.set_target(50.0);
        s.step(0.016);
        s.snap();
        assert!(s.settled());
        assert_eq!(s.pos, 50.0);
    }
}
