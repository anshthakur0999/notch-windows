//! The visualizer's ears. While something plays, this listens to what the default output device is
//! sending to the speakers (WASAPI loopback, not the microphone) and turns the last 2048 samples
//! into `BANDS` bar heights. Nothing is recorded or kept: the samples live in a small buffer on the
//! thread and are overwritten. The thread exists only while the visualizer is on screen and
//! something plays; dropping the `Listener` ends it.

use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, IAudioCaptureClient,
    IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, eConsole, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// Bars in the Media tab; the pill shows `PILL_BARS` of them, each the loudest of a group.
pub const BANDS: usize = 40;
pub const PILL_BARS: usize = 5;
const N: usize = 2048;

/// The latest bar heights (0..1), already smoothed. The UI copies them out when it draws.
static LEVELS: Mutex<[f32; BANDS]> = Mutex::new([0.0; BANDS]);
/// A redraw message is on its way; the UI clears this when it gets it, so the thread never floods.
static PENDING: AtomicBool = AtomicBool::new(false);

/// Time between looks at the music. Every look that moves the bars redraws the pill, and a redraw
/// costs a few milliseconds of CPU (the software rasteriser's worker threads), so the tiny bars on
/// the closed pill update 8 times a second and the big ones on the Media tab 20.
const PILL_MS: u32 = 125;
const TAB_MS: u32 = 50;
/// The spacing the bars' rise and fall below are tuned for; other spacings scale them to match.
const SMOOTH_MS: f32 = 33.0;
static INTERVAL_MS: AtomicU32 = AtomicU32::new(PILL_MS);

/// `true` while the Media tab shows the big bars, `false` for the closed pill's.
pub fn set_big(big: bool) {
    INTERVAL_MS.store(if big { TAB_MS } else { PILL_MS }, Ordering::Relaxed);
}

pub fn levels() -> [f32; BANDS] {
    *LEVELS.lock().unwrap_or_else(|e| e.into_inner())
}

/// `n` bars for the pill: the `BANDS` heights folded into `n` groups, each the loudest of its band.
pub fn fold(levels: &[f32; BANDS], n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| levels[i * BANDS / n..(i + 1) * BANDS / n].iter().copied().fold(0.0, f32::max))
        .collect()
}

pub fn redraw_handled() {
    PENDING.store(false, Ordering::Release);
}

/// The running listener. Dropping it stops the thread.
pub struct Listener {
    stop: Arc<AtomicBool>,
}

impl Listener {
    /// Starts listening; `msg` is posted to `hwnd` when the bars have moved.
    pub fn start(hwnd: HWND, msg: u32) -> Listener {
        let stop = Arc::new(AtomicBool::new(false));
        let (h, s) = (hwnd.0 as isize, stop.clone());
        let _ = std::thread::Builder::new().name("notch-viz".into()).spawn(move || run(h, msg, s));
        Listener { stop }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn run(hwnd: isize, msg: u32, stop: Arc<AtomicBool>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let post = || {
        if !PENDING.swap(true, Ordering::AcqRel) {
            unsafe {
                let _ = PostMessageW(Some(HWND(hwnd as *mut core::ffi::c_void)), msg, WPARAM(0), LPARAM(0));
            }
        }
    };
    while !stop.load(Ordering::Acquire) {
        if listen(&stop, &post).is_none() {
            crate::log("viz: no output device to listen to, trying again");
        }
        // the device went away (headphones, a driver update) or there was none: look again shortly
        for _ in 0..10 {
            if stop.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    *LEVELS.lock().unwrap_or_else(|e| e.into_inner()) = [0.0; BANDS];
}

/// One listening session on the current default output device. `None` if it could not start or
/// the device went away; `Some` when asked to stop.
fn listen(stop: &AtomicBool, post: &dyn Fn()) -> Option<()> {
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None).ok()?;
        let fmt = client.GetMixFormat().ok()?;
        let WAVEFORMATEX { wFormatTag: tag, nChannels: channels, nSamplesPerSec: rate, wBitsPerSample: bits, .. } =
            fmt.read_unaligned();
        // the shared-mode mix is 32-bit float on practically every machine (tag 3, or "extensible")
        let float = bits == 32 && (tag == 3 || tag == 0xFFFE);
        let ok = (float || bits == 16) && channels > 0 && rate > 0;
        let init = if ok {
            client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 2_000_000, 0, fmt, None)
        } else {
            Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))
        };
        CoTaskMemFree(Some(fmt.cast()));
        init.ok()?;
        let capture: IAudioCaptureClient = client.GetService().ok()?;
        client.Start().ok()?;

        let mut samples: Vec<f32> = Vec::with_capacity(N * 2);
        let mut bars = [0.0f32; BANDS];
        let mut moving = false;
        while !stop.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(INTERVAL_MS.load(Ordering::Relaxed) as u64));
            // everything that arrived since last time, as one mono stream
            while capture.GetNextPacketSize().ok()? > 0 {
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut::<u8>(), 0u32, 0u32);
                capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None).ok()?;
                let (n, c) = (frames as usize, channels as usize);
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    samples.extend(std::iter::repeat_n(0.0, n));
                } else if float {
                    let all = std::slice::from_raw_parts(data.cast::<f32>(), n * c);
                    samples.extend(all.chunks_exact(c).map(|f| f.iter().sum::<f32>() / c as f32));
                } else {
                    let all = std::slice::from_raw_parts(data.cast::<i16>(), n * c);
                    samples.extend(all.chunks_exact(c).map(|f| f.iter().map(|&s| s as f32 / 32768.0).sum::<f32>() / c as f32));
                }
                capture.ReleaseBuffer(frames).ok()?;
            }
            if samples.len() > N {
                samples.drain(..samples.len() - N);
            }
            let heard = spectrum(&samples, rate as f32);
            let mut changed = false;
            // the same fall-back speed however far apart the looks are (see `INTERVAL_MS`)
            let k = INTERVAL_MS.load(Ordering::Relaxed) as f32 / SMOOTH_MS;
            let (attack, decay) = (1.0 - 0.4f32.powf(k), 0.85f32.powf(k));
            for (bar, &now) in bars.iter_mut().zip(&heard) {
                // up fast, down slowly: it reads as a bar falling back, not flicker
                let next = if now > *bar { *bar + (now - *bar) * attack } else { *bar * decay };
                let next = if next < 0.01 { 0.0 } else { next };
                changed |= (next - *bar).abs() > 0.004;
                *bar = next;
            }
            if changed || moving {
                *LEVELS.lock().unwrap_or_else(|e| e.into_inner()) = bars;
                post();
            }
            moving = changed;
        }
        let _ = client.Stop();
        Some(())
    }
}

/// `(cos, sin)` of `-2 pi j / N` for the first half turn: every step of the FFT reads these.
fn twiddles() -> &'static [(f32, f32); N / 2] {
    static T: std::sync::OnceLock<[(f32, f32); N / 2]> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        std::array::from_fn(|j| {
            let (s, c) = (-2.0 * PI * j as f32 / N as f32).sin_cos();
            (c, s)
        })
    })
}

/// In-place radix-2 FFT.
fn fft(re: &mut [f32; N], im: &mut [f32; N]) {
    let tw = twiddles();
    let bits = N.trailing_zeros();
    for i in 0..N {
        let j = (i as u32).reverse_bits() >> (32 - bits);
        if j as usize > i {
            re.swap(i, j as usize);
            im.swap(i, j as usize);
        }
    }
    let mut len = 2;
    while len <= N {
        let stride = N / len;
        for start in (0..N).step_by(len) {
            for k in 0..len / 2 {
                let (c, s) = tw[k * stride];
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * c - im[b] * s, re[b] * s + im[b] * c);
                (re[b], im[b]) = (re[a] - tr, im[a] - ti);
                (re[a], im[a]) = (re[a] + tr, im[a] + ti);
            }
        }
        len *= 2;
    }
}

/// Bar heights (0..1) for the last samples: the loudness of `BANDS` log-spaced slices of 60 Hz to
/// 12 kHz, in decibels (-60 dB and below is flat, 0 dB is full), with a tilt that lifts the
/// treble, which music carries much less of.
pub fn spectrum(samples: &[f32], rate: f32) -> [f32; BANDS] {
    let mut re = [0.0f32; N];
    let mut im = [0.0f32; N];
    let n = samples.len().min(N);
    let off = N - n;
    for (i, &s) in samples[samples.len() - n..].iter().enumerate() {
        re[off + i] = s * (0.5 - 0.5 * (2.0 * PI * (off + i) as f32 / (N - 1) as f32).cos());
    }
    fft(&mut re, &mut im);
    let bin_hz = rate / N as f32;
    let (lo, hi) = (60.0f32, (rate / 2.0).min(12_000.0));
    let edge = |b: usize| lo * (hi / lo).powf(b as f32 / BANDS as f32);
    let mut out = [0.0f32; BANDS];
    for (b, bar) in out.iter_mut().enumerate() {
        let i0 = ((edge(b) / bin_hz).floor() as usize).max(1);
        let i1 = ((edge(b + 1) / bin_hz).ceil() as usize).clamp(i0 + 1, N / 2);
        let peak = (i0..i1).map(|i| re[i].hypot(im[i])).fold(0.0, f32::max);
        // a sine of amplitude A leaves 4|X|/N = A once the Hann window's gain is taken out
        let db = 20.0 * (4.0 * peak / N as f32).max(1e-6).log10() + 14.0 * b as f32 / BANDS as f32;
        *bar = ((db + 60.0) / 52.0).clamp(0.0, 1.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, amp: f32, rate: f32) -> Vec<f32> {
        (0..N).map(|i| amp * (2.0 * PI * hz * i as f32 / rate).sin()).collect()
    }

    fn loudest(bars: &[f32; BANDS]) -> usize {
        (0..BANDS).fold(0, |m, i| if bars[i] > bars[m] { i } else { m })
    }

    #[test]
    fn a_tone_lights_the_bar_for_its_pitch() {
        // 1 kHz is 21.2 of the 40 log-spaced slices between 60 Hz and 12 kHz
        let bars = spectrum(&sine(1000.0, 0.5, 48_000.0), 48_000.0);
        assert!((20..=22).contains(&loudest(&bars)), "loudest bar {}", loudest(&bars));
        // an octave up lands about 5.2 slices further on
        let up = loudest(&spectrum(&sine(2000.0, 0.5, 48_000.0), 48_000.0));
        assert!((4..=7).contains(&(up - loudest(&bars))), "{up}");
        // and the same pitch at a lower sample rate still lands on the same bar
        assert!((20..=22).contains(&loudest(&spectrum(&sine(1000.0, 0.5, 44_100.0), 44_100.0))));
    }

    #[test]
    fn silence_is_flat_and_a_quiet_tone_is_lower_than_a_loud_one() {
        assert!(spectrum(&[0.0; N], 48_000.0).iter().all(|&b| b == 0.0));
        assert!(spectrum(&[], 48_000.0).iter().all(|&b| b == 0.0), "no samples yet");
        let loud = spectrum(&sine(1000.0, 0.5, 48_000.0), 48_000.0)[21];
        let soft = spectrum(&sine(1000.0, 0.02, 48_000.0), 48_000.0)[21];
        assert!(loud > soft && loud <= 1.0 && soft >= 0.0, "{loud} {soft}");
    }

    #[test]
    fn the_pill_bars_are_the_loudest_of_their_group() {
        let mut l = [0.0f32; BANDS];
        l[0] = 0.3;
        l[7] = 0.9; // group 0 of 5 covers bands 0..8
        l[39] = 0.5; // the last group
        assert_eq!(fold(&l, 5), [0.9, 0.0, 0.0, 0.0, 0.5]);
    }
}
