// Explicit RK NPU routing policy and observable runtime state.

use super::provider::TranscriptionError;
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    RwLock,
};
use std::time::Duration;

static STATUS: RwLock<Option<AsrRuntimeStatus>> = RwLock::new(None);
static INCOMPLETE_RK_SESSION: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Default, Serialize)]
pub struct AsrRuntimeStatus {
    pub configured_backend: String,
    pub active_backend: String,
    pub model: Option<String>,
    pub device: Option<String>,
    pub fallback_reason: Option<String>,
    pub npu_reported: bool,
}

#[derive(Debug, Deserialize)]
pub struct RkHealth {
    pub ready: bool,
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub device: Option<String>,
}

impl RkHealth {
    pub fn reports_sensevoice_npu(&self) -> bool {
        self.ready
            && self.backend.as_deref() == Some("sensevoice-rknn")
            && matches!(self.device.as_deref(), Some("npu" | "rknn" | "rk3588-npu"))
    }
}

pub struct NpuPolicy {
    pub fallback: Option<super::bossai_provider::BossAiProvider>,
    pub fallback_active: AtomicBool,
    pub request_timeout: Duration,
}

impl NpuPolicy {
    pub fn new(fallback: &str, cpu_url: &str, timeout: u64) -> Result<Self, TranscriptionError> {
        if !(1..=180).contains(&timeout) {
            return Err(error(
                "MEETINGBOX_RK_ASR_TIMEOUT_SECONDS must be between 1 and 180",
            ));
        }
        let fallback = match fallback {
            "none" => None,
            "bossai" => Some(super::bossai_provider::BossAiProvider::new(cpu_url)?),
            _ => return Err(error("MEETINGBOX_ASR_FALLBACK must be 'none' or 'bossai'")),
        };
        Ok(Self {
            fallback,
            fallback_active: AtomicBool::new(false),
            request_timeout: Duration::from_secs(timeout),
        })
    }

    pub fn is_fallback(&self) -> bool {
        self.fallback_active.load(Ordering::SeqCst)
    }

    pub fn activate_fallback(&self, reason: &str) {
        self.fallback_active.store(true, Ordering::SeqCst);
        INCOMPLETE_RK_SESSION.store(true, Ordering::SeqCst);
        publish(AsrRuntimeStatus {
            configured_backend: "rk-npu".into(),
            active_backend: "bossai-cpu".into(),
            model: Some("bossai-funasr-file".into()),
            device: Some("cpu".into()),
            fallback_reason: Some(reason.into()),
            npu_reported: false,
        });
    }
}

pub fn publish(status: AsrRuntimeStatus) {
    log::info!(
        "ASR backend={} model={:?} device={:?} npu_reported={} fallback={:?}",
        status.active_backend,
        status.model,
        status.device,
        status.npu_reported,
        status.fallback_reason
    );
    if let Ok(mut current) = STATUS.write() {
        *current = Some(status);
    }
}

pub fn runtime_status() -> Option<AsrRuntimeStatus> {
    STATUS.read().ok().and_then(|status| status.clone())
}

static OPERATION_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Recording, import and retranscription share this operation ownership.
pub fn operation_active() -> bool {
    OPERATION_ACTIVE.load(Ordering::SeqCst)
}

pub struct AsrOperationGuard;

impl AsrOperationGuard {
    pub fn acquire() -> Result<Self, TranscriptionError> {
        OPERATION_ACTIVE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| error("已有录音、导入或重新转写任务，请等待完成"))?;
        Ok(Self)
    }
}

impl Drop for AsrOperationGuard {
    fn drop(&mut self) {
        OPERATION_ACTIVE.store(false, Ordering::SeqCst);
    }
}

pub fn mark_incomplete_session() {
    INCOMPLETE_RK_SESSION.store(true, Ordering::SeqCst);
}

pub fn shutdown_timeout() -> Duration {
    if std::env::var("MEETINGBOX_ASR_BACKEND").ok().as_deref() == Some("rk-npu") {
        let rk_timeout = std::env::var("MEETINGBOX_RK_ASR_TIMEOUT_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(90)
            .clamp(1, 180);
        npu_drain_timeout(rk_timeout)
    } else {
        Duration::from_secs(45)
    }
}

fn npu_drain_timeout(rk_timeout: u64) -> Duration {
    // One in-flight RK request, CPU health + one CPU attempt, plus health overhead.
    Duration::from_secs(rk_timeout + 195)
}

pub fn reset_session() {
    INCOMPLETE_RK_SESSION.store(false, Ordering::SeqCst);
    if let Ok(mut current) = STATUS.write() {
        *current = None;
    }
}

pub fn has_incomplete_rk_session() -> bool {
    INCOMPLETE_RK_SESSION.load(Ordering::SeqCst)
}

pub fn validate_endpoint(endpoint: &str) -> Result<String, TranscriptionError> {
    let url = reqwest::Url::parse(endpoint).map_err(|_| error("Invalid MEETINGBOX_RK_ASR_URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(error(
            "RK ASR URL must be HTTP(S), without credentials, query or fragment",
        ));
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

pub fn error(message: impl Into<String>) -> TranscriptionError {
    TranscriptionError::EngineFailed(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn npu_requires_explicit_backend_and_readiness() {
        for (json, expected) in [
            (r#"{"ready":true}"#, false),
            (
                r#"{"ready":true,"backend":"paraformer","device":"cpu"}"#,
                false,
            ),
            (
                r#"{"ready":true,"backend":"sensevoice-rknn","device":"cpu"}"#,
                false,
            ),
            (r#"{"ready":false,"backend":"sensevoice-rknn"}"#, false),
            (
                r#"{"ready":true,"backend":"sensevoice-rknn","device":"npu"}"#,
                true,
            ),
            (r#"{"ready":true,"backend":"sensevoice-rknn"}"#, false),
        ] {
            assert_eq!(
                serde_json::from_str::<RkHealth>(json)
                    .unwrap()
                    .reports_sensevoice_npu(),
                expected
            );
        }
    }
    #[test]
    fn fallback_and_timeout_are_explicit() {
        assert!(NpuPolicy::new("none", "unused", 90)
            .unwrap()
            .fallback
            .is_none());
        assert!(NpuPolicy::new("invalid", "unused", 90).is_err());
        assert!(NpuPolicy::new("none", "unused", 0).is_err());
        assert!(NpuPolicy::new("none", "unused", 181).is_err());
    }
    #[test]
    fn drain_budget_covers_npu_timeout_and_cpu_attempt() {
        assert_eq!(npu_drain_timeout(90), Duration::from_secs(285));
        assert!(npu_drain_timeout(180) > Duration::from_secs(180 + 180 + 5));
    }

    #[test]
    fn operations_are_exclusive_and_release_on_drop() {
        let first = AsrOperationGuard::acquire().unwrap();
        assert!(AsrOperationGuard::acquire().is_err());
        drop(first);
        assert!(AsrOperationGuard::acquire().is_ok());
    }

    #[test]
    fn validates_endpoint_without_leaking_credentials() {
        assert_eq!(
            validate_endpoint("http://localhost:8020/").unwrap(),
            "http://localhost:8020"
        );
        for url in [
            "file:///tmp/asr",
            "http://user:secret@host",
            "http://host?token=secret",
        ] {
            assert!(validate_endpoint(url).is_err());
        }
    }
}
