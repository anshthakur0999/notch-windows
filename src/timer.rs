//! The timer: state and arithmetic only (no Win32), so it can be unit tested.
//!
//! One timer at a time. `app.rs` drives it with a single one-shot Windows timer that fires exactly
//! when the displayed second changes (`next_tick`), so nothing wakes up while no timer is running.
//!
//! shortcut: time is measured with `Instant`, which does not count the time a PC spends asleep;
//! upgrade to the wall clock if timers have to survive sleep.

use std::time::{Duration, Instant};

/// Longest timer: one day.
pub const MAX_SECS: u64 = 24 * 3600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nothing is timing: the Timer tab shows the "set a time" form.
    Idle,
    Running,
    Paused,
    /// The time ran out and has not been dismissed yet ("Time's up").
    Finished,
}

#[derive(Clone, Copy, Debug)]
enum State {
    Idle,
    Running { ends: Instant },
    Paused { left: Duration },
    Finished,
}

#[derive(Debug)]
pub struct Timer {
    state: State,
    /// Length of this run, for the ring (it grows with "+1 min").
    total: Duration,
    pub label: String,
}

impl Timer {
    pub fn new() -> Timer {
        Timer { state: State::Idle, total: Duration::ZERO, label: String::new() }
    }

    pub fn phase(&self) -> Phase {
        match self.state {
            State::Idle => Phase::Idle,
            State::Running { .. } => Phase::Running,
            State::Paused { .. } => Phase::Paused,
            State::Finished => Phase::Finished,
        }
    }

    /// Running or paused: the pill shows the time left.
    pub fn active(&self) -> bool {
        matches!(self.state, State::Running { .. } | State::Paused { .. })
    }

    /// Starts a new run, replacing whatever was timing.
    pub fn start(&mut self, secs: u64, label: &str, now: Instant) {
        self.total = Duration::from_secs(secs);
        self.label = label.trim().to_string();
        self.state = State::Running { ends: now + self.total };
    }

    pub fn toggle_pause(&mut self, now: Instant) {
        self.state = match self.state {
            State::Running { ends } => State::Paused { left: ends.saturating_duration_since(now) },
            State::Paused { left } => State::Running { ends: now + left },
            other => other,
        };
    }

    /// "+1 min" while timing; after the time ran out, "+5 min" starts a fresh run of that length.
    pub fn add(&mut self, secs: u64, now: Instant) {
        let extra = Duration::from_secs(secs);
        match &mut self.state {
            State::Running { ends } => {
                *ends += extra;
                self.total += extra;
            }
            State::Paused { left } => {
                *left += extra;
                self.total += extra;
            }
            State::Finished => {
                self.total = extra;
                self.state = State::Running { ends: now + extra };
            }
            State::Idle => {}
        }
    }

    pub fn cancel(&mut self) {
        self.state = State::Idle;
    }

    pub fn left(&self, now: Instant) -> Duration {
        match self.state {
            State::Running { ends } => ends.saturating_duration_since(now),
            State::Paused { left } => left,
            State::Idle | State::Finished => Duration::ZERO,
        }
    }

    /// Whole seconds shown on screen: rounded up, so "0:01" is the last thing you see.
    pub fn left_secs(&self, now: Instant) -> u64 {
        (self.left(now).as_millis() as u64).div_ceil(1000)
    }

    /// How much of the run is left, 0..=1 (the ring).
    pub fn fraction(&self, now: Instant) -> f32 {
        if self.total.is_zero() {
            return 0.0;
        }
        (self.left(now).as_secs_f32() / self.total.as_secs_f32()).clamp(0.0, 1.0)
    }

    /// Turns a run whose time is up into `Finished`. Returns true on that change.
    pub fn tick(&mut self, now: Instant) -> bool {
        match self.state {
            State::Running { ends } if ends <= now => {
                self.state = State::Finished;
                true
            }
            _ => false,
        }
    }

    /// How long until the displayed second changes (or the time is up): the delay for the next
    /// one-shot timer. `None` unless running.
    pub fn next_tick(&self, now: Instant) -> Option<Duration> {
        let State::Running { ends } = self.state else { return None };
        let ms = ends.saturating_duration_since(now).as_millis() as u64;
        let to_boundary = if ms == 0 { 0 } else if ms.is_multiple_of(1000) { 1000 } else { ms % 1000 };
        Some(Duration::from_millis(to_boundary + 1)) // +1: land just after the boundary
    }
}

/// Reads what is typed in the time box: `25m`, `1h 30m`, `90s`, `1h30m`, `2 hours`, or a bare number
/// of minutes (`25`). `None` if it is empty, malformed, zero or longer than a day.
///
/// shortcut: no `1:30` or decimals (`1.5h`); add them if people type them.
pub fn parse(text: &str) -> Option<u64> {
    let s = text.trim().to_ascii_lowercase();
    let mut chars = s.chars().peekable();
    let (mut total, mut parts, mut bare) = (0u64, 0, false);
    while chars.peek().is_some() {
        let (mut n, mut digits) = (0u64, 0);
        while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
            n = n * 10 + d as u64;
            digits += 1;
            if digits > 6 {
                return None;
            }
            chars.next();
        }
        if digits == 0 {
            return None;
        }
        while chars.peek() == Some(&' ') {
            chars.next();
        }
        let mut unit = None; // the first letter of "m", "min", "minutes"... is enough
        while let Some(&c) = chars.peek().filter(|c| c.is_ascii_alphabetic()) {
            unit.get_or_insert(c);
            chars.next();
        }
        total += n * match unit {
            Some('h') => 3600,
            Some('m') => 60,
            Some('s') => 1,
            None => {
                bare = true;
                60
            }
            _ => return None,
        };
        parts += 1;
        while chars.peek() == Some(&' ') {
            chars.next();
        }
    }
    if (bare && parts > 1) || total == 0 || total > MAX_SECS {
        return None;
    }
    Some(total)
}

/// "25:00", "0:05", "1:24:58".
pub fn clock(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{}:{s:02}", secs / 60) }
}

/// The wall-clock time `secs` after `h:m:s`, as "10:31 pm" (12-hour; the minute is not rounded up).
///
/// shortcut: a daylight-saving change in between is ignored.
pub fn ends_at(h: u32, m: u32, s: u32, secs: u64) -> String {
    let day = (h as u64 * 3600 + m as u64 * 60 + s as u64 + secs) % 86_400;
    let (h24, min) = (day / 3600, day / 60 % 60);
    let h12 = if h24 % 12 == 0 { 12 } else { h24 % 12 };
    format!("{h12}:{min:02} {}", if h24 < 12 { "am" } else { "pm" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t: Instant, secs: f32) -> Instant {
        t + Duration::from_secs_f32(secs)
    }

    #[test]
    fn typed_times_are_understood() {
        for (text, secs) in [
            ("25m", 1500),
            ("1h 30m", 5400),
            ("1h30m", 5400),
            ("90s", 90),
            ("25", 1500), // a bare number is minutes
            (" 5 min ", 300),
            ("2 hours", 7200),
            ("45 sec", 45),
            ("1H 1M 1S", 3661),
            ("24h", MAX_SECS),
        ] {
            assert_eq!(parse(text), Some(secs), "{text:?}");
        }
    }

    #[test]
    fn nonsense_is_refused() {
        for text in ["", "   ", "m", "h 5", "1h 30", "0m", "0", "abc", "25x", "1.5h", "1:30", "25h", "9999999m", "5m,"] {
            assert_eq!(parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn clock_text() {
        assert_eq!(clock(1500), "25:00");
        assert_eq!(clock(872), "14:32");
        assert_eq!(clock(90), "1:30");
        assert_eq!(clock(5), "0:05");
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(3600), "1:00:00");
        assert_eq!(clock(5098), "1:24:58");
    }

    #[test]
    fn end_time_is_twelve_hour_and_wraps_past_midnight() {
        assert_eq!(ends_at(22, 6, 0, 1500), "10:31 pm");
        assert_eq!(ends_at(22, 6, 59, 1500), "10:31 pm", "seconds do not round the minute up");
        assert_eq!(ends_at(23, 50, 0, 1200), "12:10 am");
        assert_eq!(ends_at(11, 59, 0, 60), "12:00 pm");
        assert_eq!(ends_at(0, 5, 0, 0), "12:05 am");
    }

    #[test]
    fn a_run_counts_down_pauses_and_finishes() {
        let t0 = Instant::now();
        let mut t = Timer::new();
        assert_eq!(t.phase(), Phase::Idle);
        t.start(1500, "  Focus ", t0);
        assert_eq!((t.phase(), t.label.as_str()), (Phase::Running, "Focus"));
        assert!(t.active());
        assert_eq!(t.left_secs(at(t0, 628.0)), 872); // 14:32 left
        assert!((t.fraction(at(t0, 628.0)) - 872.0 / 1500.0).abs() < 1e-4);

        t.toggle_pause(at(t0, 600.0));
        assert_eq!(t.phase(), Phase::Paused);
        assert_eq!(t.left_secs(at(t0, 9000.0)), 900, "time stands still while paused");
        t.toggle_pause(at(t0, 1000.0));
        assert_eq!(t.left_secs(at(t0, 1010.0)), 890);

        assert!(!t.tick(at(t0, 1899.0)));
        assert!(t.tick(at(t0, 1900.0)));
        assert_eq!(t.phase(), Phase::Finished);
        assert!(!t.active());
        assert!(!t.tick(at(t0, 1901.0)), "only the change is reported");
        assert_eq!(t.left_secs(at(t0, 2000.0)), 0);
    }

    #[test]
    fn plus_minutes_extend_the_run_and_the_ring() {
        let t0 = Instant::now();
        let mut t = Timer::new();
        t.start(60, "", t0);
        t.add(60, at(t0, 30.0));
        assert_eq!(t.left_secs(at(t0, 30.0)), 90);
        assert!((t.fraction(at(t0, 30.0)) - 0.75).abs() < 1e-4, "90 s of 120 s");
        t.toggle_pause(at(t0, 30.0));
        t.add(60, at(t0, 31.0));
        assert_eq!(t.left_secs(at(t0, 99.0)), 150);
        // after the time ran out, "+5 min" is a fresh five minutes
        let mut t = Timer::new();
        t.start(10, "Tea", t0);
        assert!(t.tick(at(t0, 11.0)));
        t.add(300, at(t0, 20.0));
        assert_eq!((t.phase(), t.left_secs(at(t0, 20.0))), (Phase::Running, 300));
        assert_eq!(t.fraction(at(t0, 20.0)), 1.0);
        // add does nothing when idle
        t.cancel();
        t.add(60, t0);
        assert_eq!(t.phase(), Phase::Idle);
    }

    #[test]
    fn the_next_wake_up_is_exactly_when_the_digits_change() {
        let t0 = Instant::now();
        let mut t = Timer::new();
        t.start(1500, "", t0);
        assert_eq!(t.next_tick(at(t0, 0.0)), Some(Duration::from_millis(1001)), "on a boundary: a full second");
        let wait = t.next_tick(at(t0, 628.6)).unwrap(); // 871.4 s left, showing 14:32
        assert_eq!(wait, Duration::from_millis(401));
        // after that wait the display has moved on by exactly one second
        let later = at(t0, 628.6) + wait;
        assert_eq!(t.left_secs(at(t0, 628.6)) - t.left_secs(later), 1);
        // the last fraction of a second wakes up just after the end
        let wait = t.next_tick(at(t0, 1499.7)).unwrap();
        assert!(wait <= Duration::from_millis(302) && wait >= Duration::from_millis(300));
        t.toggle_pause(t0);
        assert_eq!(t.next_tick(t0), None, "no wake-ups while paused");
        t.cancel();
        assert_eq!(t.next_tick(t0), None, "or while idle");
    }
}
