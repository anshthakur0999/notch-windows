//! Playback devices: which ones are active, which is the default, and which one belongs to a
//! Bluetooth device. Making another one the default is `policy::set_default`.
//!
//! Read on demand (when the Devices tab opens, after a switch, when a device card asks); nothing
//! here runs by itself.

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::{
    DEVICE_STATE_ACTIVE, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
};
use windows::Win32::System::Com::StructuredStorage::{
    PropVariantClear, PropVariantToStringAlloc, PropVariantToUInt32WithDefault,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::core::GUID;

/// What the icon of a playback device shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutKind {
    Speakers,
    Headphones,
    Display,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub id: String,
    /// "Speakers", "Headphones": the part of Windows' name before the brackets.
    pub name: String,
    /// "Realtek(R) Audio", "AirPods Pro": what is in the brackets (the device behind it).
    pub sub: String,
    pub kind: OutKind,
    pub default: bool,
}

/// "Speakers (Realtek(R) Audio)" -> ("Speakers", "Realtek(R) Audio"); a name without brackets has no
/// second part.
pub fn split_name(full: &str) -> (String, String) {
    match full.split_once(" (") {
        Some((a, b)) => (a.trim().into(), b.strip_suffix(')').unwrap_or(b).trim().into()),
        None => (full.trim().into(), String::new()),
    }
}

/// The playback device of the Bluetooth device called `device`: Windows names its endpoints after
/// it ("Headphones (AirPods Pro)"). A phone-call endpoint ("... Hands-Free") is only the answer
/// when there is no other.
pub fn output_for<'a>(device: &str, outs: &'a [Output]) -> Option<&'a Output> {
    let want = device.trim().to_lowercase();
    if want.is_empty() {
        return None;
    }
    let text = |o: &Output| format!("{} {}", o.name, o.sub).to_lowercase();
    outs.iter().filter(|o| text(o).contains(&want)).min_by_key(|o| text(o).contains("hands-free"))
}

/// Whether the default playback device is no longer the one a listener is bound to (`now`: `None`
/// when there is no default at all).
pub fn default_moved(bound: &str, now: Option<&str>) -> bool {
    now != Some(bound)
}

fn key(pid: u32, fmtid: u128) -> PROPERTYKEY {
    PROPERTYKEY { fmtid: GUID::from_u128(fmtid), pid }
}

/// The endpoint's id string.
pub fn id_of(device: &IMMDevice) -> Option<String> {
    unsafe {
        let p = device.GetId().ok()?;
        let id = p.to_string().ok();
        CoTaskMemFree(Some(p.0.cast()));
        id
    }
}

fn friendly_name(store: &IPropertyStore) -> Option<String> {
    unsafe {
        // PKEY_Device_FriendlyName
        let mut v = store.GetValue(&key(14, 0xa45c254e_df1c_4efd_8020_67d146a850e0)).ok()?;
        let s = PropVariantToStringAlloc(&v).ok().map(|p| {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0.cast()));
            s
        });
        let _ = PropVariantClear(&mut v);
        s
    }
}

fn kind_of(store: &IPropertyStore) -> OutKind {
    unsafe {
        // PKEY_AudioEndpoint_FormFactor: 1 speakers, 3 headphones, 5 headset, 9 a display's speakers
        let Ok(mut v) = store.GetValue(&key(0, 0x1da5d803_d492_4edd_8c23_e0c0ffee7f0e)) else { return OutKind::Speakers };
        let ff = PropVariantToUInt32WithDefault(&v, 10);
        let _ = PropVariantClear(&mut v);
        match ff {
            3 | 5 => OutKind::Headphones,
            9 => OutKind::Display,
            _ => OutKind::Speakers,
        }
    }
}

/// The active playback devices, the default one flagged. Empty if Windows says nothing.
pub fn outputs() -> Vec<Output> {
    unsafe {
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else {
            return Vec::new();
        };
        let default = en.GetDefaultAudioEndpoint(eRender, eConsole).ok().and_then(|d| id_of(&d));
        let Ok(all) = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else { return Vec::new() };
        let mut out = Vec::new();
        for i in 0..all.GetCount().unwrap_or(0) {
            let Ok(dev) = all.Item(i) else { continue };
            let (Some(id), Ok(store)) = (id_of(&dev), dev.OpenPropertyStore(STGM_READ)) else { continue };
            let (name, sub) = split_name(&friendly_name(&store).unwrap_or_default());
            out.push(Output { default: default.as_deref() == Some(id.as_str()), id, name, sub, kind: kind_of(&store) });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(name: &str, sub: &str, default: bool) -> Output {
        Output { id: format!("{name}{sub}"), name: name.into(), sub: sub.into(), kind: OutKind::Speakers, default }
    }

    #[test]
    fn windows_names_split_into_a_title_and_the_device_behind_it() {
        assert_eq!(split_name("Speakers (Realtek(R) Audio)"), ("Speakers".into(), "Realtek(R) Audio".into()));
        assert_eq!(split_name("Headphones (AirPods Pro)"), ("Headphones".into(), "AirPods Pro".into()));
        assert_eq!(split_name("Monitor"), ("Monitor".into(), String::new()));
        assert_eq!(split_name(""), (String::new(), String::new()));
    }

    #[test]
    fn a_bluetooth_device_finds_its_own_playback_device() {
        let outs = [
            out("Speakers", "Realtek(R) Audio", true),
            out("Headset", "AirPods Pro Hands-Free", false),
            out("Headphones", "AirPods Pro Stereo", false),
            out("Monitor", "NVIDIA High Definition Audio", false),
        ];
        let got = output_for("AirPods Pro", &outs).expect("it is there");
        assert_eq!(got.name, "Headphones", "music quality, not the phone-call endpoint");
        assert_eq!(output_for("airpods pro", &outs).map(|o| o.name.as_str()), Some("Headphones"), "case does not matter");
        assert_eq!(output_for("Logitech MX", &outs), None, "a mouse has no playback device");
        assert_eq!(output_for("  ", &outs), None, "no name matches everything");
        // only the hands-free one is there (before the music one has appeared): better than nothing
        assert_eq!(output_for("AirPods Pro", &outs[..2]).map(|o| o.name.as_str()), Some("Headset"));
    }

    #[test]
    fn a_listener_notices_when_the_default_playback_device_moved() {
        assert!(!default_moved("a", Some("a")));
        assert!(default_moved("a", Some("b")), "switched to another device");
        assert!(default_moved("a", None), "no default left");
    }
}
