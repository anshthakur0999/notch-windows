//! "Add a device": a search for nearby devices that are not paired yet, and pairing with the one the
//! user picks.
//!
//! Nothing here runs until the Add screen is opened. The search is two Windows device watchers (the
//! same two `devices` uses, asked for unpaired devices), stopped when the user stops it, leaves the
//! screen or 30 seconds are up. Pairing is Windows' own, with the two answers a peripheral can need
//! without a screen of its own: "yes, pair" and "type this code on the device". A device that
//! wants anything else (a PIN typed here, comparing two numbers) is not paired from here; Windows'
//! Bluetooth settings do those.

use crate::devices::{Kind, Msg, watch};
use std::sync::Mutex;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};
use windows::Devices::Enumeration::{
    DeviceInformation, DeviceInformationKind, DevicePairingKinds, DevicePairingRequestedEventArgs, DevicePairingResult,
    DevicePairingResultStatus, DeviceWatcher,
};
use windows::Foundation::TypedEventHandler;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::HSTRING;
use windows_collections::IIterable;

/// How long a search runs by itself.
pub const SEARCH_FOR: Duration = Duration::from_secs(30);
/// The list is for picking from, not for browsing: a crowded room shows its first dozen.
const MAX_NEARBY: usize = 12;

#[derive(Clone, Debug, PartialEq)]
pub struct Nearby {
    pub id: String,
    /// Empty: the device did not share one.
    pub name: String,
    pub kind: Kind,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub searching: bool,
    pub nearby: Vec<Nearby>,
    /// The id being paired with, and the code to type on it, if it asks for one.
    pub pairing: Option<String>,
    pub code: Option<String>,
    /// The id of a pairing that did not work.
    pub failed: Option<String>,
}

#[derive(Clone)]
struct Seen {
    id: String,
    name: String,
    kind: Option<Kind>,
}

#[derive(Default)]
struct State {
    /// Bumped by every start and stop, so a search thread knows it has been told to end.
    round: u64,
    searching: bool,
    seen: Vec<Seen>,
    pairing: Option<String>,
    code: Option<String>,
    failed: Option<String>,
    paired: bool,
    /// Wakes the search thread when it is told to stop.
    wake: Option<Sender<Msg>>,
    /// Cancels the pairing under way.
    cancel: Option<Box<dyn FnOnce() + Send>>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(State::default))
}

/// What the Add screen shows. Peripherals only (a phone is not something to pair from here), each
/// device once even when Windows sees it through both Bluetooth and Bluetooth LE.
pub fn snapshot() -> Snapshot {
    with(|s| Snapshot {
        searching: s.searching,
        nearby: nearby(&s.seen),
        pairing: s.pairing.clone(),
        code: s.code.clone(),
        failed: s.failed.clone(),
    })
}

fn nearby(seen: &[Seen]) -> Vec<Nearby> {
    let mut out: Vec<Nearby> = Vec::new();
    // the classic entry first: it is the one earbuds are paired through
    let mut order: Vec<&Seen> = seen.iter().collect();
    order.sort_by_key(|s| s.id.starts_with("BluetoothLE#"));
    for s in order {
        let Some(kind) = s.kind else { continue };
        if !s.name.is_empty() && out.iter().any(|n| n.name == s.name) {
            continue;
        }
        out.push(Nearby { id: s.id.clone(), name: s.name.clone(), kind });
    }
    out.truncate(MAX_NEARBY);
    out
}

/// Whether a pairing finished and worked since the last call.
pub fn take_paired() -> bool {
    with(|s| std::mem::take(&mut s.paired))
}

/// Starts looking for devices to pair (a search already on carries on).
pub fn start_search() {
    let round = with(|s| {
        if s.searching {
            return None;
        }
        s.round += 1;
        s.searching = true;
        s.seen.clear();
        s.failed = None;
        Some(s.round)
    });
    if let Some(round) = round {
        let _ = std::thread::Builder::new().name("notch-search".into()).spawn(move || search(round));
    }
}

/// Stops the search (the list stays).
pub fn stop_search() {
    with(|s| {
        s.round += 1;
        s.searching = false;
        if let Some(w) = s.wake.take() {
            let _ = w.send(Msg::Listed);
        }
    });
}

/// Leaves the Add screen: no search, no pairing, nothing remembered.
pub fn close() {
    stop_search();
    cancel();
    with(|s| {
        s.seen.clear();
        s.failed = None;
    });
}

fn search(round: u64) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let (tx, rx) = channel();
    let watchers: Vec<DeviceWatcher> = [false, true].into_iter().filter_map(|le| watch(le, false, &tx)).collect();
    crate::log(&format!("pair: searching with {} watcher(s)", watchers.len()));
    let live = with(|s| {
        let ok = s.round == round && !watchers.is_empty();
        if ok {
            s.wake = Some(tx.clone());
        }
        ok
    });
    let end = Instant::now() + SEARCH_FOR;
    while live {
        let Ok(m) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) else { break };
        let stale = with(|s| {
            if s.round != round {
                return true;
            }
            match m {
                Msg::Seen { u, .. } => match s.seen.iter_mut().find(|e| e.id == u.id) {
                    Some(e) => {
                        e.name = u.name.filter(|n| !n.is_empty()).unwrap_or_else(|| std::mem::take(&mut e.name));
                        e.kind = u.kind.or(e.kind);
                    }
                    None => s.seen.push(Seen { id: u.id, name: u.name.unwrap_or_default(), kind: u.kind }),
                },
                Msg::Gone(id) => s.seen.retain(|e| e.id != id),
                _ => {}
            }
            false
        });
        if stale {
            break;
        }
        crate::devices::notify();
    }
    for w in &watchers {
        let _ = w.Stop();
    }
    with(|s| {
        if s.round == round {
            s.searching = false;
            s.wake = None;
        }
    });
    crate::log("pair: search over");
    crate::devices::notify();
}

/// Pairs with a device the search found. The search stops: scanning slows a pairing down.
pub fn pair(id: &str) {
    stop_search();
    let id = id.to_string();
    with(|s| {
        s.pairing = Some(id.clone());
        s.code = None;
        s.failed = None;
    });
    crate::devices::notify();
    let _ = std::thread::Builder::new().name("notch-pair".into()).spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let ok = pair_with(&id);
        crate::log(&format!("pair: {id} -> {ok}"));
        with(|s| {
            // a cancel already cleared this
            if s.pairing.as_deref() == Some(id.as_str()) {
                s.pairing = None;
                s.code = None;
                s.cancel = None;
                s.paired = ok;
                s.failed = (!ok).then_some(id);
            }
        });
        crate::devices::notify();
    });
}

/// The Cancel button.
pub fn cancel() {
    let cancel = with(|s| {
        s.pairing = None;
        s.code = None;
        s.cancel.take()
    });
    if let Some(cancel) = cancel {
        cancel();
    }
    crate::devices::notify();
}

fn pair_with(id: &str) -> bool {
    let none = IIterable::<HSTRING>::from(Vec::new());
    let info = DeviceInformation::CreateFromIdAsyncWithKindAndAdditionalProperties(&HSTRING::from(id), &none, DeviceInformationKind::AssociationEndpoint)
        .and_then(|op| op.join());
    let Some(custom) = info.ok().and_then(|i| i.Pairing().ok()).and_then(|p| p.Custom().ok()) else { return false };
    let _ = custom.PairingRequested(&TypedEventHandler::new(|_, args: windows_core::Ref<'_, DevicePairingRequestedEventArgs>| {
        let Some(a) = args.as_ref() else { return Ok(()) };
        let kind = a.PairingKind()?;
        if kind == DevicePairingKinds::DisplayPin {
            let pin = a.Pin()?.to_string();
            crate::log("pair: the device wants a code typed on it");
            with(|s| s.code = Some(spaced(&pin)));
            crate::devices::notify();
            a.Accept()?;
        } else if kind == DevicePairingKinds::ConfirmOnly {
            a.Accept()?;
        } else {
            crate::log(&format!("pair: not a kind Notch can answer: {}", kind.0));
        }
        Ok(())
    }));
    let Ok(op) = custom.PairAsync(DevicePairingKinds::ConfirmOnly | DevicePairingKinds::DisplayPin) else { return false };
    let c = op.clone();
    with(|s| s.cancel = Some(Box::new(move || drop(c.Cancel()))));
    let result: windows_core::Result<DevicePairingResult> = op.join();
    let status = result.and_then(|r| r.Status());
    crate::log(&format!("pair: status {status:?}"));
    matches!(status, Ok(DevicePairingResultStatus::Paired | DevicePairingResultStatus::AlreadyPaired))
}

/// "482913" as "482 913".
pub fn spaced(code: &str) -> String {
    match code.len() {
        6 => format!("{} {}", &code[..3], &code[3..]),
        _ => code.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(id: &str, name: &str, kind: Option<Kind>) -> Seen {
        Seen { id: id.into(), name: name.into(), kind }
    }

    #[test]
    fn only_peripherals_are_offered_each_once() {
        let all = [
            seen("BluetoothLE#x-1", "Buds", Some(Kind::Audio)),
            seen("Bluetooth#x-2", "Buds", Some(Kind::Audio)),
            seen("BluetoothLE#x-3", "Beacon", None),
            seen("Bluetooth#x-4", "", Some(Kind::Keyboard)),
            seen("Bluetooth#x-5", "Pixel 8", None),
        ];
        let n = nearby(&all);
        assert_eq!(n.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["Buds", ""], "no phone, no beacon, one pair of Buds");
        assert_eq!(n[0].id, "Bluetooth#x-2", "the classic twin: it is the one earbuds pair through");
        assert_eq!(n[1].kind, Kind::Keyboard, "a nameless keyboard is still one");
        let crowd: Vec<Seen> = (0..40).map(|i| seen(&format!("Bluetooth#x-{i}"), &format!("Mouse {i}"), Some(Kind::Mouse))).collect();
        assert_eq!(nearby(&crowd).len(), MAX_NEARBY);
    }

    #[test]
    fn a_code_reads_in_two_halves() {
        assert_eq!(spaced("482913"), "482 913");
        assert_eq!(spaced("1234"), "1234", "other lengths are left alone");
    }
}
