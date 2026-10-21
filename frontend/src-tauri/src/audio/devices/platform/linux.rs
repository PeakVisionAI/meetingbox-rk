use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait};

use crate::audio::devices::capture_guard::{
    is_pulse_monitor_name, is_raw_alsa_pcm_name, list_pulse_input_source_names,
};
use crate::audio::devices::configuration::{AudioDevice, DeviceType};

/// Configure Linux audio devices using ALSA/PulseAudio
pub fn configure_linux_audio(host: &cpal::Host) -> Result<Vec<AudioDevice>> {
    let mut devices = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for name in list_pulse_input_source_names() {
        if seen.insert(name.clone()) {
            devices.push(AudioDevice::new(name, DeviceType::Input));
        }
    }

    for device in host.input_devices()? {
        if let Ok(name) = device.name() {
            if is_raw_alsa_pcm_name(&name) {
                continue;
            }
            if seen.insert(name.clone()) {
                devices.push(AudioDevice::new(name, DeviceType::Input));
            }
        }
    }

    // Add PulseAudio monitor sources for system audio without renaming them.
    // The previous "(System Audio)" suffix made later CPAL lookups miss the device.
    if let Ok(pulse_host) = cpal::host_from_id(cpal::HostId::Alsa) {
        for device in pulse_host.input_devices()? {
            if let Ok(name) = device.name() {
                if is_pulse_monitor_name(&name)
                    && !is_raw_alsa_pcm_name(&name)
                    && seen.insert(name.clone())
                {
                    devices.push(AudioDevice::new(name, DeviceType::Output));
                }
            }
        }
    }

    Ok(devices)
}