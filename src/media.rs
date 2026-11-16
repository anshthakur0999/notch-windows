//! Live media: what Windows says is playing (the system media session, which browsers, Spotify and
//! the Windows media apps all report to) and the buttons to drive it.
//!
//! One worker thread owns every Windows object. Windows calls back when the track, the play state
//! or the position changes; each callback only queues a refresh, so nothing runs while nothing
//! plays. The UI reads the latest snapshot and is told by a payload-free `WM_APP` message (the
//! same hand-off as `thumbs.rs`).

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionMediaProperties as Props,
    GlobalSystemMediaTransportControlsSessionPlaybackControls as Controls,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapDitherTypeNone,
    WICBitmapInterpolationModeFant, WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::Interface;

/// The longest side of the album art kept: the Media tab draws it 109 DIP across.
const ART_MAX: u32 = 256;
/// A second, small copy for the pill's covers (20 to 34 DIP): sampled from the big one they would
/// need the renderer's slowest filter on every frame.
const ART_SMALL: u32 = 64;

/// What is playing, as of `at`.
#[derive(Clone, Debug)]
pub struct Track {
    pub title: String,
    pub artist: String,
    /// "your browser", "Spotify": where it plays.
    pub source: String,
    pub playing: bool,
    /// Seconds into the track when it was read, and the track's length (0: unknown, or live).
    pub pos: f64,
    pub len: f64,
    /// Whether the player takes a seek, and has a track before / after this one (a single browser video has neither).
    pub seekable: bool,
    pub can_prev: bool,
    pub can_next: bool,
    pub at: Instant,
}

impl Track {
    /// Seconds into the track `elapsed` seconds after it was read: it keeps counting while playing.
    pub fn pos_after(&self, elapsed: f64) -> f64 {
        let p = self.pos + if self.playing { elapsed } else { 0.0 };
        if self.len > 0.0 { p.min(self.len) } else { p }
    }

    pub fn position(&self) -> f64 {
        self.pos_after(self.at.elapsed().as_secs_f64())
    }

    /// "Playing in your browser" / "Paused in Spotify".
    pub fn status_line(&self) -> String {
        format!("{} in {}", if self.playing { "Playing" } else { "Paused" }, self.source)
    }
}

/// Whether `next` is a different track from `prev` (a banner is worth showing), not just the same one
/// reporting again: browsers send several updates per track.
pub fn is_new_track(prev: Option<&Track>, next: &Track) -> bool {
    !next.title.is_empty() && prev.is_none_or(|p| p.title != next.title || p.artist != next.artist)
}

/// "Spotify.exe" -> "Spotify"; any browser -> "your browser".
pub fn source_name(id: &str) -> String {
    let lower = id.to_ascii_lowercase();
    if ["chrome", "msedge", "edge", "firefox", "brave", "opera", "vivaldi"].iter().any(|b| lower.contains(b)) {
        return "your browser".into();
    }
    // a packaged app ("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic") names itself after the "!"
    let name = id.rsplit('!').next().unwrap_or(id);
    let name = name.strip_suffix(".exe").unwrap_or(name);
    let name = name.rsplit('.').next().unwrap_or(name);
    match name {
        "" => "your computer".into(),
        "ZuneMusic" => "Media Player".into(),
        "ZuneVideo" => "Movies & TV".into(),
        n => n.into(),
    }
}

/// An album cover: premultiplied BGRA pixels, top row first.
pub struct Art {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

/// The cover at the two sizes it is drawn at.
pub struct Cover {
    pub full: Art,
    pub small: Art,
}

static TRACK: Mutex<Option<Track>> = Mutex::new(None);
/// A new cover for the UI to pick up: `Some(None)` means the cover is gone.
static ART: Mutex<Option<Option<Cover>>> = Mutex::new(None);

pub fn track() -> Option<Track> {
    TRACK.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn take_art() -> Option<Option<Cover>> {
    ART.lock().unwrap_or_else(|e| e.into_inner()).take()
}

#[derive(Clone, Copy, Debug)]
pub enum Cmd {
    Refresh,
    /// The player changed the track's details: it may have a new cover for the same track (a browser
    /// shows its own logo or the last cover until the real one loads).
    Props,
    /// The renderer was rebuilt and lost the cover: send it again.
    ResendArt,
    Toggle,
    Next,
    Prev,
    /// Seconds into the track.
    Seek(f64),
}

/// The worker. It runs for as long as the app does: it holds a sender of its own (the callbacks
/// do too), so dropping this handle does not stop it.
pub struct Media {
    tx: Sender<Cmd>,
}

impl Media {
    /// Starts the worker; `msg` is posted to `hwnd` whenever the snapshot changes.
    pub fn start(hwnd: HWND, msg: u32) -> Media {
        let (tx, rx) = channel();
        let (h, t) = (hwnd.0 as isize, tx.clone());
        let _ = std::thread::Builder::new().name("notch-media".into()).spawn(move || run(h, msg, t, rx));
        Media { tx }
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }
}

/// A callback that only asks the worker to look again (`cmd`: `Refresh`, or `Props` to re-read the cover too).
fn refresher<T, U>(tx: &Sender<Cmd>, cmd: Cmd) -> TypedEventHandler<T, U>
where
    T: windows_core::RuntimeType + 'static,
    U: windows_core::RuntimeType + 'static,
{
    let tx = tx.clone();
    TypedEventHandler::new(move |_, _| {
        let _ = tx.send(cmd);
        Ok(())
    })
}

/// The session being followed and the callbacks registered on it.
struct Followed {
    session: Session,
    tokens: [i64; 3],
}

impl Followed {
    fn new(session: Session, tx: &Sender<Cmd>) -> Option<Followed> {
        let a = session.MediaPropertiesChanged(&refresher(tx, Cmd::Props)).ok()?;
        let b = session.PlaybackInfoChanged(&refresher(tx, Cmd::Refresh)).ok()?;
        let c = session.TimelinePropertiesChanged(&refresher(tx, Cmd::Refresh)).ok()?;
        Some(Followed { session, tokens: [a, b, c] })
    }
}

impl Drop for Followed {
    fn drop(&mut self) {
        let _ = self.session.RemoveMediaPropertiesChanged(self.tokens[0]);
        let _ = self.session.RemovePlaybackInfoChanged(self.tokens[1]);
        let _ = self.session.RemoveTimelinePropertiesChanged(self.tokens[2]);
    }
}

fn run(hwnd: isize, msg: u32, tx: Sender<Cmd>, rx: Receiver<Cmd>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let manager = match Manager::RequestAsync().and_then(|op| op.join()) {
        Ok(m) => m,
        Err(e) => {
            crate::log(&format!("media: no session manager: {e}"));
            return;
        }
    };
    let _ = manager.CurrentSessionChanged(&refresher(&tx, Cmd::Refresh));
    let _ = tx.send(Cmd::Refresh);

    let mut followed: Option<Followed> = None;
    let mut shown = Shown::default();
    loop {
        let first = match shown.wake_in(Instant::now()) {
            // wake up to clear the track if the player says nothing more
            Some(left) => match rx.recv_timeout(left) {
                Ok(c) => c,
                Err(RecvTimeoutError::Timeout) => Cmd::Refresh,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(c) => c,
                Err(_) => break,
            },
        };
        // commands that piled up while we were busy; any number of refreshes is one
        let mut cmds = vec![first];
        cmds.extend(rx.try_iter());
        let props_changed = cmds.iter().any(|c| matches!(c, Cmd::Props));
        for cmd in cmds.iter().filter(|c| !matches!(c, Cmd::Refresh | Cmd::Props)) {
            if matches!(cmd, Cmd::ResendArt) {
                shown.has_art = false;
            } else if let Some(f) = &followed {
                let s = &f.session;
                let done = match *cmd {
                    Cmd::Toggle => s.TryTogglePlayPauseAsync().and_then(|op| op.join()),
                    Cmd::Next => s.TrySkipNextAsync().and_then(|op| op.join()),
                    Cmd::Prev => s.TrySkipPreviousAsync().and_then(|op| op.join()),
                    Cmd::Seek(secs) => s.TryChangePlaybackPositionAsync((secs * 1e7) as i64).and_then(|op| op.join()),
                    Cmd::Refresh | Cmd::Props | Cmd::ResendArt => Ok(false),
                };
                // Ok(false): the player took the command and refused it
                crate::log(&format!("media: {cmd:?} -> {done:?}"));
            }
        }

        // follow whichever session Windows says is current now
        let current = manager.GetCurrentSession().ok();
        let same = matches!((&followed, &current), (Some(f), Some(c)) if f.session.as_raw() == c.as_raw());
        if !same {
            followed = current.and_then(|c| Followed::new(c, &tx));
        }
        let read = followed.as_ref().and_then(|f| read(&f.session));
        crate::log(&format!(
            "media worker: cmds={cmds:?} switched={} read={:?}",
            !same,
            read.as_ref().map(|(t, _)| (&t.title, t.playing, t.pos, t.len))
        ));
        match read {
            Some((track, props)) => {
                let key = format!("{}\0{}\0{}", track.source, track.title, track.artist);
                shown.timeline(&key, track.playing, track.len, Instant::now());
                if shown.wants_art(&key, props_changed) {
                    let art = fetch_art(&props);
                    let got = art.is_some();
                    if shown.art_read(key, got) {
                        *ART.lock().unwrap_or_else(|e| e.into_inner()) = Some(art);
                    }
                }
                *TRACK.lock().unwrap_or_else(|e| e.into_inner()) = Some(track);
            }
            None => {
                if let Some(had_cover) = shown.quiet(Instant::now()) {
                    if had_cover {
                        *ART.lock().unwrap_or_else(|e| e.into_inner()) = Some(None);
                    }
                    *TRACK.lock().unwrap_or_else(|e| e.into_inner()) = None;
                }
            }
        }
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as *mut core::ffi::c_void)), msg, WPARAM(0), LPARAM(0));
        }
    }
}

/// How long a player may report nothing before it counts as stopped: between two tracks (Next) it
/// does, for a moment, and the screen must not flash "Nothing playing" for that.
const GONE_AFTER: Duration = Duration::from_millis(1500);

fn is_gone(since: Instant, now: Instant) -> bool {
    now.duration_since(since) >= GONE_AFTER
}

/// What the worker has told the UI so far, and what to do about the next look at the player. Kept
/// apart from the Windows calls so the rules can be tested.
#[derive(Default)]
struct Shown {
    /// The track the shown cover belongs to ("": nothing is shown).
    art_key: String,
    has_art: bool,
    /// Since when the player has reported nothing, while the last track is still shown.
    gone: Option<Instant>,
    /// The track playing with no length yet, and since when: its seek bar waits for one.
    lenless: Option<(String, Instant)>,
}

/// A player names a new track before it sends the length, and does not always say when it does
/// (the seek bar then stays missing): look again this often, for this long after the track starts.
const LEN_POLL: Duration = Duration::from_millis(750);
const LEN_POLL_FOR: Duration = Duration::from_secs(10);

impl Shown {
    /// How long the worker may sleep before it must look again (`None`: until something happens).
    fn wake_in(&self, now: Instant) -> Option<Duration> {
        let gone = self.gone.map(|since| GONE_AFTER.saturating_sub(now.duration_since(since)));
        let poll = self.lenless.as_ref().filter(|(_, since)| now.duration_since(*since) < LEN_POLL_FOR).map(|_| LEN_POLL);
        gone.into_iter().chain(poll).min()
    }

    /// The player reports track `key`, `playing` or not, with length `len` (0: not known).
    fn timeline(&mut self, key: &str, playing: bool, len: f64, now: Instant) {
        if !playing || len > 0.0 {
            self.lenless = None;
        } else if self.lenless.as_ref().is_none_or(|(k, _)| k != key) {
            self.lenless = Some((key.into(), now));
        }
    }

    /// The player reports track `key`. Should the cover be read again? A browser names the track
    /// first and sends the cover a moment later (until then it shows its logo or the last cover),
    /// so also when the details changed (`props_changed`) and when there is no cover yet.
    fn wants_art(&mut self, key: &str, props_changed: bool) -> bool {
        self.gone = None;
        key != self.art_key || !self.has_art || props_changed
    }

    /// The cover was read for `key` (`got`: there was one). Should the UI be told? A read that
    /// finds nothing keeps the cover already shown, unless the track changed.
    fn art_read(&mut self, key: String, got: bool) -> bool {
        let tell = key != self.art_key || got;
        if tell {
            self.has_art = got;
        }
        self.art_key = key;
        tell
    }

    /// The player reports nothing at `now`. `None`: keep showing the last track for now (between two
    /// tracks it says nothing for a moment). `Some(had_cover)`: it is really gone, clear it.
    fn quiet(&mut self, now: Instant) -> Option<bool> {
        let since = *self.gone.get_or_insert(now);
        if !self.art_key.is_empty() && !is_gone(since, now) {
            return None;
        }
        (self.gone, self.lenless) = (None, None);
        let shown = !self.art_key.is_empty();
        self.art_key.clear();
        self.has_art = false;
        Some(shown)
    }
}

/// 100 ns ticks between 1601 and 1970 (what `DateTime` counts from).
const EPOCH_GAP: i64 = 116_444_736_000_000_000;

/// The session's state, or `None` when nothing is playing or paused on it.
fn read(s: &Session) -> Option<(Track, Props)> {
    let props = s.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
    let info = s.GetPlaybackInfo().ok()?;
    let status = info.PlaybackStatus().ok()?;
    let playing = status == Status::Playing;
    if !playing && status != Status::Paused {
        return None;
    }
    let title = props.Title().ok()?.to_string();
    let artist = props.Artist().ok()?.to_string();
    if title.is_empty() && artist.is_empty() {
        return None;
    }
    let source = source_name(&s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default());

    let secs = |t: windows::Foundation::TimeSpan| t.Duration as f64 / 1e7;
    let (mut pos, mut len) = (0.0, 0.0);
    if let Ok(t) = s.GetTimelineProperties() {
        if let (Ok(start), Ok(end), Ok(at)) = (t.StartTime(), t.EndTime(), t.Position()) {
            len = (secs(end) - secs(start)).max(0.0);
            pos = (secs(at) - secs(start)).max(0.0);
            // the position was true when the player last said so; playing, it has moved on since
            if let (true, Ok(up)) = (playing, t.LastUpdatedTime()) {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() / 100) as i64 + EPOCH_GAP;
                let age = (now - up.UniversalTime) as f64 / 1e7;
                if (0.0..36_000.0).contains(&age) {
                    pos += age;
                }
            }
        }
    }
    let controls = info.Controls().ok();
    let enabled = |f: fn(&Controls) -> windows::core::Result<bool>| controls.as_ref().is_some_and(|c| f(c).unwrap_or(false));
    let seekable = len > 0.0 && enabled(|c| c.IsPlaybackPositionEnabled());
    // a player that says it has no next track still answers "done" to a skip, and does nothing
    let (can_prev, can_next) = (enabled(|c| c.IsPreviousEnabled()), enabled(|c| c.IsNextEnabled()));
    if len > 0.0 {
        pos = pos.min(len);
    }
    Some((Track { title, artist, source, playing, pos, len, seekable, can_prev, can_next, at: Instant::now() }, props))
}

/// The cover, decoded and shrunk to `ART_MAX` and `ART_SMALL`. Any failure is just "no cover".
fn fetch_art(props: &Props) -> Option<Cover> {
    let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()? as u32;
    if size == 0 || size > 16 << 20 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size).ok()?.join().ok()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    Some(Cover { full: decode(&bytes, ART_MAX)?, small: decode(&bytes, ART_SMALL)? })
}

/// The picture shrunk (never enlarged) so its longest side is at most `max`.
fn decode(bytes: &[u8], max: u32) -> Option<Art> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let stream = wic.CreateStream().ok()?;
        stream.InitializeFromMemory(bytes).ok()?;
        let decoder = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 {
            return None;
        }
        let k = (max as f32 / w.max(h) as f32).min(1.0);
        let (tw, th) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
        let scaler = wic.CreateBitmapScaler().ok()?;
        scaler.Initialize(&frame, tw, th, WICBitmapInterpolationModeFant).ok()?;
        let conv = wic.CreateFormatConverter().ok()?;
        conv.Initialize(&scaler, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)
            .ok()?;
        let mut px = vec![0u8; (tw * th * 4) as usize];
        conv.CopyPixels(std::ptr::null(), tw * 4, &mut px).ok()?;
        Some(Art { w: tw, h: th, px })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artist: &str, playing: bool, pos: f64, len: f64) -> Track {
        Track { title: title.into(), artist: artist.into(), source: "x".into(), playing, pos, len, seekable: true, can_prev: true, can_next: true, at: Instant::now() }
    }

    #[test]
    fn the_position_keeps_counting_while_playing_and_stops_at_the_end() {
        let t = track("a", "b", true, 100.0, 180.0);
        assert_eq!(t.pos_after(5.0), 105.0);
        assert_eq!(t.pos_after(500.0), 180.0, "never past the end");
        assert_eq!(track("a", "b", false, 100.0, 180.0).pos_after(5.0), 100.0, "paused holds");
        assert_eq!(track("a", "b", true, 100.0, 0.0).pos_after(5.0), 105.0, "no length: no end to stop at");
    }

    #[test]
    fn a_banner_only_for_a_different_track() {
        let a = track("Song", "Band", true, 0.0, 10.0);
        assert!(is_new_track(None, &a));
        assert!(!is_new_track(Some(&a), &track("Song", "Band", false, 5.0, 10.0)), "same track, new position or state");
        assert!(is_new_track(Some(&a), &track("Other", "Band", true, 0.0, 10.0)));
        assert!(is_new_track(Some(&a), &track("Song", "Cover Band", true, 0.0, 10.0)));
        assert!(!is_new_track(None, &track("", "Band", true, 0.0, 10.0)), "a browser names the track a moment later");
    }

    #[test]
    fn a_player_that_goes_quiet_between_tracks_is_not_gone_yet() {
        let t = Instant::now();
        assert!(!is_gone(t, t + Duration::from_millis(500)), "Next: the new track is about to be named");
        assert!(is_gone(t, t + Duration::from_secs(2)), "really stopped");
    }

    /// One look at the player that reports track `key`: `Some(told the UI)` if the cover was read.
    fn look(s: &mut Shown, key: &str, props_changed: bool, got_cover: bool) -> Option<bool> {
        s.wants_art(key, props_changed).then(|| s.art_read(key.into(), got_cover))
    }

    #[test]
    fn the_cover_is_read_once_per_track_and_again_when_the_details_change() {
        let mut s = Shown::default();
        assert_eq!(look(&mut s, "a", false, true), Some(true), "first look at a track");
        assert_eq!(look(&mut s, "a", false, true), None, "position ticks do not read it again");
        assert_eq!(look(&mut s, "a", true, true), Some(true), "a browser swaps its logo for the real cover");
        assert_eq!(look(&mut s, "b", false, true), Some(true), "next track");
    }

    #[test]
    fn a_cover_that_arrives_late_is_picked_up_and_a_failed_read_keeps_the_old_one() {
        let mut s = Shown::default();
        assert_eq!(look(&mut s, "a", false, false), Some(true), "new track without a cover: clear the old one");
        assert_eq!(look(&mut s, "a", false, false), Some(false), "keeps asking, nothing new to tell");
        assert_eq!(look(&mut s, "a", false, true), Some(true), "it arrived");
        assert_eq!(look(&mut s, "a", true, false), Some(false), "a read that fails keeps the cover shown");
        assert!(s.has_art);
    }

    #[test]
    fn a_track_stays_until_the_player_has_been_quiet_for_good() {
        let mut s = Shown::default();
        let t = Instant::now();
        assert_eq!(s.quiet(t), Some(false), "nothing was shown, nothing to hold");
        look(&mut s, "a", false, true);
        assert_eq!(s.quiet(t), None, "between two tracks");
        assert!(s.wake_in(t).is_some(), "the worker must wake up to clear it");
        assert_eq!(s.quiet(t + Duration::from_millis(500)), None);
        assert_eq!(s.quiet(t + Duration::from_secs(2)), Some(true), "really stopped: clear the track and its cover");
        assert!(s.wake_in(t).is_none());
        // the player comes back in time: the wait is over
        look(&mut s, "b", false, true);
        assert_eq!(s.quiet(t), None);
        look(&mut s, "b", false, true);
        assert!(s.wake_in(t).is_none());
    }

    #[test]
    fn a_new_track_without_a_length_is_asked_again_for_a_while() {
        let mut s = Shown::default();
        let t = Instant::now();
        s.timeline("a", true, 0.0, t);
        assert_eq!(s.wake_in(t), Some(LEN_POLL), "after Next the length may arrive without a word");
        s.timeline("a", true, 0.0, t + Duration::from_secs(3));
        assert!(s.wake_in(t + Duration::from_secs(5)).is_some(), "asking again does not restart the clock");
        assert!(s.wake_in(t + Duration::from_secs(11)).is_none(), "a stream with no length is not polled for ever");
        s.timeline("b", true, 0.0, t + Duration::from_secs(12));
        assert!(s.wake_in(t + Duration::from_secs(13)).is_some(), "a new track gets its own window");
        s.timeline("b", true, 215.0, t + Duration::from_secs(14));
        assert!(s.wake_in(t + Duration::from_secs(14)).is_none(), "the length arrived");
        s.timeline("c", false, 0.0, t + Duration::from_secs(15));
        assert!(s.wake_in(t + Duration::from_secs(15)).is_none(), "paused: nothing to wait for");
    }

    #[test]
    fn players_get_readable_names() {
        assert_eq!(source_name("Spotify.exe"), "Spotify");
        assert_eq!(source_name("Chrome"), "your browser");
        assert_eq!(source_name("MSEdge"), "your browser");
        assert_eq!(source_name("firefox.exe"), "your browser");
        assert_eq!(source_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "Media Player");
        assert_eq!(source_name(""), "your computer");
    }
}
