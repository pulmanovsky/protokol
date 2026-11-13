//! Finding audio devices without opening every one of them.
//!
//! cpal's `input_devices()` / `output_devices()` open each device on the Mac
//! to see whether it has inputs or outputs. A third-party virtual driver whose
//! app is not running (Microsoft Teams Audio, ZoomAudioDevice, Steam
//! Streaming …) can stop answering, and opening it then blocks forever: the
//! recording start hung at 00:00:00 on such a Mac. Here a device is found by
//! its name (a property read, which such drivers still answer) and only that
//! one device is opened; lists check each device on a side thread with a
//! time limit and leave out one that does not answer.

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
/// macOS allows the microphone, checking an input waits for that answer,
/// so only the default microphone is listed then.
pub fn device_names(host: &cpal::Host, input: bool) -> Vec<String> {
    let default = if input { host.default_input_device() } else { host.default_output_device() }
        .and_then(|d| d.name().ok());
    let mut names: Vec<String> = default.iter().cloned().collect();
    if input && !microphone_allowed() {
        return names;
    }
    let Ok(devices) = host.devices() else { return names };
    for device in devices {
        let Ok(name) = device.name() else { continue };
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
            let has = if input {
                device.supported_input_configs().map(|mut c| c.next().is_some()).unwrap_or(false)
            } else {
                device.supported_output_configs().map(|mut c| c.next().is_some()).unwrap_or(false)
            };
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
