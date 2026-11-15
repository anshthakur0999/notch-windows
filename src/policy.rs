//! The one undocumented thing Notch uses: `IPolicyConfig`, the COM interface behind Windows' own
//! sound settings. The documented audio APIs can read the default playback device but not change
//! it, so this is the only way to make another device the default from code (SoundSwitch and
//! NirCmd call the same method).
//!
//! It is not in Microsoft's headers, so a future Windows build may change it. If that happens
//! `set_default` fails and says so in the log; nothing else in Notch depends on it.

#![allow(non_snake_case)] // the method names are Windows' own

use windows::Win32::Media::Audio::{ERole, eCommunications, eConsole, eMultimedia};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, PCWSTR, interface};

const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

/// The methods before `SetDefaultEndpoint` are never called, but each one holds a slot in the
/// table, so they are listed with loose types to keep it in the right place.
#[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: IUnknown {
    unsafe fn GetMixFormat(&self, device: PCWSTR, format: *mut *mut core::ffi::c_void) -> HRESULT;
    unsafe fn GetDeviceFormat(&self, device: PCWSTR, default: i32, format: *mut *mut core::ffi::c_void) -> HRESULT;
    unsafe fn ResetDeviceFormat(&self, device: PCWSTR) -> HRESULT;
    unsafe fn SetDeviceFormat(&self, device: PCWSTR, endpoint: *mut core::ffi::c_void, mix: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn GetProcessingPeriod(&self, device: PCWSTR, default: i32, a: *mut i64, b: *mut i64) -> HRESULT;
    unsafe fn SetProcessingPeriod(&self, device: PCWSTR, period: *mut i64) -> HRESULT;
    unsafe fn GetShareMode(&self, device: PCWSTR, mode: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn SetShareMode(&self, device: PCWSTR, mode: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn GetPropertyValue(&self, device: PCWSTR, key: *const core::ffi::c_void, value: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn SetPropertyValue(&self, device: PCWSTR, key: *const core::ffi::c_void, value: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn SetDefaultEndpoint(&self, device: PCWSTR, role: ERole) -> HRESULT;
    unsafe fn SetEndpointVisibility(&self, device: PCWSTR, visible: i32) -> HRESULT;
}

/// Makes the playback device `id` the default for every role (apps, media, calls). `false` if
/// Windows refused.
pub fn set_default(id: &str) -> bool {
    let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let config: IPolicyConfig = match CoCreateInstance(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL) {
            Ok(c) => c,
            Err(e) => {
                crate::log(&format!("policy: no IPolicyConfig: {e}"));
                return false;
            }
        };
        let mut all = true;
        for role in [eConsole, eMultimedia, eCommunications] {
            let r = config.SetDefaultEndpoint(PCWSTR(wide.as_ptr()), role);
            if r.is_err() {
                crate::log(&format!("policy: SetDefaultEndpoint({role:?}) failed: {r:?}"));
                all = false;
            }
        }
        all
    }
}
