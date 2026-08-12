use anyhow::{anyhow, Result};
use cpal::traits::{HostTrait, DeviceTrait};
use log::{info, warn};

use super::configuration::{AudioDevice, DeviceType};

/// Get the default output (speaker/system audio) device for the system
pub fn default_output_device() -> Result<AudioDevice> {
    #[cfg(target_os = "macos")]
    {
        // Use default host for all macOS devices
        // Core Audio backend uses direct cidre API for system capture, not cpal
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("No default output device found"))?;
        return Ok(AudioDevice::new(device.name()?, DeviceType::Output));
    }

    #[cfg(target_os = "windows")]
    {
        // Try WASAPI host first for Windows
        if let Ok(wasapi_host) = cpal::host_from_id(cpal::HostId::Wasapi) {
            if let Some(device) = wasapi_host.default_output_device() {
                if let Ok(name) = device.name() {
                    return Ok(AudioDevice::new(name, DeviceType::Output));
                }
            }
        }
        // Fallback to default host if WASAPI fails
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("No default output device found"))?;
        return Ok(AudioDevice::new(device.name()?, DeviceType::Output));
    }

    #[cfg(target_os = "linux")]
    {
        match find_linux_system_audio_device() {
            Some(device) => {
                info!("🔊 Using Pulse monitor for system audio: '{}'", device.name);
                Ok(device)
            }
            None => {
                warn!("⚠️ No PulseAudio monitor source found; system audio capture disabled");
                Err(anyhow!(
                    "No PulseAudio monitor source found for system audio capture"
                ))
            }
        }
    }
}

/// Find the built-in speaker/output device (wired, stable, consistent sample rate)
///
/// Searches for MacBook/built-in speaker patterns to find the hardware
/// speakers instead of Bluetooth devices. This is useful for:
/// - System audio capture using ScreenCaptureKit (macOS) with consistent sample rates
/// - Getting audio before Bluetooth processing (pristine quality)
/// - Fallback when Bluetooth device is default but causes sample rate issues
///
/// Note: On macOS, system audio is captured via ScreenCaptureKit from the
/// output device. Using built-in speakers ensures Core Audio provides
/// consistent sample rates for reliable mixing with microphone.
///
/// Returns None if no built-in speaker found
pub fn find_builtin_output_device() -> Result<Option<AudioDevice>> {
    let host = cpal::default_host();

    // Built-in speaker name patterns (platform-specific)
    let builtin_patterns = [
        // macOS patterns
        "macbook",
        "built-in speakers",
        "built-in output",
        "internal speakers",
        // Windows patterns
        "speakers",
        "realtek",
        "conexant",
        "high definition audio",
        // Linux patterns
        "hda intel",
        "built-in audio",
        "analog output",
    ];

    // Search all output devices for built-in pattern matches
    for device in host.output_devices()? {
        if let Ok(name) = device.name() {
            let name_lower = name.to_lowercase();

            // Check if this is a built-in device
            for pattern in &builtin_patterns {
                if name_lower.contains(pattern) {
                    // Additional filter: exclude Bluetooth/wireless devices
                    if name_lower.contains("bluetooth") ||
                       name_lower.contains("airpods") ||
                       name_lower.contains("wireless") {
                        continue; // Skip Bluetooth devices
                    }

                    // Additional filter: exclude virtual audio devices
                    // (we want real hardware speakers for ScreenCaptureKit)
                    if name_lower.contains("blackhole") ||
                       name_lower.contains("vb-audio") ||
                       name_lower.contains("virtual") ||
                       name_lower.contains("loopback") {
                        continue; // Skip virtual devices
                    }

                    info!("🔊 Found built-in speaker: '{}'", name);
                    return Ok(Some(AudioDevice::new(name, DeviceType::Output)));
                }
            }
        }
    }

    warn!("⚠️ No built-in speaker found (searched {} patterns)", builtin_patterns.len());
    Ok(None)
}

#[cfg(target_os = "linux")]
fn find_linux_system_audio_device() -> Option<AudioDevice> {
    use super::capture_guard::{is_generic_capture_alias, is_pulse_monitor_name};

    let host = cpal::default_host();
    let input_names: Vec<String> = host
        .input_devices()
        .ok()?
        .filter_map(|device| device.name().ok())
        .collect();

    if let Some(monitor) = pulse_default_sink_monitor() {
        if let Some(matched) = input_names.iter().find(|name| {
            *name == &monitor || name.contains(&monitor) || monitor.contains(*name)
        }) {
            return Some(AudioDevice::new(matched.clone(), DeviceType::Output));
        }
        info!("Pulse default sink monitor is '{}', but CPAL does not expose it", monitor);
    }

    input_names
        .into_iter()
        .find(|name| is_pulse_monitor_name(name) && !is_generic_capture_alias(name))
        .map(|name| AudioDevice::new(name, DeviceType::Output))
}

#[cfg(target_os = "linux")]
fn pulse_default_sink_monitor() -> Option<String> {
    let output = std::process::Command::new("pactl")
        .args(["get-default-sink"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sink = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sink.is_empty() {
        return None;
    }
    Some(format!("{sink}.monitor"))
}