// Audio device management module
// Re-exports all device-related functionality to preserve API surface

pub mod capture_guard;
pub mod discovery;
pub mod microphone;
pub mod speakers;
pub mod configuration;
pub mod platform;
pub mod fallback;

// Re-export all public functions to preserve existing API
pub use discovery::{list_audio_devices, trigger_audio_permission};
pub use microphone::{
    default_input_device, find_builtin_input_device, find_linux_meeting_microphone,
    resolve_recording_microphone,
};
pub use speakers::{default_output_device, find_builtin_output_device};
pub use capture_guard::{
    is_generic_capture_alias, is_linux_meeting_microphone, is_linux_onboard_capture,
    is_pulse_monitor_name, is_raw_alsa_pcm_name, linux_device_lookup_names,
    system_audio_conflicts_with_microphone,
};
pub use configuration::{get_device_and_config, parse_audio_device, AudioDevice, DeviceType, DeviceControl, AudioTranscriptionEngine, LAST_AUDIO_CAPTURE};

// Re-export fallback functions (platform-specific)
#[cfg(target_os = "macos")]
pub use fallback::get_safe_recording_devices_macos;

#[cfg(not(target_os = "macos"))]
pub use fallback::get_safe_recording_devices;