use anyhow::{anyhow, Result};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::atomic::AtomicU64;

lazy_static! {
    pub static ref LAST_AUDIO_CAPTURE: AtomicU64 = AtomicU64::new(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    );
}

#[derive(Clone, Debug, PartialEq)]
pub enum AudioTranscriptionEngine {
    Deepgram,
    WhisperTiny,
    WhisperDistilLargeV3,
    WhisperLargeV3Turbo,
    WhisperLargeV3,
}

impl fmt::Display for AudioTranscriptionEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioTranscriptionEngine::Deepgram => write!(f, "Deepgram"),
            AudioTranscriptionEngine::WhisperTiny => write!(f, "WhisperTiny"),
            AudioTranscriptionEngine::WhisperDistilLargeV3 => write!(f, "WhisperLarge"),
            AudioTranscriptionEngine::WhisperLargeV3Turbo => write!(f, "WhisperLargeV3Turbo"),
            AudioTranscriptionEngine::WhisperLargeV3 => write!(f, "WhisperLargeV3"),
        }
    }
}

impl Default for AudioTranscriptionEngine {
    fn default() -> Self {
        AudioTranscriptionEngine::WhisperLargeV3Turbo
    }
}

#[derive(Clone, Debug)]
pub struct DeviceControl {
    pub is_running: bool,
    pub is_paused: bool,
}

#[derive(Clone, Eq, PartialEq, Hash, Serialize, Debug, Deserialize)]
pub enum DeviceType {
    Input,
    Output,
}

#[derive(Clone, Eq, PartialEq, Hash, Serialize, Debug)]
pub struct AudioDevice {
    pub name: String,
    pub device_type: DeviceType,
}

impl AudioDevice {
    pub fn new(name: String, device_type: DeviceType) -> Self {
        AudioDevice { name, device_type }
    }

    pub fn from_name(name: &str) -> Result<Self> {
        if name.trim().is_empty() {
            return Err(anyhow!("Device name cannot be empty"));
        }

        let (name, device_type) = if name.to_lowercase().ends_with("(input)") {
            (
                name.trim_end_matches("(input)").trim().to_string(),
                DeviceType::Input,
            )
        } else if name.to_lowercase().ends_with("(output)") {
            (
                name.trim_end_matches("(output)").trim().to_string(),
                DeviceType::Output,
            )
        } else {
            return Err(anyhow!(
                "Device type (input/output) not specified in the name"
            ));
        };

        Ok(AudioDevice::new(name, device_type))
    }
}

impl fmt::Display for AudioDevice {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{} ({})",
            self.name,
            match self.device_type {
                DeviceType::Input => "input",
                DeviceType::Output => "output",
            }
        )
    }
}

/// Parse audio device from string name
pub fn parse_audio_device(name: &str) -> Result<AudioDevice> {
    AudioDevice::from_name(name)
}

/// Get device and config for audio operations
pub async fn get_device_and_config(
    audio_device: &AudioDevice,
) -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    #[cfg(target_os = "windows")]
    {
        return super::platform::get_windows_device(audio_device);
    }

    #[cfg(not(target_os = "windows"))]
    {
        use cpal::traits::{DeviceTrait, HostTrait};

        let host = cpal::default_host();

        match audio_device.device_type {
            DeviceType::Input => {
                #[cfg(target_os = "linux")]
                {
                    if let Some(found) = open_linux_input_device(&host, audio_device)? {
                        return Ok(found);
                    }
                }

                for device in host.input_devices()? {
                    if let Ok(name) = device.name() {
                        if name == audio_device.name {
                            let default_config = device
                                .default_input_config()
                                .map_err(|e| anyhow!("Failed to get default input config: {}", e))?;
                            return Ok((device, default_config));
                        }
                    }
                }
            }
            DeviceType::Output => {
                #[cfg(target_os = "macos")]
                {
                    // Use default host for all macOS output devices
                    // Core Audio backend uses direct cidre API for system capture, not cpal
                    for device in host.output_devices()? {
                        if let Ok(name) = device.name() {
                            if name == audio_device.name {
                                let default_config = device
                                    .default_output_config()
                                    .map_err(|e| anyhow!("Failed to get output config: {}", e))?;
                                return Ok((device, default_config));
                            }
                        }
                    }
                }

                #[cfg(target_os = "linux")]
                {
                    use super::capture_guard::{
                        is_generic_capture_alias, is_pulse_monitor_name, linux_device_lookup_names,
                    };

                    let lookup_names = linux_device_lookup_names(&audio_device.name);
                    if lookup_names.iter().any(|name| {
                        is_generic_capture_alias(name) && !is_pulse_monitor_name(name)
                    }) {
                        return Err(anyhow!(
                            "Refusing to capture system audio from generic input alias '{}'",
                            audio_device.name
                        ));
                    }

                    if let Some(found) = find_linux_system_capture_device(&host, &lookup_names)? {
                        return Ok(found);
                    }
                    if let Ok(alsa_host) = cpal::host_from_id(cpal::HostId::Alsa) {
                        if let Some(found) = find_linux_system_capture_device(&alsa_host, &lookup_names)? {
                            return Ok(found);
                        }
                    }
                }
            }
        }

        Err(anyhow!("Device not found: {}", audio_device.name))
    }
}

#[cfg(target_os = "linux")]
fn find_linux_system_capture_device(
    host: &cpal::Host,
    lookup_names: &[String],
) -> Result<Option<(cpal::Device, cpal::SupportedStreamConfig)>> {
    use cpal::traits::{DeviceTrait, HostTrait};
    use super::capture_guard::is_pulse_monitor_name;

    for device in host.input_devices()? {
        let Ok(name) = device.name() else {
            continue;
        };
        let matches_requested = lookup_names.iter().any(|wanted| wanted == &name);
        let matches_monitor = is_pulse_monitor_name(&name)
            && lookup_names
                .iter()
                .any(|wanted| name.contains(wanted) || wanted.contains(&name));
        if matches_requested || matches_monitor {
            let default_config = device
                .default_input_config()
                .map_err(|e| anyhow!("Failed to get default input config: {}", e))?;
            return Ok(Some((device, default_config)));
        }
    }
    Ok(None)
}

#[cfg(target_os = "linux")]
fn open_linux_input_device(
    host: &cpal::Host,
    audio_device: &AudioDevice,
) -> Result<Option<(cpal::Device, cpal::SupportedStreamConfig)>> {
    use super::capture_guard::{
        is_linux_meeting_microphone, is_pulse_passthrough_pcm, is_raw_alsa_pcm_name,
        linux_device_lookup_names, set_pulse_default_source,
    };
    use cpal::traits::{DeviceTrait, HostTrait};

    let lookup_names = linux_device_lookup_names(&audio_device.name);
    let mut passthrough = None;
    for device in host.input_devices()? {
        let Ok(name) = device.name() else {
            continue;
        };
        if lookup_names.iter().any(|wanted| wanted == &name) && !is_raw_alsa_pcm_name(&name) {
            let default_config = device
                .default_input_config()
                .map_err(|e| anyhow!("Failed to get default input config: {}", e))?;
            return Ok(Some((device, default_config)));
        }
        if passthrough.is_none() && is_pulse_passthrough_pcm(&name) {
            passthrough = Some(device);
        }
    }

    let wants_pulse = lookup_names.iter().any(|name| {
        name.starts_with("alsa_input.") || is_linux_meeting_microphone(name)
    });
    if wants_pulse {
        if let Some(name) = lookup_names.iter().find(|name| name.starts_with("alsa_input.")) {
            set_pulse_default_source(name);
        }
        if let Some(device) = passthrough {
            let opened = device
                .name()
                .unwrap_or_else(|_| "pulse-default".to_string());
            log::info!(
                "Opening Pulse capture '{}' via ALSA device '{}'",
                audio_device.name,
                opened
            );
            let default_config = device
                .default_input_config()
                .map_err(|e| anyhow!("Failed to get default input config: {}", e))?;
            return Ok(Some((device, default_config)));
        }
    }
    Ok(None)
}