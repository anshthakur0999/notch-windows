//! Bluetooth peripherals: which earbuds, mice, keyboards and controllers are connected, and how
//! full their batteries are, as far as Windows says.
//!
//! One worker thread owns the Windows objects, like `media.rs`. Windows' device watchers call back
//! when a paired device appears, goes away or changes (it connects, a new battery value); each
//! callback only queues what it was told, so nothing runs while nothing changes. The worker keeps
//! the list in a `Registry` (plain data, so the rules can be tested) and tells the UI with a
//! payload-free `WM_APP` message; the UI then takes the list and the events (a device connected,
//! a battery ran low).
//!
//! What kind of device it is comes with the paired-device entry (its class of device, or for
//! Bluetooth LE its appearance). The battery is `DEVPKEY_Bluetooth_Battery`, the value Windows' own
//! Settings page shows; it lives on another object Windows groups under the same physical device,
//! so a second watcher over that group runs for as long as the device is connected and reports the
//! value again whenever it changes. What a device does not report there stays "unknown": no vendor
//! protocols, no guessing.

use std::collections::HashMap;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::Devices::Bluetooth::{BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind, DeviceInformationUpdate, DeviceWatcher};
use windows::Devices::Radios::{Radio, RadioKind, RadioState};
use windows::Foundation::{IPropertyValue, PropertyType, TypedEventHandler};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::{HSTRING, IInspectable, Interface};
use windows_collections::{IIterable, IMapView};

/// A battery at or below this has a "Battery low" look, and an alert is raised when it falls to
/// it, and again when it falls to `VERY_LOW_AT`.
pub const LOW_AT: u8 = 20;
pub const VERY_LOW_AT: u8 = 10;
/// A device that comes back this soon after it went is a reconnect, not news.
pub const QUICK_BACK: Duration = Duration::from_secs(30);

// what the paired-device entries are asked for
const CONNECTED: &str = "System.Devices.Aep.IsConnected";
const CONTAINER: &str = "System.Devices.Aep.ContainerId";
const COD_MAJOR: &str = "System.Devices.Aep.Bluetooth.Cod.Major";
const COD_MINOR: &str = "System.Devices.Aep.Bluetooth.Cod.Minor";
const APPEARANCE: &str = "System.Devices.Aep.Bluetooth.Le.Appearance";
/// `DEVPKEY_Bluetooth_Battery`: a byte, 0 to 100.
const BATTERY: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Earbuds, headphones, headsets, speakers: they can be the audio output.
    Audio,
    Controller,
    Mouse,
    Keyboard,
}

impl Kind {
    pub fn is_audio(self) -> bool {
        self == Kind::Audio
    }
}

/// The kind of a classic Bluetooth device from its Class of Device, as Windows reports the two
/// fields (`Cod.Major`: bits 8 to 12 of the 24-bit value; `Cod.Minor`: bits 2 to 7). `None`: not
/// something Notch shows (a phone, a PC).
pub fn kind_from_class(major: u32, minor: u32) -> Option<Kind> {
    match major {
        // audio/video: wearable headset, hands-free, loudspeaker, headphones
        4 => matches!(minor, 1 | 2 | 5 | 6).then_some(Kind::Audio),
        // peripheral: the top two bits say keyboard / pointing device, the low four the kind of gadget
        5 => match (minor >> 4, minor & 0xF) {
            (1 | 3, _) => Some(Kind::Keyboard),
            (2, _) => Some(Kind::Mouse),
            (0, 1 | 2) => Some(Kind::Controller), // joystick, gamepad
            _ => None,
        },
        _ => None,
    }
}

/// The kind of a Bluetooth LE device from its GAP appearance (category in the top ten bits).
pub fn kind_from_appearance(raw: u16) -> Option<Kind> {
    match (raw >> 6, raw & 0x3F) {
        // audio sink (speakers) and wearable audio device (earbuds, headsets, headphones)
        (33 | 37, _) => Some(Kind::Audio),
        (15, 1) => Some(Kind::Keyboard),
        (15, 2) => Some(Kind::Mouse),
        (15, 3 | 4) => Some(Kind::Controller), // joystick, gamepad
        _ => None,
    }
}

/// The battery value as a percentage, or `None` when it is not one (Windows uses values above 100
/// for "no reading").
pub fn battery_percent(raw: u32) -> Option<u8> {
    (raw <= 100).then_some(raw as u8)
}

/// Whether a battery falling from `prev` to `now` just crossed a line worth an alert.
pub fn low_crossed(prev: u8, now: u8) -> bool {
    [LOW_AT, VERY_LOW_AT].iter().any(|&line| prev > line && now <= line)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// Percent; `None`: the device does not say ("Battery unknown").
    pub battery: Option<u8>,
    /// Its Bluetooth address when Windows lists it as classic Bluetooth, the only kind `link` can
    /// connect and disconnect (`None`: Windows looks after those itself).
    pub addr: Option<u64>,
}

impl Device {
    pub fn low(&self) -> bool {
        self.battery.is_some_and(|b| b <= LOW_AT)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Connected; `back`: it was here a moment ago.
    Connected { dev: Device, back: bool },
    Disconnected(Device),
    /// The battery fell to `LOW_AT` or to `VERY_LOW_AT`.
    Low(Device),
    /// A connect or disconnect the user asked for is over: `ok` says whether the device then was
    /// (or no longer was) connected.
    Link { dev: Device, connect: bool, ok: bool },
}

/// What a watcher said about one device. `None`: not part of that report.
#[derive(Clone, Debug, Default)]
pub struct Update {
    pub id: String,
    pub name: Option<String>,
    pub connected: Option<bool>,
    pub battery: Option<u8>,
    pub kind: Option<Kind>,
}

#[derive(Clone, Debug)]
struct Entry {
    id: String,
    name: String,
    connected: bool,
    kind: Option<Kind>,
    battery: Option<u8>,
    /// It connected after the watchers had listed what was paired (so it is news once it is
    /// known to be a peripheral, which Windows may only say a moment after the connection).
    fresh: bool,
}

/// Every paired device Windows has told us about, and what changed.
#[derive(Default)]
pub struct Registry {
    entries: Vec<Entry>,
    /// The watchers have listed what was already paired: from now on a connection is news. Before
    /// that, devices that were connected when Notch started are just recorded, with no event.
    ready: bool,
    /// Names of devices that disconnected, and when.
    gone: Vec<(String, Instant)>,
}

impl Registry {
    pub fn set_ready(&mut self) {
        self.ready = true;
    }

    /// What entry `i` is: its own kind, or its twin's. One device is listed through both Bluetooth
    /// and Bluetooth LE, and only one of the two may know what it is (earbuds have an LE appearance
    /// but no classic class, and it is the classic entry that reports the connection).
    fn kind_at(&self, i: usize) -> Option<Kind> {
        let e = &self.entries[i];
        e.kind.or_else(|| self.entries.iter().filter(|o| !e.name.is_empty() && o.name == e.name).find_map(|o| o.kind))
    }

    /// Whether entry `i` is connected and a peripheral Notch shows.
    fn visible_at(&self, i: usize) -> bool {
        self.entries[i].connected && self.kind_at(i).is_some()
    }

    /// The ids of the entries that are connected and a peripheral Notch shows.
    pub fn visible_ids(&self) -> Vec<String> {
        (0..self.entries.len()).filter(|&i| self.visible_at(i)).map(|i| self.entries[i].id.clone()).collect()
    }

    fn device(&self, i: usize) -> Option<Device> {
        let e = &self.entries[i];
        // the twin may be the classic one even when this entry is the LE one
        let addr = self
            .entries
            .iter()
            .filter(|o| o.id == e.id || (!e.name.is_empty() && o.name == e.name))
            .find_map(|o| crate::link::classic_address(&o.id));
        Some(Device { id: e.id.clone(), name: e.name.clone(), kind: self.kind_at(i)?, battery: e.battery, addr })
    }

    /// Takes in one report; returns what is news.
    pub fn apply(&mut self, u: Update, now: Instant) -> Vec<Event> {
        let i = self.entries.iter().position(|e| e.id == u.id).unwrap_or_else(|| {
            self.entries.push(Entry { id: u.id.clone(), name: String::new(), connected: false, kind: None, battery: None, fresh: false });
            self.entries.len() - 1
        });
        let (before, was) = (self.entries[i].clone(), self.visible_at(i));
        let ready = self.ready;
        let e = &mut self.entries[i];
        if let Some(n) = u.name.filter(|n| !n.is_empty()) {
            e.name = n;
        }
        e.kind = u.kind.or(e.kind);
        e.battery = u.battery.or(e.battery);
        e.connected = u.connected.unwrap_or(e.connected);
        e.fresh = e.connected && (e.fresh || (!before.connected && ready));
        if !ready {
            return Vec::new();
        }
        let Some(dev) = self.device(i) else { return Vec::new() };
        // a device that shows up through both Bluetooth and Bluetooth LE is one device, not two
        let twin = self.entries.iter().enumerate().any(|(j, o)| j != i && self.visible_at(j) && o.name == dev.name);
        let e = &self.entries[i];
        match (was, self.visible_at(i)) {
            (false, true) if e.fresh && !twin => {
                let back = self.gone.iter().any(|(n, at)| *n == dev.name && now.duration_since(*at) < QUICK_BACK);
                self.gone.retain(|(n, _)| *n != dev.name);
                vec![Event::Connected { dev, back }]
            }
            (true, false) if !twin => {
                self.gone.push((dev.name.clone(), now));
                vec![Event::Disconnected(dev)]
            }
            (true, true) => match (before.battery, dev.battery) {
                (Some(was), Some(is)) if low_crossed(was, is) => vec![Event::Low(dev)],
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// The device was unpaired.
    pub fn remove(&mut self, id: &str, now: Instant) -> Vec<Event> {
        let events = self.apply(Update { id: id.into(), connected: Some(false), ..Update::default() }, now);
        self.entries.retain(|e| e.id != id);
        events
    }

    /// The connected devices Notch shows, each once.
    pub fn connected(&self) -> Vec<Device> {
        let mut out: Vec<Device> = Vec::new();
        for i in 0..self.entries.len() {
            let Some(d) = self.device(i).filter(|_| self.entries[i].connected) else { continue };
            match out.iter_mut().find(|o| o.name == d.name) {
                Some(o) => o.battery = o.battery.or(d.battery), // the twin may know the battery
                None => out.push(d),
            }
        }
        out
    }

    /// The paired peripherals that are not connected, each once, named. Only the ones `link` can
    /// connect have an address; the rest are listed too, so the tab shows everything that is paired.
    pub fn available(&self) -> Vec<Device> {
        let mut out: Vec<Device> = Vec::new();
        for i in 0..self.entries.len() {
            let Some(d) = self.device(i).filter(|d| !d.name.is_empty()) else { continue };
            let connected = self.entries.iter().any(|o| o.connected && o.name == d.name);
            if !connected && !out.iter().any(|o| o.name == d.name) {
                out.push(d);
            }
        }
        out
    }
}

// ---------------------------------------------------------------- the worker

/// What the worker last found, for the UI to pick up.
struct Shared {
    list: Vec<Device>,
    /// Paired, not connected.
    avail: Vec<Device>,
    /// The Bluetooth radio is on (assumed when there is none to ask).
    radio_on: bool,
    events: Vec<Event>,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared { list: Vec::new(), avail: Vec::new(), radio_on: true, events: Vec::new() });

/// Where to post the "something changed" message, set once by `start`.
static NOTIFY: OnceLock<(isize, u32)> = OnceLock::new();

/// Tells the UI to look again (a payload-free `WM_APP` message). Safe from any thread.
pub fn notify() {
    if let Some(&(hwnd, msg)) = NOTIFY.get() {
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as *mut core::ffi::c_void)), msg, WPARAM(0), LPARAM(0));
        }
    }
}

/// The connected devices as of the last change.
pub fn list() -> Vec<Device> {
    SHARED.lock().unwrap_or_else(|e| e.into_inner()).list.clone()
}

/// The paired devices that are not connected.
pub fn available() -> Vec<Device> {
    SHARED.lock().unwrap_or_else(|e| e.into_inner()).avail.clone()
}

/// Whether the Bluetooth radio is on.
pub fn bluetooth_on() -> bool {
    SHARED.lock().unwrap_or_else(|e| e.into_inner()).radio_on
}

fn push(e: Event) {
    SHARED.lock().unwrap_or_else(|e| e.into_inner()).events.push(e);
    notify();
}

/// Connects (or disconnects) `dev` on a thread of its own; `Event::Link` says how it went.
pub fn link(dev: Device, connect: bool) {
    let Some(addr) = dev.addr else { return };
    let _ = std::thread::Builder::new().name("notch-link".into()).spawn(move || {
        let ok = if connect { crate::link::connect(addr) } else { crate::link::disconnect(addr) };
        crate::log(&format!("link: {} {} -> {ok}", if connect { "connect" } else { "disconnect" }, dev.name));
        push(Event::Link { dev, connect, ok });
    });
}

/// Switches the Bluetooth radio on, as Windows' own quick settings do (Windows may ask first).
pub fn turn_on() {
    let _ = std::thread::Builder::new().name("notch-radio".into()).spawn(|| {
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let allowed = Radio::RequestAccessAsync().and_then(|op| op.join());
        crate::log(&format!("radio: access {allowed:?}"));
        for r in bluetooth_radios() {
            crate::log(&format!("radio: on -> {:?}", r.SetStateAsync(RadioState::On).and_then(|op| op.join())));
        }
    });
}

fn bluetooth_radios() -> Vec<Radio> {
    let all = Radio::GetRadiosAsync().and_then(|op| op.join());
    all.map(|v| v.into_iter().filter(|r| r.Kind() == Ok(RadioKind::Bluetooth)).collect()).unwrap_or_default()
}

/// The Bluetooth radios, each telling the worker when it is switched on or off (kept alive by the
/// caller: dropping a radio drops its handler).
fn watch_radios(tx: &Sender<Msg>) -> Vec<Radio> {
    let radios = bluetooth_radios();
    for r in &radios {
        let t = tx.clone();
        let _ = r.StateChanged(&TypedEventHandler::new(move |r: windows_core::Ref<'_, Radio>, _| {
            if let Some(r) = r.as_ref() {
                let _ = t.send(Msg::Radio(r.State() == Ok(RadioState::On)));
            }
            Ok(())
        }));
    }
    if let Some(r) = radios.first() {
        let _ = tx.send(Msg::Radio(r.State() == Ok(RadioState::On)));
    }
    radios
}

/// What happened since the last call.
pub fn take_events() -> Vec<Event> {
    std::mem::take(&mut SHARED.lock().unwrap_or_else(|e| e.into_inner()).events)
}

pub(crate) enum Msg {
    /// A paired device was listed or changed (or, with only a battery, a battery watcher spoke).
    /// `container`: what Windows groups the device's objects under, where its battery is.
    Seen { u: Update, container: Option<String> },
    Gone(String),
    /// A watcher has finished listing what was already paired.
    Listed,
    /// The Bluetooth radio was switched on (`true`) or off.
    Radio(bool),
}

/// Starts the worker; `msg` is posted to `hwnd` whenever the list or the events change. It runs for
/// as long as the app does.
pub fn start(hwnd: HWND, msg: u32) {
    let h = hwnd.0 as isize;
    let _ = NOTIFY.set((h, msg));
    let _ = std::thread::Builder::new().name("notch-devices".into()).spawn(run);
}

fn flag(v: &IInspectable) -> Option<bool> {
    v.cast::<IPropertyValue>().ok()?.GetBoolean().ok()
}

fn num(v: &IInspectable) -> Option<u32> {
    let p = v.cast::<IPropertyValue>().ok()?;
    match p.Type().ok()? {
        PropertyType::UInt8 => p.GetUInt8().ok().map(u32::from),
        PropertyType::UInt16 => p.GetUInt16().ok().map(u32::from),
        PropertyType::UInt32 => p.GetUInt32().ok(),
        PropertyType::Int16 => p.GetInt16().ok().and_then(|v| u32::try_from(v).ok()),
        PropertyType::Int32 => p.GetInt32().ok().and_then(|v| u32::try_from(v).ok()),
        _ => None,
    }
}

/// The kind of device a report carries: its class of device (classic Bluetooth; both fields are 0
/// for a device that has none) or its GAP appearance (LE).
fn kind_of(get: &impl Fn(&str) -> Option<IInspectable>) -> Option<Kind> {
    let n = |k: &str| get(k).and_then(|v| num(&v));
    match (n(COD_MAJOR), n(COD_MINOR), n(APPEARANCE)) {
        (Some(major), Some(minor), _) if major != 0 => kind_from_class(major, minor),
        (_, _, Some(raw)) if raw != 0 => u16::try_from(raw).ok().and_then(kind_from_appearance),
        _ => None,
    }
}

/// An `Update` (and the container id) from a watcher's report; `get` looks a property up by name.
fn update(id: HSTRING, name: Option<HSTRING>, get: impl Fn(&str) -> Option<IInspectable>) -> (Update, Option<String>) {
    let u = Update {
        id: id.to_string(),
        name: name.map(|n| n.to_string()),
        connected: get(CONNECTED).and_then(|v| flag(&v)),
        battery: None,
        kind: kind_of(&get),
    };
    let container = get(CONTAINER).and_then(|v| v.cast::<IPropertyValue>().ok()?.GetGuid().ok()).map(|g| format!("{g:?}"));
    crate::log(&format!("devices: {} {:?} connected={:?} kind={:?}", u.id, u.name, u.connected, u.kind));
    (u, container)
}

/// A watcher over the devices of one kind (classic Bluetooth, or LE) that are paired (or, for the
/// "Add a device" search, are not) and asks for what `update` reads with every report.
pub(crate) fn watch(le: bool, paired: bool, tx: &Sender<Msg>) -> Option<DeviceWatcher> {
    let aqs = if le {
        BluetoothLEDevice::GetDeviceSelectorFromPairingState(paired)
    } else {
        BluetoothDevice::GetDeviceSelectorFromPairingState(paired)
    }
    .ok()?;
    let ask = [CONNECTED, CONTAINER, COD_MAJOR, COD_MINOR, APPEARANCE].map(HSTRING::from).to_vec();
    let w = DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(
        &aqs,
        &IIterable::<HSTRING>::from(ask),
        DeviceInformationKind::AssociationEndpoint,
    )
    .ok()?;
    let t = tx.clone();
    w.Added(&TypedEventHandler::new(move |_, d: windows_core::Ref<'_, DeviceInformation>| {
        if let Some(d) = d.as_ref() {
            let p = d.Properties().ok();
            let (u, container) = update(d.Id()?, d.Name().ok(), |k| p.as_ref()?.Lookup(&HSTRING::from(k)).ok());
            let _ = t.send(Msg::Seen { u, container });
        }
        Ok(())
    }))
    .ok()?;
    let t = tx.clone();
    w.Updated(&TypedEventHandler::new(move |_, d: windows_core::Ref<'_, DeviceInformationUpdate>| {
        if let Some(d) = d.as_ref() {
            let p = d.Properties().ok();
            let (u, container) = update(d.Id()?, None, |k| p.as_ref()?.Lookup(&HSTRING::from(k)).ok());
            let _ = t.send(Msg::Seen { u, container });
        }
        Ok(())
    }))
    .ok()?;
    let t = tx.clone();
    w.Removed(&TypedEventHandler::new(move |_, d: windows_core::Ref<'_, DeviceInformationUpdate>| {
        if let Some(d) = d.as_ref() {
            let _ = t.send(Msg::Gone(d.Id()?.to_string()));
        }
        Ok(())
    }))
    .ok()?;
    let t = tx.clone();
    w.EnumerationCompleted(&TypedEventHandler::new(move |_, _| {
        let _ = t.send(Msg::Listed);
        Ok(())
    }))
    .ok()?;
    w.Start().ok()?;
    Some(w)
}

/// The battery in a report about one of the device's objects, if it carries one.
fn battery_in(props: windows_core::Result<IMapView<HSTRING, IInspectable>>) -> Option<u8> {
    props.ok()?.Lookup(&HSTRING::from(BATTERY)).ok().and_then(|v| num(&v)).and_then(battery_percent)
}

/// A watcher over everything Windows groups under the physical device behind paired device `id`,
/// asking for the battery. It lists the current value, then reports again whenever it changes; the
/// worker runs one only while the device is connected.
fn watch_battery(id: &str, container: &str, tx: &Sender<Msg>) -> Option<DeviceWatcher> {
    let aqs = HSTRING::from(format!("System.Devices.ContainerId:=\"{{{container}}}\""));
    let w = DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(
        &aqs,
        &IIterable::<HSTRING>::from(vec![HSTRING::from(BATTERY)]),
        DeviceInformationKind::Device,
    )
    .ok()?;
    let say = |tx: Sender<Msg>, id: String, b: Option<u8>| {
        if let Some(battery) = b {
            crate::log(&format!("devices: {id} battery={battery}"));
            let _ = tx.send(Msg::Seen { u: Update { id, battery: Some(battery), ..Update::default() }, container: None });
        }
    };
    let (t, who) = (tx.clone(), id.to_string());
    w.Added(&TypedEventHandler::new(move |_, d: windows_core::Ref<'_, DeviceInformation>| {
        say(t.clone(), who.clone(), d.as_ref().and_then(|d| battery_in(d.Properties())));
        Ok(())
    }))
    .ok()?;
    let (t, who) = (tx.clone(), id.to_string());
    w.Updated(&TypedEventHandler::new(move |_, d: windows_core::Ref<'_, DeviceInformationUpdate>| {
        say(t.clone(), who.clone(), d.as_ref().and_then(|d| battery_in(d.Properties())));
        Ok(())
    }))
    .ok()?;
    w.Start().ok()?;
    Some(w)
}

/// Gives every connected peripheral a battery watcher and stops the ones of devices that are gone.
/// A device can become one without a report of its own (its twin's report names its kind), so this
/// looks at all of them after every report.
fn sync_batteries(reg: &Registry, containers: &HashMap<String, String>, batteries: &mut HashMap<String, DeviceWatcher>, tx: &Sender<Msg>) {
    let want = reg.visible_ids();
    batteries.retain(|id, w| {
        let keep = want.contains(id);
        if !keep {
            let _ = w.Stop();
        }
        keep
    });
    for id in want {
        if !batteries.contains_key(&id) {
            if let Some(w) = containers.get(&id).and_then(|c| watch_battery(&id, c, tx)) {
                batteries.insert(id, w);
            }
        }
    }
}

fn run() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let (tx, rx) = channel();
    // kept alive for as long as the thread runs, like the watchers
    let _radios = watch_radios(&tx);
    // kept alive for as long as the thread runs: dropping a watcher stops it
    let watchers: Vec<DeviceWatcher> = [false, true].into_iter().filter_map(|le| watch(le, true, &tx)).collect();
    if watchers.is_empty() {
        crate::log("devices: no Bluetooth device watcher");
        return;
    }
    crate::log(&format!("devices: {} watcher(s) running", watchers.len()));
    let mut waiting = watchers.len();
    let mut reg = Registry::default();
    let mut containers: HashMap<String, String> = HashMap::new();
    // one battery watcher per connected peripheral, stopped when it goes
    let mut batteries: HashMap<String, DeviceWatcher> = HashMap::new();
    let mut shown: (Vec<Device>, Vec<Device>, bool) = (Vec::new(), Vec::new(), true);
    let mut radio_on = true;
    // sleeps here until Windows reports something
    while let Ok(m) = rx.recv() {
        let now = Instant::now();
        let events = match m {
            Msg::Radio(on) => {
                radio_on = on;
                Vec::new()
            }
            Msg::Listed => {
                waiting = waiting.saturating_sub(1);
                if waiting == 0 {
                    reg.set_ready();
                }
                Vec::new()
            }
            Msg::Gone(id) => {
                containers.remove(&id);
                let events = reg.remove(&id, now);
                sync_batteries(&reg, &containers, &mut batteries, &tx);
                events
            }
            Msg::Seen { u, container } => {
                if let Some(c) = container {
                    containers.insert(u.id.clone(), c);
                }
                let events = reg.apply(u, now);
                sync_batteries(&reg, &containers, &mut batteries, &tx);
                events
            }
        };
        // pairings with a radio this PC no longer has are listed as paired but cannot be connected
        let mine = crate::link::local_radios();
        let mut avail = reg.available();
        avail.retain(|d| crate::link::radio_of(&d.id).is_none_or(|r| mine.is_empty() || mine.contains(&r)));
        let now_shown = (reg.connected(), avail, radio_on);
        if now_shown == shown && events.is_empty() {
            continue; // an unplugged device changed: nothing the UI shows
        }
        shown = now_shown.clone();
        {
            let mut s = SHARED.lock().unwrap_or_else(|e| e.into_inner());
            (s.list, s.avail, s.radio_on) = now_shown;
            s.events.extend(events);
        }
        notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t0: Instant, secs: u64) -> Instant {
        t0 + Duration::from_secs(secs)
    }

    fn up(id: &str, name: &str, connected: bool, battery: Option<u8>, kind: Option<Kind>) -> Update {
        Update { id: id.into(), name: Some(name.into()), connected: Some(connected), battery, kind }
    }

    /// The class of device from the raw 24-bit value: Windows reports its two fields on their own.
    fn cod(raw: u32) -> Option<Kind> {
        kind_from_class((raw >> 8) & 0x1F, (raw >> 2) & 0x3F)
    }

    #[test]
    fn the_class_of_device_says_what_kind_of_gadget_it_is() {
        assert_eq!(cod(0x240418), Some(Kind::Audio), "AirPods report headphones");
        assert_eq!(cod(0x240404), Some(Kind::Audio), "wearable headset");
        assert_eq!(cod(0x240408), Some(Kind::Audio), "hands-free");
        assert_eq!(cod(0x002580), Some(Kind::Mouse), "pointing device");
        assert_eq!(cod(0x002540), Some(Kind::Keyboard));
        assert_eq!(cod(0x0025C0), Some(Kind::Keyboard), "a keyboard with a trackpad is a keyboard");
        assert_eq!(cod(0x002508), Some(Kind::Controller), "gamepad");
        assert_eq!(cod(0x002504), Some(Kind::Controller), "joystick");
        assert_eq!(cod(0x002500), None, "a peripheral that does not say what it is");
        assert_eq!(cod(0x5A020C), None, "a phone");
        assert_eq!(cod(0x02010C), None, "a laptop");
        assert_eq!(cod(0), None);
        // what Windows reported for earbuds and a phone paired with this machine
        assert_eq!(kind_from_class(4, 1), Some(Kind::Audio), "major 4, minor 1: wearable headset");
        assert_eq!(kind_from_class(2, 3), None, "major 2, minor 3: a smartphone");
        assert_eq!(kind_from_class(0, 0), None, "a classic entry that has no class (its LE twin may)");
    }

    #[test]
    fn the_le_appearance_says_what_kind_of_gadget_it_is() {
        assert_eq!(kind_from_appearance(0x03C1), Some(Kind::Keyboard));
        assert_eq!(kind_from_appearance(0x03C2), Some(Kind::Mouse));
        assert_eq!(kind_from_appearance(0x03C3), Some(Kind::Controller), "joystick");
        assert_eq!(kind_from_appearance(0x03C4), Some(Kind::Controller), "gamepad");
        assert_eq!(kind_from_appearance(0x0941), Some(Kind::Audio), "earbud");
        assert_eq!(kind_from_appearance(0x0940), Some(Kind::Audio), "wearable audio device (LE earbuds report this)");
        assert_eq!(kind_from_appearance(0x0943), Some(Kind::Audio), "headphones");
        assert_eq!(kind_from_appearance(0x0841), Some(Kind::Audio), "standalone speaker");
        assert_eq!(kind_from_appearance(0x03C0), None, "a generic HID device");
        assert_eq!(kind_from_appearance(0x0040), None, "a phone");
        assert_eq!(kind_from_appearance(0), None);
    }

    #[test]
    fn a_battery_is_a_percentage_or_unknown() {
        assert_eq!(battery_percent(82), Some(82));
        assert_eq!(battery_percent(0), Some(0));
        assert_eq!(battery_percent(100), Some(100));
        assert_eq!(battery_percent(101), None, "not a percentage: never shown as one");
        assert_eq!(battery_percent(255), None);
    }

    #[test]
    fn an_alert_is_raised_once_at_twenty_and_once_at_ten() {
        assert!(low_crossed(21, 20));
        assert!(!low_crossed(20, 19), "already past 20: no second alert");
        assert!(low_crossed(11, 10));
        assert!(!low_crossed(10, 9));
        assert!(low_crossed(80, 5), "falling past both lines at once is one alert");
        assert!(!low_crossed(50, 40) && !low_crossed(15, 15));
        assert!(!low_crossed(5, 80), "charging is not an alert");
    }

    #[test]
    fn what_was_connected_at_startup_is_recorded_without_news() {
        let (mut r, t) = (Registry::default(), Instant::now());
        // the watchers list the paired devices: the AirPods are already connected
        let ev = r.apply(up("a", "AirPods Pro", true, Some(82), Some(Kind::Audio)), t);
        assert!(ev.is_empty(), "no banner for what was already connected");
        assert_eq!(r.connected().len(), 1, "but it is on the Devices tab");
        r.set_ready();
        // a repeat of the same report is not news either
        assert!(r.apply(up("a", "AirPods Pro", true, Some(82), Some(Kind::Audio)), t).is_empty());
        // a new device connecting after that is
        let ev = r.apply(up("m", "MX Master", true, Some(41), Some(Kind::Mouse)), t);
        assert!(matches!(&ev[..], [Event::Connected { dev, back: false }] if dev.name == "MX Master" && dev.battery == Some(41)));
    }

    #[test]
    fn a_device_connects_and_disconnects_once_each() {
        let (mut r, t) = (Registry::default(), Instant::now());
        r.apply(up("a", "AirPods Pro", false, None, None), t);
        r.set_ready();
        let ev = r.apply(up("a", "AirPods Pro", true, None, Some(Kind::Audio)), t);
        assert!(matches!(&ev[..], [Event::Connected { dev, .. }] if dev.battery.is_none()), "battery unknown, not made up");
        // the battery arrives in a report of its own: the list changes, no new card
        let ev = r.apply(Update { id: "a".into(), battery: Some(82), ..Update::default() }, at(t, 1));
        assert!(ev.is_empty());
        assert_eq!(r.connected()[0].battery, Some(82));
        let ev = r.apply(Update { id: "a".into(), connected: Some(false), ..Update::default() }, at(t, 60));
        assert!(matches!(&ev[..], [Event::Disconnected(d)] if d.name == "AirPods Pro"));
        assert!(r.connected().is_empty());
        assert!(r.apply(Update { id: "a".into(), connected: Some(false), ..Update::default() }, at(t, 61)).is_empty(), "again: nothing");
    }

    #[test]
    fn a_device_windows_can_only_name_a_moment_after_it_connects_is_news_then() {
        let (mut r, t) = (Registry::default(), Instant::now());
        r.set_ready();
        // the connection is reported first, what it is a moment later
        assert!(r.apply(up("a", "Buds", true, None, None), t).is_empty());
        assert!(r.connected().is_empty(), "not shown while it is unknown");
        let ev = r.apply(Update { id: "a".into(), kind: Some(Kind::Audio), ..Update::default() }, at(t, 1));
        assert!(matches!(&ev[..], [Event::Connected { back: false, .. }]), "the card comes now");
        // but one that was already connected at startup stays quiet, however late it is named
        let mut r = Registry::default();
        assert!(r.apply(up("b", "Mouse", true, None, None), t).is_empty());
        r.set_ready();
        assert!(r.apply(Update { id: "b".into(), kind: Some(Kind::Mouse), ..Update::default() }, at(t, 5)).is_empty());
        assert_eq!(r.connected().len(), 1, "it is listed all the same");
    }

    #[test]
    fn a_device_that_is_not_a_peripheral_is_never_news() {
        let (mut r, t) = (Registry::default(), Instant::now());
        r.set_ready();
        assert!(r.apply(up("p", "Pixel 8", true, Some(50), None), t).is_empty(), "a phone");
        assert!(r.connected().is_empty());
    }

    #[test]
    fn a_quick_reconnect_is_marked_as_one() {
        let (mut r, t) = (Registry::default(), Instant::now());
        r.set_ready();
        r.apply(up("a", "Buds", true, Some(60), Some(Kind::Audio)), t);
        r.apply(up("a", "Buds", false, None, None), at(t, 10));
        let ev = r.apply(up("a", "Buds", true, None, None), at(t, 20));
        assert!(matches!(&ev[..], [Event::Connected { back: true, .. }]), "10 s later");
        r.apply(up("a", "Buds", false, None, None), at(t, 30));
        let ev = r.apply(up("a", "Buds", true, None, None), at(t, 30 + QUICK_BACK.as_secs() + 1));
        assert!(matches!(&ev[..], [Event::Connected { back: false, .. }]), "long enough ago to be news again");
    }

    #[test]
    fn the_connection_on_a_classic_entry_with_no_class_uses_its_le_twins_kind() {
        // earbuds as Windows listed them here: the LE entry knows they are audio, the classic one has
        // no class, and it is the classic one that reports the connection
        let (mut r, t) = (Registry::default(), Instant::now());
        r.apply(up("le", "Buds 4", false, None, Some(Kind::Audio)), t);
        r.apply(up("classic", "Buds 4", false, None, None), t);
        r.set_ready();
        assert!(r.visible_ids().is_empty());
        let ev = r.apply(Update { id: "classic".into(), connected: Some(true), ..Update::default() }, at(t, 30));
        assert!(matches!(&ev[..], [Event::Connected { dev, back: false }] if dev.kind == Kind::Audio && dev.name == "Buds 4"), "{ev:?}");
        assert_eq!(r.visible_ids(), ["classic"], "so it gets a battery watcher");
        assert_eq!(r.connected().len(), 1, "and is on the Devices tab");
        let ev = r.apply(Update { id: "classic".into(), connected: Some(false), ..Update::default() }, at(t, 60));
        assert!(matches!(&ev[..], [Event::Disconnected(_)]));
        assert!(r.connected().is_empty());
        // a device nobody knows the kind of stays hidden
        r.apply(up("p1", "Pixel", false, None, None), t);
        assert!(r.apply(Update { id: "p1".into(), connected: Some(true), ..Update::default() }, at(t, 70)).is_empty());
    }

    #[test]
    fn a_device_already_connected_at_startup_is_listed_once_its_twin_names_its_kind() {
        // the order Windows reported here: the connected classic entry first, the LE twin after it
        let (mut r, t) = (Registry::default(), Instant::now());
        assert!(r.apply(up("classic", "Buds 4", true, None, None), t).is_empty());
        assert!(r.connected().is_empty(), "nobody knows what it is yet");
        assert!(r.apply(up("le", "Buds 4", false, None, Some(Kind::Audio)), t).is_empty());
        r.set_ready();
        assert_eq!(r.visible_ids(), ["classic"], "the entry that is connected, not the one that was just reported");
        assert_eq!(r.connected().len(), 1);
        // its battery arrives from the battery watcher, on the entry that is connected
        r.apply(Update { id: "classic".into(), battery: Some(90), ..Update::default() }, t);
        assert_eq!(r.connected()[0].battery, Some(90));
    }

    #[test]
    fn a_device_seen_twice_is_one_device() {
        let (mut r, t) = (Registry::default(), Instant::now());
        r.set_ready();
        // the same mouse through Bluetooth and Bluetooth LE
        let ev = r.apply(up("classic", "MX Master", true, None, Some(Kind::Mouse)), t);
        assert_eq!(ev.len(), 1);
        let ev = r.apply(up("le", "MX Master", true, Some(41), Some(Kind::Mouse)), t);
        assert!(ev.is_empty(), "no second card");
        let list = r.connected();
        assert_eq!(list.len(), 1, "one row");
        assert_eq!(list[0].battery, Some(41), "with the battery the twin knows");
        // one twin going away while the other stays is not a disconnect
        assert!(r.apply(Update { id: "classic".into(), connected: Some(false), ..Update::default() }, at(t, 5)).is_empty());
        assert_eq!(r.connected().len(), 1);
    }

    #[test]
    fn the_battery_running_low_raises_one_alert_per_line() {
        let (mut r, t) = (Registry::default(), Instant::now());
        r.set_ready();
        r.apply(up("m", "Mouse", true, Some(25), Some(Kind::Mouse)), t);
        let batt = |r: &mut Registry, b| r.apply(Update { id: "m".into(), battery: Some(b), ..Update::default() }, t);
        assert!(batt(&mut r, 22).is_empty());
        assert!(matches!(&batt(&mut r, 20)[..], [Event::Low(d)] if d.battery == Some(20)));
        assert!(batt(&mut r, 19).is_empty(), "no alert for every percent");
        assert!(matches!(&batt(&mut r, 10)[..], [Event::Low(_)]));
        assert!(batt(&mut r, 9).is_empty());
        // while it is not connected, nothing is raised
        r.apply(Update { id: "m".into(), connected: Some(false), ..Update::default() }, t);
        assert!(batt(&mut r, 5).is_empty());
    }

    const CLASSIC: &str = "Bluetooth#Bluetooth70:a6:cc:81:41:82-88:92:cc:e9:4e:c6";
    const LE: &str = "BluetoothLE#BluetoothLE70:a6:cc:81:41:82-7d:f7:bf:a5:d0:ce";

    #[test]
    fn a_paired_device_that_is_not_connected_is_available_once_and_moves_when_it_connects() {
        let (mut r, t) = (Registry::default(), Instant::now());
        r.apply(up(CLASSIC, "Buds", false, None, None), t);
        r.apply(up(LE, "Buds", false, None, Some(Kind::Audio)), t);
        r.apply(up("Bluetooth#Bluetooth70:a6:cc:81:41:82-30:bb:7d:9b:47:9c", "Phone", false, None, None), t);
        r.set_ready();
        let a = r.available();
        assert_eq!(a.len(), 1, "the earbuds listed twice are one device, and a phone is not a peripheral");
        assert_eq!(a[0].name, "Buds");
        assert_eq!(a[0].addr, Some(0x8892_CCE9_4EC6), "the address of the classic entry, though the LE one named the kind");
        assert!(r.connected().is_empty());
        // it connects (the classic entry says so): Connected, no longer Available, same address
        r.apply(Update { id: CLASSIC.into(), connected: Some(true), ..Update::default() }, t);
        assert!(r.available().is_empty());
        assert_eq!(r.connected()[0].addr, Some(0x8892_CCE9_4EC6));
        // and back
        r.apply(Update { id: CLASSIC.into(), connected: Some(false), ..Update::default() }, at(t, 60));
        assert_eq!(r.available().len(), 1);
    }

    #[test]
    fn a_device_windows_only_lists_as_le_is_available_but_cannot_be_linked() {
        let (mut r, t) = (Registry::default(), Instant::now());
        r.apply(up(LE, "MX Master", false, None, Some(Kind::Mouse)), t);
        let a = r.available();
        assert_eq!((a.len(), a[0].addr), (1, None), "listed, with no button: Windows connects it itself");
        // a device with no name yet is not listed
        r.apply(up("BluetoothLE#BluetoothLE70:a6:cc:81:41:82-01:02:03:04:05:06", "", false, None, Some(Kind::Mouse)), t);
        assert_eq!(r.available().len(), 1);
    }

    #[test]
    fn an_unpaired_device_leaves_the_list_and_says_so() {
        let (mut r, t) = (Registry::default(), Instant::now());
        r.set_ready();
        r.apply(up("a", "Buds", true, Some(60), Some(Kind::Audio)), t);
        let ev = r.remove("a", at(t, 3));
        assert!(matches!(&ev[..], [Event::Disconnected(_)]));
        assert!(r.connected().is_empty());
        assert!(r.remove("a", at(t, 4)).is_empty());
    }
}
