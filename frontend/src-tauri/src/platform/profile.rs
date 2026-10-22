//! Runtime platform profile.
//!
//! One codebase ships two deployment shapes:
//!
//! * `intel` — Windows desktop app talking to a VM/host service stack
//!   (asrserve bridge, 35B summary, punctuation).
//! * `rk` — self-contained RK3588/RK1828 device where every service runs on
//!   localhost and the UI exposes device power controls.
//!
//! The profile is resolved once at startup and is the single source of truth for
//! service endpoints, paths and UI capabilities. Nothing below the UI should
//! hard-code an IP address or a Windows path.

use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::OnceLock;

pub static CURRENT: OnceLock<Profile> = OnceLock::new();

const INTEL_JSON: &str = include_str!("../../config/profile-intel.json");
const RK_JSON: &str = include_str!("../../config/profile-rk.json");
const DEV_JSON: &str = include_str!("../../config/profile-dev.json");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileKind {
    Intel,
    Rk,
    Dev,
}

impl ProfileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intel => "intel",
            Self::Rk => "rk",
            Self::Dev => "dev",
        }
    }

    fn embedded(self) -> &'static str {
        match self {
            Self::Intel => INTEL_JSON,
            Self::Rk => RK_JSON,
            Self::Dev => DEV_JSON,
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "intel" | "windows" | "x86" => Some(Self::Intel),
            "rk" | "rk3588" | "rk1828" | "rockchip" | "npu" => Some(Self::Rk),
            "dev" | "macos" | "whisper" => Some(Self::Dev),
            _ => None,
        }
    }
}

/// One service the app can probe before recording starts.
#[derive(Clone, Debug, Deserialize)]
pub struct ServiceProbe {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub require_fast_realtime: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AsrConfig {
    pub endpoint: String,
    #[serde(default)]
    pub stream: bool,
    #[serde(default = "default_asr_backend")]
    pub backend: String,
}

fn default_asr_backend() -> String {
    "rk".to_string()
}

#[derive(Clone, Debug, Deserialize)]
pub struct SummaryConfig {
    pub endpoint: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub live_endpoint: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct KnowledgeConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub endpoint: String,
    /// Explicit token path (Windows install). Empty means "under app data dir".
    #[serde(default)]
    pub token_path: String,
    /// Explicit audio-cache root (Windows install). Empty means "under app data dir".
    #[serde(default)]
    pub audio_cache: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Capabilities {
    /// Knowledge centre entry point in the UI.
    #[serde(default)]
    pub knowledge_center: bool,
    /// Reboot / power-off actions (RK device only).
    #[serde(default)]
    pub power_actions: bool,
    /// "设备状态 / 本地模型服务" strip on the home page.
    #[serde(default)]
    pub device_strip: bool,
    /// Block "开始会议" until local services report ready.
    #[serde(default)]
    pub service_readiness: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct RawProfile {
    kind: ProfileKind,
    #[serde(default)]
    label: String,
    asr: AsrConfig,
    summary: SummaryConfig,
    #[serde(default)]
    services: Vec<ServiceProbe>,
    #[serde(default)]
    knowledge: KnowledgeConfig,
    #[serde(default)]
    bootstrap_script: Option<String>,
    capabilities: Capabilities,
}

/// Fully resolved profile: every optional/derived value is materialised here.
#[derive(Clone, Debug)]
pub struct Profile {
    pub kind: ProfileKind,
    pub label: String,
    pub asr_endpoint: String,
    pub asr_stream: bool,
    pub asr_backend: String,
    pub summary_endpoint: String,
    pub summary_model: String,
    pub live_summary_endpoint: String,
    pub services: Vec<ServiceProbe>,
    pub knowledge_enabled: bool,
    pub knowledge_endpoint: String,
    pub knowledge_token_path: Option<PathBuf>,
    pub knowledge_audio_cache: Option<PathBuf>,
    pub bootstrap_script: Option<PathBuf>,
    pub caps: Capabilities,
}

fn env_trim(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn trim_endpoint(value: &str) -> String {
    value.trim_end_matches('/').to_string()
}

impl Profile {
    /// Resolve the profile for this process.
    ///
    /// Order: `MEETINGBOX_PROFILE` → `MEETINGBOX_CONFIG` file → compiled-in
    /// platform default → per-field environment overrides.
    pub fn resolve() -> Self {
        let kind = env_trim("MEETINGBOX_PROFILE")
            .and_then(|value| ProfileKind::parse(&value))
            .unwrap_or_else(Self::detect);

        let raw_json = env_trim("MEETINGBOX_CONFIG")
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_else(|| kind.embedded().to_string());

        let raw: RawProfile = serde_json::from_str(&raw_json).unwrap_or_else(|error| {
            log::error!(
                "Invalid platform profile JSON ({error}); falling back to built-in dev profile"
            );
            serde_json::from_str(DEV_JSON).expect("built-in dev profile must parse")
        });

        let mut profile = Self {
            kind: raw.kind,
            label: if raw.label.is_empty() {
                raw.kind.as_str().to_string()
            } else {
                raw.label
            },
            asr_endpoint: trim_endpoint(&raw.asr.endpoint),
            asr_stream: raw.asr.stream,
            asr_backend: raw.asr.backend,
            summary_endpoint: trim_endpoint(&raw.summary.endpoint),
            summary_model: raw.summary.model,
            live_summary_endpoint: trim_endpoint(&raw.summary.live_endpoint),
            services: raw.services,
            knowledge_enabled: raw.knowledge.enabled,
            knowledge_endpoint: trim_endpoint(&raw.knowledge.endpoint),
            knowledge_token_path: non_empty_path(&raw.knowledge.token_path),
            knowledge_audio_cache: non_empty_path(&raw.knowledge.audio_cache),
            bootstrap_script: raw
                .bootstrap_script
                .as_deref()
                .and_then(non_empty_path),
            caps: raw.capabilities,
        };

        profile.apply_env_overrides();
        profile
    }

    fn detect() -> ProfileKind {
        #[cfg(target_os = "windows")]
        {
            return ProfileKind::Intel;
        }
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        {
            return ProfileKind::Rk;
        }
        #[allow(unreachable_code)]
        ProfileKind::Dev
    }

    /// Environment keeps working as an escape hatch (existing installs and field
    /// debugging), but the profile file is the documented surface.
    fn apply_env_overrides(&mut self) {
        if let Some(value) = env_trim("MEETINGBOX_ASR_URL") {
            self.asr_endpoint = trim_endpoint(&value);
        }
        if let Some(value) = env_trim("MEETINGBOX_ASR_STREAM") {
            self.asr_stream = value == "1" || value.eq_ignore_ascii_case("true");
        }
        if let Some(value) = env_trim("MEETINGBOX_ASR_BACKEND") {
            self.asr_backend = value;
        }
        if let Some(value) = env_trim("MEETINGBOX_SUMMARY_URL") {
            self.summary_endpoint = trim_endpoint(&value);
        }
        if let Some(value) = env_trim("MEETINGBOX_SUMMARY_MODEL") {
            self.summary_model = value;
        }
        if let Some(value) = env_trim("MEETINGBOX_KNOWLEDGE_URL") {
            self.knowledge_endpoint = trim_endpoint(&value);
        }
        if let Some(value) = env_trim("MEETINGBOX_KNOWLEDGE_ROOT") {
            let root = PathBuf::from(value);
            self.knowledge_token_path = Some(root.join("service.token"));
            self.knowledge_audio_cache = Some(root.join("audio-cache"));
            self.knowledge_enabled = true;
        }
        if env_trim("MEETINGBOX_KNOWLEDGE_DISABLE").is_some() {
            self.knowledge_enabled = false;
        }
    }

    pub fn asr_health_url(&self) -> String {
        format!("{}/health", self.asr_endpoint)
    }

    /// Summary services expose `/health` at the root, while the OpenAI-compatible
    /// client talks to `<root>/v1`.
    pub fn summary_health_url(&self) -> String {
        let base = self
            .summary_endpoint
            .strip_suffix("/v1")
            .unwrap_or(&self.summary_endpoint);
        format!("{base}/health")
    }

    pub fn asr_stream_health_url(&self) -> String {
        format!("{}/healthz", self.asr_endpoint)
    }

    /// Payload handed to the frontend so it can adapt its navigation and controls
    /// instead of guessing from `navigator.userAgent`.
    pub fn to_frontend(&self) -> Value {
        json!({
            "kind": self.kind.as_str(),
            "label": self.label,
            "knowledgeCenter": self.caps.knowledge_center && self.knowledge_enabled,
            "powerActions": self.caps.power_actions,
            "deviceStrip": self.caps.device_strip,
            "serviceReadiness": self.caps.service_readiness,
            "asrStreaming": self.asr_stream,
            "asrBackend": self.asr_backend,
            "summaryModel": self.summary_model,
        })
    }
}

fn non_empty_path(value: &str) -> Option<PathBuf> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// Process-wide profile.
pub fn current() -> &'static Profile {
    CURRENT.get_or_init(|| {
        let profile = Profile::resolve();
        log::info!(
            "Platform profile: {} ({}), asr={} stream={} summary={}",
            profile.kind.as_str(),
            profile.label,
            profile.asr_endpoint,
            profile.asr_stream,
            profile.summary_endpoint
        );
        profile
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_profile_parses() {
        for kind in [ProfileKind::Intel, ProfileKind::Rk, ProfileKind::Dev] {
            let raw: RawProfile =
                serde_json::from_str(kind.embedded()).expect("embedded profile must parse");
            assert_eq!(raw.kind, kind);
            assert!(!raw.asr.endpoint.is_empty());
        }
    }

    #[test]
    fn resolves_kind_from_env_aliases() {
        assert_eq!(ProfileKind::parse("RK3588"), Some(ProfileKind::Rk));
        assert_eq!(ProfileKind::parse(" rockchip "), Some(ProfileKind::Rk));
        assert_eq!(ProfileKind::parse("Windows"), Some(ProfileKind::Intel));
        assert_eq!(ProfileKind::parse("nonsense"), None);
    }

    #[test]
    fn intel_profile_keeps_streaming_and_knowledge() {
        let raw: RawProfile = serde_json::from_str(INTEL_JSON).unwrap();
        assert!(raw.asr.stream);
        assert!(raw.knowledge.enabled);
        assert!(raw.capabilities.service_readiness);
        assert!(!raw.capabilities.power_actions);
        assert_eq!(raw.services.len(), 3);
    }

    #[test]
    fn rk_profile_exposes_power_actions_without_readiness_gate() {
        let raw: RawProfile = serde_json::from_str(RK_JSON).unwrap();
        assert!(raw.capabilities.power_actions);
        assert!(!raw.capabilities.service_readiness);
        assert!(!raw.asr.stream);
        assert_eq!(raw.asr.backend, "rk-npu");
    }

    #[test]
    fn empty_paths_are_treated_as_unset() {
        assert!(non_empty_path("   ").is_none());
        assert!(non_empty_path("D:\\x").is_some());
    }
}
