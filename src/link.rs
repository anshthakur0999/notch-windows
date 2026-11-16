//! Connect and disconnect a paired classic Bluetooth device: the two things Windows has no friendly
//! call for. Run on a short thread of their own (they wait for the answer), once per click.
//!
//! * Disconnect is the Bluetooth driver's own "drop this link" request (`bthioctl.h`), which keeps
//!   the pairing.
//! * Connect switches the device's services off and on again, which makes Windows connect to it.
//!   Both are what Windows' bthprops API offers; neither is documented as "connect".
//!
//! Only devices Windows lists as classic Bluetooth (earbuds, most keyboards, controllers) can be
//! reached this way. A Bluetooth LE device (many mice) is connected by Windows itself.

use std::ffi::c_void;
use std::time::{Duration, Instant};
use windows::Win32::Devices::Bluetooth::{
    BLUETOOTH_DEVICE_INFO, BLUETOOTH_FIND_RADIO_PARAMS, BLUETOOTH_SERVICE_DISABLE, BLUETOOTH_SERVICE_ENABLE,
    BluetoothEnumerateInstalledServices, BluetoothFindFirstRadio, BluetoothFindNextRadio, BluetoothFindRadioClose,
    BluetoothGetDeviceInfo, BluetoothGetRadioInfo, BluetoothSetServiceState, BLUETOOTH_RADIO_INFO,
};
use windows::Win32::Foundation::{CloseHandle, ERROR_MORE_DATA, HANDLE};
use windows::Win32::System::IO::DeviceIoControl;
use windows::core::GUID;

/// `IOCTL_BTH_DISCONNECT_DEVICE`: `CTL_CODE(FILE_DEVICE_BLUETOOTH, 3, METHOD_BUFFERED, FILE_ANY_ACCESS)`
/// in `bthioctl.h`. Its input is the device's 8-byte address.
const DISCONNECT: u32 = 0x0041_000C;

/// A connect gives up trying another service after this long (one switch takes about six seconds),
/// and waits this long after each for the device to show up. A disconnect is quick.
const CONNECT_WAIT: Duration = Duration::from_secs(10);
const CONNECT_SETTLE: Duration = Duration::from_secs(6);
const DISCONNECT_WAIT: Duration = Duration::from_secs(4);

/// The address of a classic Bluetooth device from its paired-device id
/// (`Bluetooth#Bluetooth70:a6:cc:81:41:82-88:92:cc:e9:4e:c6`: the part after the last dash).
/// `None` for anything else, Bluetooth LE ids (`BluetoothLE#...`) included.
pub fn classic_address(id: &str) -> Option<u64> {
    let digits: String = id.strip_prefix("Bluetooth#")?.rsplit('-').next()?.split(':').collect();
    if digits.len() != 12 {
        return None;
    }
    u64::from_str_radix(&digits, 16).ok()
}

/// The local radio a paired-device id belongs to: `Bluetooth#Bluetooth70:a6:cc:81:41:82-88:...` (or
/// the `BluetoothLE` kind) is paired with the radio at 70:a6:cc:81:41:82.
pub fn radio_of(id: &str) -> Option<u64> {
    let rest = id.split_once('#')?.1;
    let rest = rest.strip_prefix("BluetoothLE").or_else(|| rest.strip_prefix("Bluetooth"))?;
    let digits: String = rest.rsplit_once('-')?.0.split(':').collect();
    if digits.len() != 12 {
        return None;
    }
    u64::from_str_radix(&digits, 16).ok()
}

/// The addresses of this PC's Bluetooth radios. Windows keeps the pairings of an adapter that is
/// gone (a replaced card, a dongle that was unplugged) and lists them as paired, but they cannot
/// be connected.
pub fn local_radios() -> Vec<u64> {
    radios()
        .into_iter()
        .filter_map(|r| {
            let mut ri = BLUETOOTH_RADIO_INFO { dwSize: size_of::<BLUETOOTH_RADIO_INFO>() as u32, ..Default::default() };
            let ok = unsafe { BluetoothGetRadioInfo(r, &mut ri) } == 0;
            let _ = unsafe { CloseHandle(r) };
            ok.then(|| unsafe { ri.address.Anonymous.ullLong })
        })
        .collect()
}

fn info(addr: u64) -> Option<BLUETOOTH_DEVICE_INFO> {
    let mut i = BLUETOOTH_DEVICE_INFO { dwSize: size_of::<BLUETOOTH_DEVICE_INFO>() as u32, ..Default::default() };
    i.Address.Anonymous.ullLong = addr;
    (unsafe { BluetoothGetDeviceInfo(None, &mut i) } == 0).then_some(i)
}

/// Whether Windows says the device is connected right now (`None`: it does not know the device).
pub fn is_connected(addr: u64) -> Option<bool> {
    info(addr).map(|i| i.fConnected.as_bool())
}

/// Waits (this is a click's wait, not a poll) for the device to be, or not be, connected.
fn settle(addr: u64, want: bool, within: Duration) -> bool {
    let end = Instant::now() + within;
    loop {
        if is_connected(addr) == Some(want) {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// The local Bluetooth radios (a PC has one, nearly always).
fn radios() -> Vec<HANDLE> {
    let params = BLUETOOTH_FIND_RADIO_PARAMS { dwSize: size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32 };
    let mut h = HANDLE::default();
    let Ok(find) = (unsafe { BluetoothFindFirstRadio(&params, &mut h) }) else { return Vec::new() };
    let mut out = vec![h];
    while unsafe { BluetoothFindNextRadio(find, &mut h) }.is_ok() {
        out.push(h);
    }
    let _ = unsafe { BluetoothFindRadioClose(find) };
    out
}

/// Drops the device's link and waits to see it gone. The pairing stays.
pub fn disconnect(addr: u64) -> bool {
    let mut asked = false;
    for radio in radios() {
        if !asked {
            let r = unsafe { DeviceIoControl(radio, DISCONNECT, Some((&addr as *const u64).cast::<c_void>()), 8, None, 0, None, None) };
            crate::log(&format!("link: disconnect {addr:012X}: {r:?}"));
            asked = r.is_ok();
        }
        let _ = unsafe { CloseHandle(radio) }; // every radio's, also the ones after the one that took it
    }
    asked && settle(addr, false, DISCONNECT_WAIT)
}

/// Asks Windows to connect to the device and waits to see it connected. A device that is off or out
/// of range does not answer, and this says so after `CONNECT_WAIT`.
pub fn connect(addr: u64) -> bool {
    let Some(i) = info(addr) else {
        crate::log(&format!("link: connect {addr:012X}: Windows does not know it"));
        return false;
    };
    let mut n = 32u32;
    let mut found = [GUID::zeroed(); 32];
    let err = unsafe { BluetoothEnumerateInstalledServices(None, &i, &mut n, Some(found.as_mut_ptr())) };
    // more than 32 services only means the first 32 were filled in; any other failure filled in none
    if err != 0 && err != ERROR_MORE_DATA.0 {
        crate::log(&format!("link: connect {addr:012X}: no services ({err})"));
        return false;
    }
    let services = order(found[..n.min(32) as usize].to_vec());
    crate::log(&format!("link: connect {addr:012X}: {} service(s), enumerate={err}", services.len()));
    let start = Instant::now();
    for s in &services {
        // One switch is enough to make Windows connect, and each takes seconds (Windows reinstalls
        // the service), so stop at the first that works: switching the services of a device that
        // is connected would cut its audio.
        let off = unsafe { BluetoothSetServiceState(None, &i, s, BLUETOOTH_SERVICE_DISABLE) };
        // always switched back on, whatever the first call said: never leave one off
        let on = unsafe { BluetoothSetServiceState(None, &i, s, BLUETOOTH_SERVICE_ENABLE) };
        crate::log(&format!("link:   {s:?} off={off} on={on} after {:.1}s", start.elapsed().as_secs_f32()));
        if settle(addr, true, CONNECT_SETTLE) {
            return true;
        }
        if start.elapsed() >= CONNECT_WAIT {
            break;
        }
    }
    false
}

/// The 16-bit number of a service that has one (`0000110B-0000-1000-8000-00805F9B34FB`: 0x110B).
fn short_id(g: &GUID) -> Option<u32> {
    (g.data2 == 0 && g.data3 == 0x1000 && g.data4 == [0x80, 0x00, 0x00, 0x80, 0x5F, 0x9B, 0x34, 0xFB]).then_some(g.data1)
}

/// The services to switch, best first: the ones that carry audio and input (A2DP sink, HID, hands-free,
/// headset), then the rest. The two GATT ones are not a profile to connect and are left out.
fn order(mut services: Vec<GUID>) -> Vec<GUID> {
    const FIRST: [u32; 4] = [0x110B, 0x1124, 0x111E, 0x1108];
    services.retain(|g| !matches!(short_id(g), Some(0x1800 | 0x1801)));
    services.sort_by_key(|g| FIRST.iter().position(|&f| short_id(g) == Some(f)).unwrap_or(FIRST.len()));
    services
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_classic_id_carries_the_devices_address() {
        // as Windows names the paired earbuds of this machine
        assert_eq!(classic_address("Bluetooth#Bluetooth70:a6:cc:81:41:82-88:92:cc:e9:4e:c6"), Some(0x8892_CCE9_4EC6));
        assert_eq!(classic_address("Bluetooth#Bluetooth70:a6:cc:81:41:82-08:12:87:52:2c:89"), Some(0x0812_8752_2C89));
    }

    #[test]
    fn only_classic_ids_have_an_address() {
        assert_eq!(classic_address("BluetoothLE#BluetoothLE70:a6:cc:81:41:82-7d:f7:bf:a5:d0:ce"), None, "LE: Windows connects those itself");
        assert_eq!(classic_address("Bluetooth#Bluetooth70:a6:cc:81:41:82-88:92:cc"), None, "too short");
        assert_eq!(classic_address("Bluetooth#Bluetooth70:a6:cc:81:41:82-zz:92:cc:e9:4e:c6"), None, "not hex");
        assert_eq!(classic_address(""), None);
        assert_eq!(classic_address("USB\\VID_046D"), None);
    }

    #[test]
    fn an_id_names_the_radio_it_was_paired_with() {
        assert_eq!(radio_of("Bluetooth#Bluetooth70:a6:cc:81:41:82-88:92:cc:e9:4e:c6"), Some(0x70A6_CC81_4182));
        assert_eq!(radio_of("BluetoothLE#BluetoothLE70:a6:cc:7f:d4:d2-5e:d0:19:27:71:23"), Some(0x70A6_CC7F_D4D2), "LE ids too");
        assert_eq!(radio_of("USB\\VID_046D"), None);
        assert_eq!(radio_of("Bluetooth#Bluetooth70:a6-88:92:cc:e9:4e:c6"), None);
    }

    #[test]
    fn the_audio_service_is_switched_first_and_the_gatt_ones_never() {
        let std = |n: u32| GUID::from_u128(((n as u128) << 96) | 0x0000_1000_8000_0080_5F9B_34FB);
        // the services of the earbuds on this machine, in the order Windows lists them
        let vendor = GUID::from_u128(0x0000079A_D102_11E1_9B23_00025B00A5A5);
        let listed = vec![vendor, std(0x110B), std(0x110C), std(0x110E), std(0x111E), std(0x1800), std(0x1801)];
        let ordered: Vec<Option<u32>> = order(listed).iter().map(short_id).collect();
        assert_eq!(ordered, [Some(0x110B), Some(0x111E), None, Some(0x110C), Some(0x110E)], "A2DP, hands-free, then the rest as listed");
        let mouse = order(vec![std(0x1200), std(0x1124)]);
        assert_eq!(short_id(&mouse[0]), Some(0x1124), "a mouse or keyboard: HID first");
    }

    #[test]
    fn the_disconnect_request_is_the_drivers_code() {
        // CTL_CODE(0x41, 3, METHOD_BUFFERED = 0, FILE_ANY_ACCESS = 0): device << 16 | function << 2
        assert_eq!(DISCONNECT, (0x41 << 16) | (3 << 2));
    }
}
