/// Helpers that keep Linux microphone capture from being opened twice
/// as both "mic" and "system audio".

pub fn is_generic_capture_alias(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "default" | "pulse" | "sysdefault" | "pulse:default"
    )
}

pub fn is_pulse_monitor_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("monitor") && !lower.contains("hdmiin")
}

pub fn system_audio_conflicts_with_microphone(mic_name: &str, system_name: &str) -> bool {
    let mic = mic_name.trim().to_ascii_lowercase();
    let system = system_name.trim().to_ascii_lowercase();
    if mic == system {
        return true;
    }
    is_generic_capture_alias(&mic) && is_generic_capture_alias(&system) && !is_pulse_monitor_name(&system)
}

pub fn is_linux_onboard_capture(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("rt5651") || lower.contains("cb063")
}

/// Raw ALSA PCM names. Pulse already owns the USB card, so opening these
/// either fails (`unable to open slave`) or captures silence.
pub fn is_raw_alsa_pcm_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    let prefixes = [
        "dsnoop:",
        "hw:",
        "plughw:",
        "sysdefault:",
        "front:",
        "surround:",
        "iec958:",
        "hdmi:",
        "usbstream:",
    ];
    prefixes.iter().any(|prefix| lower.starts_with(prefix))
        || matches!(
            lower.as_str(),
            "null" | "samplerate" | "speexrate" | "jack" | "oss" | "upmix" | "vdownmix"
        )
}

pub fn is_pulse_passthrough_pcm(name: &str) -> bool {
    matches!(name.trim().to_ascii_lowercase().as_str(), "default" | "pulse")
}

pub fn is_linux_meeting_microphone(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if is_raw_alsa_pcm_name(&lower) || is_pulse_monitor_name(&lower) || lower.contains("hdmi") {
        return false;
    }
    lower.contains("powerconf")
        || lower.contains("mn108")
        || (lower.contains("usb") && (lower.contains("alsa_input") || lower.contains("uac")))
}

pub fn meeting_microphone_score(name: &str) -> u8 {
    let lower = name.to_ascii_lowercase();
    if lower.contains("powerconf") || lower.contains("mn108") {
        if lower.contains("alsa_input") { 3 } else { 2 }
    } else { 1 }
}

/// Select a Pulse source without opening a device or changing the system default.
/// Match recording's USB preference for automatic and onboard selections.
pub fn resolve_pulse_microphone_source(
    requested: Option<&str>, sources: &[String], default_source: Option<&str>,
) -> Result<String, String> {
    let usable = |name: &str| {
        !name.is_empty() && !is_pulse_monitor_name(name)
            && !name.to_ascii_lowercase().contains("hdmi")
            && !name.starts_with("alsa_output.") && !is_raw_alsa_pcm_name(name)
            && !is_generic_capture_alias(name)
    };
    let mut inputs: Vec<&str> = sources.iter().map(String::as_str).filter(|name| usable(name)).collect();
    inputs.sort_unstable();
    inputs.dedup();
    let requested = requested.map(str::trim).filter(|name| !name.is_empty());
    let names = requested.map(linux_device_lookup_names).unwrap_or_default();
    if names.iter().any(|name| !usable(name) && !is_generic_capture_alias(name)) {
        return Err("请选择物理麦克风，不能使用扬声器监视或 HDMI 输入".into());
    }
    let automatic = requested.is_none() || names.iter().any(|name| is_generic_capture_alias(name) || is_linux_onboard_capture(name));
    if automatic {
        if let Some(source) = inputs.iter().filter(|name| is_linux_meeting_microphone(name))
            .max_by(|a, b| meeting_microphone_score(a).cmp(&meeting_microphone_score(b)).then_with(|| b.cmp(a))) {
            return Ok((*source).to_string());
        }
    }
    if let Some(source) = inputs.iter().find(|name| names.iter().any(|candidate| candidate == **name)) {
        return Ok((*source).to_string());
    }
    if !automatic { return Err("所选麦克风当前不可用，请重新选择会议麦克风".into()); }
    if let Some(source) = default_source.filter(|name| inputs.contains(name)) {
        return Ok(source.to_string());
    }
    inputs.first().map(|name| name.to_string()).ok_or_else(|| "未找到可用的物理麦克风".into())
}

pub fn list_pulse_input_source_names() -> Vec<String> {
    let output = match std::process::Command::new("pactl")
        .args(["list", "short", "sources"])
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let name = line.split_whitespace().nth(1)?;
            if is_pulse_monitor_name(name) {
                None
            } else {
                Some(name.to_string())
            }
        })
        .collect()
}

pub fn set_pulse_default_source(name: &str) {
    let trimmed = name.trim();
    if trimmed.is_empty() || !trimmed.starts_with("alsa_input.") {
        return;
    }
    let _ = std::process::Command::new("pactl")
        .args(["set-default-source", trimmed])
        .status();
}

pub fn linux_device_lookup_names(name: &str) -> Vec<String> {
    let trimmed = name.trim().to_string();
    let mut names = vec![trimmed.clone()];
    for suffix in [" (System Audio)", " (input)", " (output)", " (Input)", " (Output)"] {
        if let Some(stripped) = trimmed.strip_suffix(suffix) {
            names.push(stripped.trim().to_string());
        }
    }
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_source_selection_matches_conference_preference() {
        let board = "alsa_input.platform-rt5651.source";
        let usb = "alsa_input.usb-PowerConf.mono";
        let other = "alsa_input.usb-other.mono";
        let sources: Vec<String> = [board, other, usb, "sink.monitor", "alsa_input.hdmiin.source"].iter().map(|s| s.to_string()).collect();
        for request in [None, Some("default (input)"), Some(board)] {
            assert_eq!(resolve_pulse_microphone_source(request, &sources, Some(board)).unwrap(), usb);
        }
        assert_eq!(resolve_pulse_microphone_source(Some(&format!("{other} (input)")), &sources, Some(board)).unwrap(), other);
        assert_eq!(resolve_pulse_microphone_source(None, &[board.into()], Some("sink.monitor")).unwrap(), board);
        assert!(resolve_pulse_microphone_source(Some("alsa_input.usb-unplugged"), &sources, Some(board)).is_err());
        for invalid in ["sink.monitor", "alsa_output.rt5651.sink.monitor", "alsa_input.hdmiin.source", "hw:3"] {
            assert!(resolve_pulse_microphone_source(Some(invalid), &sources, Some(board)).is_err(), "{invalid}");
        }
        assert!(resolve_pulse_microphone_source(None, &["sink.monitor".into(), "alsa_input.hdmiin.source".into()], None).is_err());
        let mut reversed = sources.clone(); reversed.reverse();
        assert_eq!(resolve_pulse_microphone_source(None, &sources, None), resolve_pulse_microphone_source(None, &reversed, None));
    }

    #[test]
    fn default_aliases_conflict() {
        assert!(system_audio_conflicts_with_microphone("default", "default"));
        assert!(system_audio_conflicts_with_microphone("pulse", "default"));
        assert!(!system_audio_conflicts_with_microphone(
            "default",
            "alsa_output.platform-rt5651-sound.HiFi__hw_cb063rt5651__sink.monitor"
        ));
        assert!(!system_audio_conflicts_with_microphone(
            "USB Mic",
            "Built-in Audio Analog Stereo"
        ));
    }

    #[test]
    fn monitor_names_are_detected() {
        assert!(is_pulse_monitor_name(
            "alsa_output.platform-rt5651-sound.HiFi__hw_cb063rt5651__sink.monitor"
        ));
        assert!(!is_pulse_monitor_name(
            "alsa_input.platform-hdmiin0-sound.HDMIIN__hw_hdmiin0__source"
        ));
    }

    #[test]
    fn lookup_names_strip_system_audio_suffix() {
        let names = linux_device_lookup_names("sink.monitor (System Audio)");
        assert!(names.iter().any(|name| name == "sink.monitor"));
    }

    #[test]
    fn usb_conference_mic_is_preferred_over_onboard() {
        assert!(is_linux_meeting_microphone(
            "alsa_input.usb-MN108_0.0.3__MN108_0.0.3__PowerConf_88AB88CD-00.mono-fallback"
        ));
        assert!(is_linux_onboard_capture(
            "alsa_input.platform-rt5651-sound.HiFi__hw_cb063rt5651__source"
        ));
        assert!(!is_linux_meeting_microphone(
            "alsa_input.platform-rt5651-sound.HiFi__hw_cb063rt5651__source"
        ));
        assert!(!is_linux_meeting_microphone(
            "alsa_output.platform-hdmi0-sound.HDMI__hw_rockchiphdmi0__sink.monitor"
        ));
        assert!(!is_linux_meeting_microphone("dsnoop:CARD=PowerConf,DEV=0"));
        assert!(is_raw_alsa_pcm_name("dsnoop:CARD=PowerConf,DEV=0"));
        assert!(is_raw_alsa_pcm_name("hw:CARD=PowerConf,DEV=0"));
        assert!(is_pulse_passthrough_pcm("default"));
        assert!(!is_raw_alsa_pcm_name("default"));
    }
}
