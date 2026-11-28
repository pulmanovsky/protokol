//! Finding audio devices without opening any of them.
//!
//! cpal's `input_devices()` / `output_devices()` and its
//! `supported_*_configs()` open each device on the Mac (an AudioUnit is made
//! for it) to see whether it has inputs or outputs. A third-party virtual
//! driver whose app is not running (Microsoft Teams Audio, ZoomAudioDevice,
//! Steam Streaming …) can stop answering when it is opened like that, and
//! then stays stuck for every app on the Mac: the recording start hung at
//! 00:00:00, and a call page waited for its device list without end. So on
//! the Mac a device is found by its name and the lists ask only for
//! properties (name, number of streams): the one device that is opened is
//! the one a recording uses. Each property read still runs on a side thread
//! with a time limit, and a device that does not answer is left out.

use std::collections::HashSet;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait};
use log::warn;
use once_cell::sync::Lazy;

/// How long one device may take to say whether it has inputs or outputs
const CHECK_LIMIT: Duration = Duration::from_millis(1500);

/// Devices that did not answer in time: left out until the app restarts
static STUCK: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));

fn is_stuck(name: &str) -> bool {
    STUCK.lock().map(|s| s.contains(name)).unwrap_or(false)
}

/// The device with this name. The system default is tried first, then all
/// devices by name only; `input` picks the direction the caller will use.
pub fn device_by_name(host: &cpal::Host, name: &str, input: bool) -> Option<cpal::Device> {
    let default = if input { host.default_input_device() } else { host.default_output_device() };
    if let Some(d) = default {
        if d.name().ok().as_deref() == Some(name) {
            return Some(d);
        }
    }
    if is_stuck(name) {
        warn!("Audio device '{}' did not answer earlier: not used", name);
        return None;
    }
    host.devices().ok()?.find(|d| d.name().ok().as_deref() == Some(name))
}

/// Runs `f` on a side thread and waits at most `limit`. None when it did not
/// finish in time (the thread is left behind, blocked in the driver).
pub fn within<T: Send + 'static>(limit: Duration, what: &str, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("audio-device-check".into())
        .spawn(move || {
            let _ = tx.send(f());
        });
    if spawned.is_err() {
        return None;
    }
    match rx.recv_timeout(limit) {
        Ok(v) => Some(v),
        Err(_) => {
            warn!("{} did not answer within {} ms: skipped (a virtual audio driver may be stuck)", what, limit.as_millis());
            None
        }
    }
}

/// Device checks still running (blocked in a driver): not started again, so
/// a stuck device costs one waiting thread, not one per list
static PROBING: Lazy<Mutex<HashSet<(String, bool)>>> = Lazy::new(|| Mutex::new(HashSet::new()));

/// Names of the devices with inputs (`input`) or outputs. The system default
/// is listed without a check; every other device is checked with a time
/// limit, and one that does not answer is left out and remembered. Until
/// macOS allows the microphone only the default microphone is listed.
pub fn device_names(host: &cpal::Host, input: bool) -> Vec<String> {
    let default = if input { host.default_input_device() } else { host.default_output_device() }
        .and_then(|d| d.name().ok());
    let mut names: Vec<String> = default.iter().cloned().collect();
    if input && !microphone_allowed() {
        return names;
    }
    for (name, check) in candidates(host, input) {
        if Some(&name) == default.as_ref() || is_stuck(&name) {
            continue;
        }
        let key = (name.clone(), input);
        {
            let Ok(mut probing) = PROBING.lock() else { continue };
            if !probing.insert(key.clone()) {
                continue;
            }
        }
        let what = format!("Audio device '{}'", name);
        let has = within(CHECK_LIMIT, &what, move || {
            let has = check();
            if let Ok(mut probing) = PROBING.lock() {
                probing.remove(&key);
            }
            has
        });
        match has {
            Some(true) => names.push(name),
            Some(false) => {}
            None => {
                if let Ok(mut s) = STUCK.lock() {
                    s.insert(name);
                }
            }
        }
    }
    names
}

/// Asks one device whether it has inputs or outputs
type Check = Box<dyn FnOnce() -> bool + Send>;

/// Every device with its name and the question to ask it. On the Mac the
/// question is a property read (how many streams the device has in that
/// direction): the device is not opened.
#[cfg(target_os = "macos")]
fn candidates(_host: &cpal::Host, input: bool) -> Vec<(String, Check)> {
    hal::devices()
        .into_iter()
        .filter_map(|id| Some((hal::name(id)?, Box::new(move || hal::streams(id, input) > 0) as Check)))
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn candidates(host: &cpal::Host, input: bool) -> Vec<(String, Check)> {
    let Ok(devices) = host.devices() else { return Vec::new() };
    devices
        .filter_map(|device| {
            let name = device.name().ok()?;
            let check: Check = Box::new(move || {
                if input {
                    device.supported_input_configs().map(|mut c| c.next().is_some()).unwrap_or(false)
                } else {
                    device.supported_output_configs().map(|mut c| c.next().is_some()).unwrap_or(false)
                }
            });
            Some((name, check))
        })
        .collect()
}

/// Core Audio property reads: what the Mac knows about a device, asked
/// without opening it
#[cfg(target_os = "macos")]
mod hal {
    use std::ffi::{c_void, CStr};
    use std::os::raw::c_char;

    #[repr(C)]
    struct Address {
        selector: u32,
        scope: u32,
        element: u32,
    }

    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectGetPropertyDataSize(
            object: u32,
            address: *const Address,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
        ) -> i32;
        fn AudioObjectGetPropertyData(
            object: u32,
            address: *const Address,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
            data: *mut c_void,
        ) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringGetCString(string: *const c_void, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFRelease(object: *const c_void);
    }

    const fn code(c: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*c)
    }

    /// kAudioObjectSystemObject
    const SYSTEM: u32 = 1;
    /// kAudioHardwarePropertyDevices
    const DEVICES: u32 = code(b"dev#");
    /// kAudioObjectPropertyName
    const NAME: u32 = code(b"lnam");
    /// kAudioDevicePropertyStreams
    const STREAMS: u32 = code(b"stm#");
    const GLOBAL: u32 = code(b"glob");
    const INPUT: u32 = code(b"inpt");
    const OUTPUT: u32 = code(b"outp");
    /// kCFStringEncodingUTF8
    const UTF8: u32 = 0x0800_0100;

    fn size(object: u32, address: &Address) -> Option<u32> {
        let mut size = 0u32;
        let status = unsafe { AudioObjectGetPropertyDataSize(object, address, 0, std::ptr::null(), &mut size) };
        (status == 0).then_some(size)
    }

    /// Ids of all audio devices
    pub fn devices() -> Vec<u32> {
        let address = Address { selector: DEVICES, scope: GLOBAL, element: 0 };
        let Some(bytes) = size(SYSTEM, &address) else { return Vec::new() };
        let mut ids = vec![0u32; bytes as usize / std::mem::size_of::<u32>()];
        if ids.is_empty() {
            return ids;
        }
        let mut bytes = (ids.len() * std::mem::size_of::<u32>()) as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(SYSTEM, &address, 0, std::ptr::null(), &mut bytes, ids.as_mut_ptr() as *mut c_void)
        };
        if status != 0 {
            return Vec::new();
        }
        ids.truncate(bytes as usize / std::mem::size_of::<u32>());
        ids
    }

    /// The device's name, the same one cpal gives
    pub fn name(device: u32) -> Option<String> {
        let address = Address { selector: NAME, scope: OUTPUT, element: 0 };
        let mut string: *const c_void = std::ptr::null();
        let mut bytes = std::mem::size_of::<*const c_void>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(device, &address, 0, std::ptr::null(), &mut bytes, &mut string as *mut _ as *mut c_void)
        };
        if status != 0 || string.is_null() {
            return None;
        }
        let mut buffer = [0 as c_char; 1024];
        let ok = unsafe { CFStringGetCString(string, buffer.as_mut_ptr(), buffer.len() as isize, UTF8) };
        unsafe { CFRelease(string) };
        if ok == 0 {
            return None;
        }
        Some(unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy().into_owned())
    }

    /// How many input (`input`) or output streams the device has
    pub fn streams(device: u32, input: bool) -> usize {
        let address = Address { selector: STREAMS, scope: if input { INPUT } else { OUTPUT }, element: 0 };
        size(device, &address).map_or(0, |bytes| bytes as usize / std::mem::size_of::<u32>())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn codes_are_the_core_audio_ones() {
            assert_eq!(DEVICES, 0x6465_7623);
            assert_eq!(NAME, 0x6C6E_616D);
            assert_eq!(STREAMS, 0x7374_6D23);
            assert_eq!(GLOBAL, 0x676C_6F62);
            assert_eq!(INPUT, 0x696E_7074);
            assert_eq!(OUTPUT, 0x6F75_7470);
        }

        /// The lists come back (each device within its time limit) and
        /// carry real names. Touches the real devices of the Mac, so it is
        /// run by hand: cargo test --lib lists_ask_for_properties_only -- --ignored
        #[test]
        #[ignore]
        fn lists_ask_for_properties_only() {
            let host = cpal::default_host();
            for input in [false, true] {
                let names = super::super::device_names(&host, input);
                assert!(names.iter().all(|n| !n.is_empty()));
            }
        }
    }
}

/// macOS's answer for the microphone: Some(true) allowed, Some(false) denied
/// or restricted, None not asked yet (or unknown)
#[cfg(target_os = "macos")]
#[link(name = "AVFoundation", kind = "framework")]
extern "C" {}

#[cfg(target_os = "macos")]
pub fn microphone_permission() -> Option<bool> {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};
    let device_class = AnyClass::get(c"AVCaptureDevice")?;
    let string_class = AnyClass::get(c"NSString")?;
    // AVMediaTypeAudio
    let media: *mut AnyObject = unsafe { msg_send![string_class, stringWithUTF8String: c"soun".as_ptr()] };
    if media.is_null() {
        return None;
    }
    // AVAuthorizationStatus: 0 not determined, 1 restricted, 2 denied, 3 authorized
    let status: isize = unsafe { msg_send![device_class, authorizationStatusForMediaType: media] };
    match status {
        3 => Some(true),
        1 | 2 => Some(false),
        _ => None,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn microphone_permission() -> Option<bool> {
    Some(true)
}

/// The microphone may be opened without waiting for macOS to ask
fn microphone_allowed() -> bool {
    microphone_permission() == Some(true)
}
