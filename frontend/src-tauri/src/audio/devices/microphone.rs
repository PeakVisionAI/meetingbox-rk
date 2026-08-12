use anyhow::{anyhow, Result};
use cpal::traits::{HostTrait, DeviceTrait};
use log::{info, warn};

use super::capture_guard::{is_generic_capture_alias, is_linux_onboard_capture};
#[cfg(target_os = "linux")]
use super::capture_guard::{
    is_linux_meeting_microphone, is_raw_alsa_pcm_name, list_pulse_input_source_names,
    set_pulse_default_source,
};
use super::configuration::{parse_audio_device, AudioDevice, DeviceType};

/// Get the default input (microphone) device for the system
pub fn default_input_device() -> Result<AudioDevice> {
    if let Some(meeting_mic) = find_linux_meeting_microphone()? {
        info!("Using USB meeting microphone: {}", meeting_mic.name);
        return Ok(meeting_mic);
    }

    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("No default input device found"))?;
    Ok(AudioDevice::new(device.name()?, DeviceType::Input))
}

/// Prefer a USB conference microphone on Linux when one is plugged in.
pub fn find_linux_meeting_microphone() -> Result<Option<AudioDevice>> {
    #[cfg(not(target_os = "linux"))]
    {
        return Ok(None);
    }

    #[cfg(target_os = "linux")]
    {
        let mut ranked: Vec<(u8, String)> = Vec::new();
        for name in list_pulse_input_source_names() {
            if !is_linux_meeting_microphone(&name) {
                continue;
            }
            ranked.push((meeting_mic_score(&name), name));
        }
        let host = cpal::default_host();
        for device in host.input_devices()? {
            let Ok(name) = device.name() else {
                continue;
            };
            if is_raw_alsa_pcm_name(&name) || !is_linux_meeting_microphone(&name) {
                continue;
            }
            if ranked.iter().any(|(_, existing)| existing == &name) {
                continue;
            }
            ranked.push((meeting_mic_score(&name), name));
        }
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        if let Some((_, name)) = ranked.into_iter().next() {
            set_pulse_default_source(&name);
            return Ok(Some(AudioDevice::new(name, DeviceType::Input)));
        }
        Ok(None)
    }
}

#[cfg(target_os = "linux")]
fn meeting_mic_score(name: &str) -> u8 {
    let lower = name.to_ascii_lowercase();
    if lower.contains("powerconf") || lower.contains("mn108") {
        if lower.contains("alsa_input") {
            3
        } else {
            2
        }
    } else {
        1
    }
}

fn parse_requested_input(name: &str) -> Result<AudioDevice> {
    parse_audio_device(name).or_else(|_| {
        Ok(AudioDevice::new(name.trim().to_string(), DeviceType::Input))
    })
}

/// Resolve the microphone for a recording session.
/// On Linux, onboard analog / Pulse "default" is replaced by a USB meeting mic when present.
pub fn resolve_recording_microphone(requested: Option<&str>) -> Result<AudioDevice> {
    let parsed = match requested.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => Some(parse_requested_input(name)?),
        None => None,
    };

    let should_use_meeting_mic = match &parsed {
        None => true,
        Some(device) => {
            is_generic_capture_alias(&device.name) || is_linux_onboard_capture(&device.name)
        }
    };

    if should_use_meeting_mic {
        if let Some(meeting_mic) = find_linux_meeting_microphone()? {
            info!(
                "Linux meeting microphone selected: {} (requested={:?})",
                meeting_mic.name,
                requested
            );
            return Ok(meeting_mic);
        }
    }

    if let Some(device) = parsed {
        return Ok(device);
    }

    default_input_device()
}

/// Find the built-in microphone device (wired, stable, consistent sample rate)
///
/// Searches for MacBook/built-in microphone patterns to find the hardware
/// microphone instead of Bluetooth devices. This is useful for:
/// - Avoiding Bluetooth variable sample rate issues
/// - Getting stable wired audio for recording
/// - Fallback when Bluetooth device is default but unreliable
///
/// Returns None if no built-in microphone found
pub fn find_builtin_input_device() -> Result<Option<AudioDevice>> {
    let host = cpal::default_host();

    // Built-in microphone name patterns (platform-specific)
    let builtin_patterns = [
        // macOS patterns
        "macbook",
        "built-in microphone",
        "internal microphone",
        // Windows patterns
        "microphone array",
        "realtek",
        "conexant",
        // Linux patterns
        "hda intel",
        "built-in audio",
    ];

    // Search all input devices for built-in pattern matches
    for device in host.input_devices()? {
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

                    info!("🎤 Found built-in microphone: '{}'", name);
                    return Ok(Some(AudioDevice::new(name, DeviceType::Input)));
                }
            }
        }
    }

    warn!("⚠️ No built-in microphone found (searched {} patterns)", builtin_patterns.len());
    Ok(None)
}