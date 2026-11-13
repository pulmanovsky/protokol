use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait};

use crate::audio::devices::configuration::{AudioDevice, DeviceType};

/// Configure macOS audio devices using ScreenCaptureKit and CoreAudio
pub fn configure_macos_audio(host: &cpal::Host) -> Result<Vec<AudioDevice>> {
    let mut devices: Vec<AudioDevice> = Vec::new();

    // Existing macOS implementation
    // Each device checked with a time limit (devices/lookup.rs)
    for name in crate::audio::devices::lookup::device_names(host, true) {
        devices.push(AudioDevice::new(name, DeviceType::Input));
    }

    // Filter function to exclude macOS built-in speakers for output devices
    // NOTE: AirPods and other Bluetooth devices are now allowed (with device monitoring for disconnect handling)
    fn should_include_output_device(name: &str) -> bool {
        // Only filter out built-in speakers (they don't typically capture system audio properly)
        !name.to_lowercase().contains("speakers")
    }

    // Use default host for all macOS output devices
    // Core Audio backend uses direct cidre API for system capture, not cpal
    for name in crate::audio::devices::lookup::device_names(host, false) {
        if should_include_output_device(&name) {
            devices.push(AudioDevice::new(name, DeviceType::Output));
        }
    }

    Ok(devices)
}