// RK3588 ASR service provider for the Meeting Box distribution.

use super::provider::{TranscriptResult, TranscriptionError, TranscriptionProvider};
use async_trait::async_trait;
use log::{info, warn};
use once_cell::sync::Lazy;
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const RK_ASR_ENDPOINT: &str = "http://127.0.0.1:8020";
pub const RK_ASR_MODEL: &str = "sensevoice-rknn";
const SAMPLE_RATE: u32 = 16_000;
const MINIMUM_SAMPLES: usize = 16_000; // 1s at 16kHz; shorter clips crash SenseVoice fsmnvad
const FINALIZE_POLL_INTERVAL: Duration = Duration::from_millis(500);
const FINALIZE_TIMEOUT: Duration = Duration::from_secs(300);

static CURRENT_MEETING_ID: AtomicI64 = AtomicI64::new(0);
static CURRENT_CHUNK_START_MS: AtomicI64 = AtomicI64::new(0);
static MEETING_SEQ: AtomicI64 = AtomicI64::new(1);
static SKIP_VAD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static HOTWORDS: Lazy<RwLock<Vec<(String, Option<String>)>>> =
    Lazy::new(|| RwLock::new(Vec::new()));

pub fn set_hotwords(hotwords: Vec<(String, Option<String>)>) {
    if let Ok(mut configured) = HOTWORDS.write() {
        *configured = hotwords;
    }
}

fn configured_hotwords_json() -> String {
    HOTWORDS
        .read()
        .ok()
        .and_then(|configured| {
            serde_json::to_string(
                &configured
                    .iter()
                    .map(|(phrase, _)| phrase)
                    .collect::<Vec<_>>(),
            )
            .ok()
        })
        .unwrap_or_else(|| "[]".to_string())
}

fn apply_hotword_corrections(mut text: String) -> String {
    if let Ok(configured) = HOTWORDS.read() {
        for (phrase, replacement) in configured.iter() {
            if let Some(replacement) = replacement.as_deref().filter(|value| !value.is_empty()) {
                text = text.replace(replacement, phrase);
            }
        }
    }
    text
}

#[derive(Debug, Deserialize)]
struct HealthResponse {
    ready: bool,
}

#[derive(Debug, Deserialize)]
struct AsrSegment {
    text: String,
    #[serde(default)]
    speaker: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    speaker_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct AsrResponse {
    #[serde(default)]
    error: Option<serde_json::Value>,
    #[serde(default)]
    backend: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    segments: Option<Vec<AsrSegment>>,
    #[serde(default)]
    diarization: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct FinalizedAsrSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    #[serde(default)]
    pub speaker_id: Option<i64>,
    #[serde(default)]
    pub speaker: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SpeakerCalibrationResult {
    pub finalized: bool,
    pub segments: Vec<FinalizedAsrSegment>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FinalizeResponse {
    #[serde(default)]
    finalized: bool,
    #[serde(default)]
    pending: bool,
    #[serde(default)]
    job_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    segments: Vec<FinalizedAsrSegment>,
    #[serde(default)]
    error: Option<String>,
}

pub struct RkAsrProvider {
    endpoint: String,
    client: reqwest::Client,
    bossai: Option<super::bossai_provider::BossAiProvider>,
    npu_policy: Option<super::rk_policy::NpuPolicy>,
}

impl RkAsrProvider {
    pub fn new(endpoint: impl Into<String>) -> Result<Self, TranscriptionError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(180))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| TranscriptionError::EngineFailed(error.to_string()))?;

        Ok(Self {
            endpoint: super::rk_policy::validate_endpoint(&endpoint.into())?,
            client,
            bossai: None,
            npu_policy: None,
        })
    }

    pub fn local() -> Result<Self, TranscriptionError> {
        // Defaults come from the platform profile; environment variables still win so
        // field debugging and existing installs keep working.
        let profile = crate::platform::current();
        let backend = std::env::var("MEETINGBOX_ASR_BACKEND")
            .unwrap_or_else(|_| profile.asr_backend.clone());
        let cpu_url = std::env::var("MEETINGBOX_BOSSAI_ASR_URL").ok();
        let mut provider = Self::configured(&backend, cpu_url.as_deref())?;
        if provider.bossai.is_none() {
            provider.endpoint = super::rk_policy::validate_endpoint(
                &std::env::var("MEETINGBOX_RK_ASR_URL")
                    .unwrap_or_else(|_| profile.asr_endpoint.clone()),
            )?;
        }
        if backend == "rk-npu" {
            let timeout = std::env::var("MEETINGBOX_RK_ASR_TIMEOUT_SECONDS")
                .unwrap_or_else(|_| "90".into())
                .parse::<u64>()
                .map_err(|_| {
                    super::rk_policy::error("Invalid MEETINGBOX_RK_ASR_TIMEOUT_SECONDS")
                })?;
            provider.npu_policy = Some(super::rk_policy::NpuPolicy::new(
                &std::env::var("MEETINGBOX_ASR_FALLBACK").unwrap_or_else(|_| "none".into()),
                cpu_url
                    .as_deref()
                    .unwrap_or(super::bossai_provider::BOSSAI_ASR_ENDPOINT),
                timeout,
            )?);
        }
        Ok(provider)
    }

    fn configured(backend: &str, endpoint: Option<&str>) -> Result<Self, TranscriptionError> {
        let mut provider = Self::new(crate::platform::current().asr_endpoint.clone())?;
        match backend {
            "rk" => {}
            "rk-npu" => {
                provider.npu_policy = Some(super::rk_policy::NpuPolicy::new(
                    "none",
                    super::bossai_provider::BOSSAI_ASR_ENDPOINT,
                    90,
                )?);
            }
            "bossai" => {
                provider.bossai = Some(super::bossai_provider::BossAiProvider::new(
                    endpoint.unwrap_or(super::bossai_provider::BOSSAI_ASR_ENDPOINT),
                )?);
            }
            _ => {
                return Err(TranscriptionError::EngineFailed(
                    "MEETINGBOX_ASR_BACKEND must be 'rk', 'rk-npu' or 'bossai'".to_string(),
                ))
            }
        }
        Ok(provider)
    }

    pub async fn finalize_current_meeting(
        &self,
    ) -> Result<SpeakerCalibrationResult, TranscriptionError> {
        let meeting_id = CURRENT_MEETING_ID.swap(0, Ordering::SeqCst);
        CURRENT_CHUNK_START_MS.store(0, Ordering::SeqCst);
        if super::rk_policy::has_incomplete_rk_session() {
            return Ok(SpeakerCalibrationResult {
                error: Some("RK session is incomplete after CPU fallback or cancellation; speaker calibration was skipped".into()),
                ..SpeakerCalibrationResult::default()
            });
        }
        if meeting_id <= 0 || self.bossai.is_some() {
            // BossAI's file endpoint has no meeting-wide speaker finalization.
            // Preserve live transcript timestamps and do not call the RK API.
            return Ok(SpeakerCalibrationResult::default());
        }

        let response = self
            .client
            .post(format!("{}/v1/asr/finalize-meeting", self.endpoint))
            .json(&json!({
                "meeting_id": meeting_id,
                "segments": []
            }))
            .send()
            .await
            .map_err(|error| TranscriptionError::EngineFailed(error.to_string()))?;

        let mut payload = parse_finalize_response(response, "finalize").await?;
        if payload.finalized || !payload.pending {
            return Ok(calibration_result(payload));
        }
        let job_id = payload.job_id.take().ok_or_else(|| {
            TranscriptionError::EngineFailed("RK ASR finalize did not return job_id".to_string())
        })?;

        info!(
            "RK ASR finalize requested for meeting_id={} job_id={}",
            meeting_id, job_id
        );
        let started = std::time::Instant::now();
        while started.elapsed() < FINALIZE_TIMEOUT {
            tokio::time::sleep(FINALIZE_POLL_INTERVAL).await;
            let response = self
                .client
                .get(format!(
                    "{}/v1/asr/finalize-meeting/{}",
                    self.endpoint, job_id
                ))
                .send()
                .await
                .map_err(|error| TranscriptionError::EngineFailed(error.to_string()))?;
            payload = parse_finalize_response(response, "finalize status").await?;
            if payload.status.as_deref() == Some("failed") {
                return Err(TranscriptionError::EngineFailed(
                    payload
                        .error
                        .unwrap_or_else(|| "RK ASR speaker calibration failed".to_string()),
                ));
            }
            if payload.finalized || payload.status.as_deref() == Some("completed") {
                info!(
                    "RK ASR speaker calibration completed for meeting_id={} segments={}",
                    meeting_id,
                    payload.segments.len()
                );
                return Ok(calibration_result(payload));
            }
        }

        Err(TranscriptionError::EngineFailed(format!(
            "RK ASR speaker calibration timed out after {} seconds",
            FINALIZE_TIMEOUT.as_secs()
        )))
    }

    async fn probe_npu(&self) -> Result<(), TranscriptionError> {
        let response = self
            .client
            .get(format!("{}/healthz", self.endpoint))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|_| super::rk_policy::error("RK NPU health request failed"))?;
        if !response.status().is_success() {
            return Err(super::rk_policy::error(format!(
                "RK NPU health returned HTTP {}",
                response.status()
            )));
        }
        let health = response
            .json::<super::rk_policy::RkHealth>()
            .await
            .map_err(|_| super::rk_policy::error("RK NPU health response is invalid"))?;
        if !health.reports_sensevoice_npu() {
            return Err(super::rk_policy::error(
                "RK service is not ready or does not report backend=sensevoice-rknn",
            ));
        }
        super::rk_policy::publish(super::rk_policy::AsrRuntimeStatus {
            configured_backend: "rk-npu".into(),
            active_backend: health.backend.unwrap(),
            model: health.model,
            device: health.device,
            npu_reported: true,
            fallback_reason: None,
        });
        Ok(())
    }

    async fn activate_cpu_fallback(&self, reason: &str) -> Result<(), TranscriptionError> {
        super::rk_policy::mark_incomplete_session();
        super::rk_policy::publish(super::rk_policy::AsrRuntimeStatus {
            configured_backend: "rk-npu".into(),
            active_backend: "unavailable".into(),
            fallback_reason: Some(reason.into()),
            ..super::rk_policy::AsrRuntimeStatus::default()
        });
        let policy = self.npu_policy.as_ref().unwrap();
        let cpu = policy
            .fallback
            .as_ref()
            .ok_or_else(|| super::rk_policy::error(reason))?;
        if !cpu.check_cpu_ready().await? {
            return Err(super::rk_policy::error(
                "RK NPU unavailable and BossAI file model is not ready",
            ));
        }
        policy.activate_fallback(reason);
        Ok(())
    }

    async fn transcribe_npu(
        &self,
        audio: Vec<f32>,
        language: Option<String>,
    ) -> Result<TranscriptResult, TranscriptionError> {
        if audio.len() < 1600 {
            return Err(TranscriptionError::AudioTooShort {
                samples: audio.len(),
                minimum: 1600,
            });
        }
        if audio.iter().any(|sample| !sample.is_finite()) {
            return Err(super::rk_policy::error(
                "RK NPU audio contains non-finite samples",
            ));
        }
        let policy = self.npu_policy.as_ref().unwrap();
        if !policy.is_fallback() {
            let result = match self.probe_npu().await {
                Ok(()) => {
                    let mut rk_audio = audio.clone();
                    rk_audio.resize(rk_audio.len().max(MINIMUM_SAMPLES), 0.0);
                    self.transcribe_rk(rk_audio).await
                }
                Err(error) => Err(error),
            };
            match result {
                Ok(result) => return Ok(result),
                Err(error) => self.activate_cpu_fallback(&error.to_string()).await?,
            }
        }
        let mut result = policy
            .fallback
            .as_ref()
            .unwrap()
            .transcribe(audio, language)
            .await?;
        result.text = apply_hotword_corrections(result.text);
        Ok(result)
    }

    async fn transcribe_rk(&self, audio: Vec<f32>) -> Result<TranscriptResult, TranscriptionError> {
        if audio.len() < MINIMUM_SAMPLES {
            return Err(TranscriptionError::AudioTooShort {
                samples: audio.len(),
                minimum: MINIMUM_SAMPLES,
            });
        }

        let meeting_id = current_meeting_id();
        let chunk_start_ms = CURRENT_CHUNK_START_MS.load(Ordering::SeqCst).max(0);
        let audio = prepare_asr_audio(&audio);
        let wav = encode_pcm16_wav(&audio);
        let part = Part::bytes(wav)
            .file_name("meetily-chunk.wav")
            .mime_str("audio/wav")
            .map_err(|error| TranscriptionError::EngineFailed(error.to_string()))?;
        let form = Form::new()
            .part("audio", part)
            .text("meeting_id", meeting_id.to_string())
            .text("chunk_start_ms", chunk_start_ms.to_string())
            .text("hotwords", configured_hotwords_json())
            .text(
                "skip_vad",
                if SKIP_VAD.load(Ordering::SeqCst) {
                    "1"
                } else {
                    "0"
                }
                .to_string(),
            );

        let started = std::time::Instant::now();
        let request = self
            .client
            .post(format!("{}/v1/asr/transcribe-upload", self.endpoint))
            .multipart(form);
        let request = match &self.npu_policy {
            Some(policy) => request.timeout(policy.request_timeout),
            None => request,
        };
        let response = request
            .send()
            .await
            .map_err(|error| TranscriptionError::EngineFailed(error.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            if self.npu_policy.is_some() {
                return Err(super::rk_policy::error(format!("RK NPU recognition returned HTTP {}", status)));
            }
            let detail = response.text().await.unwrap_or_default();
            if self.npu_policy.is_none() && is_empty_vad_failure(&detail) {
                warn!(
                    "RK ASR inner VAD found no speech for meeting_id={} (HTTP {}), skipping chunk",
                    meeting_id, status
                );
                return Ok(TranscriptResult {
                    text: String::new(),
                    confidence: None,
                    is_partial: false,
                });
            }
            return Err(TranscriptionError::EngineFailed(format!(
                "RK ASR returned HTTP {}: {}",
                status,
                sanitize_asr_detail(&detail)
            )));
        }

        let payload = response
            .json::<AsrResponse>()
            .await
            .map_err(|error| TranscriptionError::EngineFailed(error.to_string()))?;
        if self.npu_policy.is_some() {
            if payload.segments.is_none()
                || payload.error.as_ref().is_some_and(|error| !error.is_null())
            {
                return Err(super::rk_policy::error(
                    "RK recognition response is missing segments or reports failure",
                ));
            }
            let reported = super::rk_policy::RkHealth {
                ready: true,
                backend: payload.backend.clone(),
                model: payload.model.clone(),
                device: payload.device.clone(),
            };
            if !reported.reports_sensevoice_npu() {
                return Err(super::rk_policy::error(
                    "RK recognition did not report backend=sensevoice-rknn",
                ));
            }
            super::rk_policy::publish(super::rk_policy::AsrRuntimeStatus {
                configured_backend: "rk-npu".into(),
                active_backend: payload.backend.clone().unwrap(),
                model: payload.model.clone(),
                device: payload.device.clone(),
                npu_reported: true,
                fallback_reason: None,
            });
        }
        if let Some(diarization) = payload.diarization.as_deref() {
            info!(
                "RK ASR meeting_id={} chunk_start_ms={} diarization={}",
                meeting_id, chunk_start_ms, diarization
            );
        }
        let text = apply_hotword_corrections(format_transcript_text(
            payload.segments.as_deref().unwrap_or_default(),
        ));
        info!(
            "RK ASR meeting_id={} chunk_start_ms={} elapsed_ms={} chars={}",
            meeting_id,
            chunk_start_ms,
            started.elapsed().as_millis(),
            text.chars().count()
        );

        Ok(TranscriptResult {
            text,
            confidence: None,
            is_partial: false,
        })
    }

    pub async fn check_ready(&self) -> Result<bool, TranscriptionError> {
        if let Some(policy) = &self.npu_policy {
            if policy.is_fallback() {
                return policy.fallback.as_ref().unwrap().check_cpu_ready().await;
            }
            return match self.probe_npu().await {
                Ok(()) => Ok(true),
                Err(error) => self
                    .activate_cpu_fallback(&error.to_string())
                    .await
                    .map(|()| true),
            };
        }
        if let Some(provider) = &self.bossai {
            return provider.check_ready().await;
        }
        let response = self
            .client
            .get(format!("{}/healthz", self.endpoint))
            .send()
            .await
            .map_err(|error| TranscriptionError::EngineFailed(error.to_string()))?;

        if !response.status().is_success() {
            return Err(TranscriptionError::EngineFailed(format!(
                "RK ASR health check returned HTTP {}",
                response.status()
            )));
        }

        response
            .json::<HealthResponse>()
            .await
            .map(|health| health.ready)
            .map_err(|error| TranscriptionError::EngineFailed(error.to_string()))
    }
}

#[async_trait]
impl TranscriptionProvider for RkAsrProvider {
    async fn transcribe(
        &self,
        audio: Vec<f32>,
        language: Option<String>,
    ) -> Result<TranscriptResult, TranscriptionError> {
        if let Some(provider) = &self.bossai {
            let mut result = provider.transcribe(audio, language).await?;
            result.text = apply_hotword_corrections(result.text);
            return Ok(result);
        }
        if self.npu_policy.is_some() {
            return self.transcribe_npu(audio, language).await;
        }
        self.transcribe_rk(audio).await
    }

    async fn is_model_loaded(&self) -> bool {
        self.check_ready().await.unwrap_or(false)
    }

    async fn get_current_model(&self) -> Option<String> {
        if let Some(provider) = &self.bossai {
            return provider.get_current_model().await;
        }
        if let Some(policy) = &self.npu_policy {
            if !self.check_ready().await.unwrap_or(false) {
                return None;
            }
            if policy.is_fallback() {
                return Some("bossai-funasr-file".into());
            }
            return super::rk_policy::runtime_status().and_then(|status| status.model);
        }
        self.is_model_loaded()
            .await
            .then(|| RK_ASR_MODEL.to_string())
    }

    fn provider_name(&self) -> &'static str {
        if let Some(policy) = &self.npu_policy {
            return if policy.is_fallback() {
                "BossAI CPU fallback"
            } else {
                "RK NPU service"
            };
        }
        match &self.bossai {
            Some(provider) => provider.provider_name(),
            None => "RK ASR",
        }
    }
}

pub fn begin_meeting_session() -> i64 {
    super::rk_policy::reset_session();
    set_skip_vad(false);
    let seq = MEETING_SEQ.fetch_add(1, Ordering::Relaxed);
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(1)
        .saturating_mul(1000)
        .saturating_add(seq.abs());
    let meeting_id = id.max(1);
    CURRENT_MEETING_ID.store(meeting_id, Ordering::SeqCst);
    CURRENT_CHUNK_START_MS.store(0, Ordering::SeqCst);
    info!("RK ASR meeting session started: {}", meeting_id);
    meeting_id
}

pub fn set_chunk_start_ms(chunk_start_ms: i64) {
    CURRENT_CHUNK_START_MS.store(chunk_start_ms.max(0), Ordering::SeqCst);
}

pub fn set_skip_vad(skip_vad: bool) {
    SKIP_VAD.store(skip_vad, Ordering::SeqCst);
}

pub struct SkipVadGuard;

impl SkipVadGuard {
    pub fn enable() -> Self {
        set_skip_vad(true);
        Self
    }
}

impl Drop for SkipVadGuard {
    fn drop(&mut self) {
        set_skip_vad(false);
    }
}

pub fn current_meeting_id() -> i64 {
    CURRENT_MEETING_ID.load(Ordering::SeqCst)
}

pub fn is_rk_asr_provider(provider: Option<&str>) -> bool {
    !matches!(provider, Some("parakeet" | "whisper" | "localWhisper"))
}

pub async fn finalize_current_meeting() -> SpeakerCalibrationResult {
    match RkAsrProvider::local() {
        Ok(provider) => match provider.finalize_current_meeting().await {
            Ok(result) => result,
            Err(error) => {
                warn!("RK ASR finalize failed: {}", error);
                SpeakerCalibrationResult {
                    error: Some(error.to_string()),
                    ..SpeakerCalibrationResult::default()
                }
            }
        },
        Err(error) => {
            warn!("RK ASR finalize skipped: {}", error);
            SpeakerCalibrationResult {
                error: Some(error.to_string()),
                ..SpeakerCalibrationResult::default()
            }
        }
    }
}

async fn parse_finalize_response(
    response: reqwest::Response,
    operation: &str,
) -> Result<FinalizeResponse, TranscriptionError> {
    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        return Err(TranscriptionError::EngineFailed(format!(
            "RK ASR {} returned HTTP {}: {}",
            operation,
            status,
            detail.chars().take(500).collect::<String>()
        )));
    }
    response
        .json::<FinalizeResponse>()
        .await
        .map_err(|error| TranscriptionError::EngineFailed(error.to_string()))
}

fn calibration_result(payload: FinalizeResponse) -> SpeakerCalibrationResult {
    SpeakerCalibrationResult {
        finalized: payload.finalized || payload.status.as_deref() == Some("completed"),
        segments: payload.segments,
        error: payload.error,
    }
}

fn format_transcript_text(segments: &[AsrSegment]) -> String {
    segments
        .iter()
        .filter_map(format_segment_text)
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_segment_text(segment: &AsrSegment) -> Option<String> {
    let text = segment.text.trim();
    if text.is_empty() {
        return None;
    }
    let speaker = segment
        .speaker
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty() && *label != "发言人待识别");
    Some(match speaker {
        Some(label) => format!("{label}：{text}"),
        None => text.to_string(),
    })
}

fn prepare_asr_audio(audio: &[f32]) -> Vec<f32> {
    let peak = audio
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0f32, f32::max);
    let gain = if peak > 1e-4 {
        (0.85 / peak).min(12.0)
    } else {
        1.0
    };
    let pad = (SAMPLE_RATE as usize) * 100 / 1000;
    let mut prepared = vec![0.0; pad];
    prepared.extend(audio.iter().map(|sample| (sample * gain).clamp(-1.0, 1.0)));
    prepared.extend(std::iter::repeat(0.0).take(pad));
    info!(
        "RK ASR audio prepared: in={} peak={:.4} gain={:.2} out={}",
        audio.len(),
        peak,
        gain,
        prepared.len()
    );
    prepared
}

fn is_empty_vad_failure(detail: &str) -> bool {
    detail.contains("IndexError")
        || detail.contains("fsmnvad")
        || detail.contains("list index out of range")
}

fn sanitize_asr_detail(detail: &str) -> String {
    if is_empty_vad_failure(detail) {
        return "no speech in this chunk".to_string();
    }
    detail
        .lines()
        .next()
        .unwrap_or(detail)
        .chars()
        .take(160)
        .collect()
}

pub(super) fn encode_pcm16_wav(audio: &[f32]) -> Vec<u8> {
    let data_len = (audio.len() * std::mem::size_of::<i16>()) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());

    for sample in audio {
        let pcm = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        wav.extend_from_slice(&pcm.to_le_bytes());
    }
    wav
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn mock_service(
        replies: Vec<(&str, &str)>,
    ) -> (String, tokio::task::JoinHandle<Vec<Vec<u8>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let replies: Vec<_> = replies.into_iter().map(|(status, body)| format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())).collect();
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for reply in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0; 8192];
                    let n = socket.read(&mut buffer).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..n]);
                    if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                        let len: usize = headers
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length: "))
                            .unwrap_or("0")
                            .parse()
                            .unwrap();
                        if request.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                socket.write_all(reply.as_bytes()).await.unwrap();
                requests.push(request);
            }
            requests
        });
        (endpoint, task)
    }

    fn npu_provider(rk: &str, cpu: Option<&str>) -> RkAsrProvider {
        let mut provider = RkAsrProvider::new(rk).unwrap();
        provider.npu_policy = Some(
            super::super::rk_policy::NpuPolicy::new(
                if cpu.is_some() { "bossai" } else { "none" },
                cpu.unwrap_or("unused"),
                90,
            )
            .unwrap(),
        );
        provider
    }

    #[tokio::test]
    async fn npu_success_uploads_short_speech_and_uses_reported_model() {
        let _guard = SESSION_TEST_LOCK.lock().unwrap();
        super::super::rk_policy::reset_session();
        let (rk, requests) = mock_service(vec![
            ("200 OK", r#"{"ready":true,"backend":"sensevoice-rknn","model":"sensevoice-v1","device":"npu"}"#),
            ("200 OK", r#"{"backend":"sensevoice-rknn","model":"sensevoice-v1","device":"npu","segments":[{"text":"你好"}]}"#),
        ]).await;
        let result = npu_provider(&rk, None)
            .transcribe(vec![0.0; 3200], None)
            .await
            .unwrap();
        assert_eq!(result.text, "你好");
        let status = super::super::rk_policy::runtime_status().unwrap();
        assert!(status.npu_reported);
        assert_eq!(status.model.as_deref(), Some("sensevoice-v1"));
        assert!(status.fallback_reason.is_none());
        let requests = requests.await.unwrap();
        assert!(String::from_utf8_lossy(&requests[0]).starts_with("GET /healthz "));
        assert!(
            String::from_utf8_lossy(&requests[1]).starts_with("POST /v1/asr/transcribe-upload ")
        );
    }

    #[tokio::test]
    async fn legacy_ready_cannot_claim_npu_and_falls_back_explicitly() {
        let _guard = SESSION_TEST_LOCK.lock().unwrap();
        super::super::rk_policy::reset_session();
        let (rk, rk_requests) = mock_service(vec![("200 OK", r#"{"ready":true}"#)]).await;
        let (cpu, cpu_requests) = mock_service(vec![
            (
                "200 OK",
                r#"{"ok":true,"backend":"funasr","fileLoaded":true,"device":"cpu"}"#,
            ),
            (
                "200 OK",
                r#"{"text":"cpu","segments":[{"text":"CPU结果"}]}"#,
            ),
            ("200 OK", r#"{"text":"cpu","segments":[{"text":"下一句"}]}"#),
        ])
        .await;
        let provider = npu_provider(&rk, Some(&cpu));
        assert_eq!(
            provider
                .transcribe(vec![0.0; 3200], None)
                .await
                .unwrap()
                .text,
            "CPU结果"
        );
        // Sticky within this provider: no repeated NPU upload after failure.
        assert_eq!(
            provider
                .transcribe(vec![0.0; 3200], None)
                .await
                .unwrap()
                .text,
            "下一句"
        );
        let status = super::super::rk_policy::runtime_status().unwrap();
        assert_eq!(status.active_backend, "bossai-cpu");
        assert!(!status.npu_reported);
        assert!(status.fallback_reason.is_some());
        let calibration = provider.finalize_current_meeting().await.unwrap();
        assert!(!calibration.finalized);
        assert!(calibration.error.unwrap().contains("CPU fallback"));
        rk_requests.await.unwrap();
        cpu_requests.await.unwrap();
        super::super::rk_policy::reset_session();
    }

    #[tokio::test]
    async fn inference_failure_replays_original_audio_to_cpu_once() {
        let _guard = SESSION_TEST_LOCK.lock().unwrap();
        super::super::rk_policy::reset_session();
        let (rk, rk_requests) = mock_service(vec![
            (
                "200 OK",
                r#"{"ready":true,"backend":"sensevoice-rknn","device":"npu"}"#,
            ),
            ("503 Service Unavailable", "failed"),
        ])
        .await;
        let (cpu, cpu_requests) = mock_service(vec![
            (
                "200 OK",
                r#"{"ok":true,"backend":"funasr","fileLoaded":true,"device":"cpu"}"#,
            ),
            (
                "200 OK",
                r#"{"text":"ok","segments":[{"text":"回退成功"}]}"#,
            ),
        ])
        .await;
        let provider = npu_provider(&rk, Some(&cpu));
        assert_eq!(
            provider
                .transcribe(vec![0.25; 3200], None)
                .await
                .unwrap()
                .text,
            "回退成功"
        );
        let requests = cpu_requests.await.unwrap();
        let wav = encode_pcm16_wav(&vec![0.25; 3200]);
        assert!(requests[1].windows(wav.len()).any(|part| part == wav));
        rk_requests.await.unwrap();
        super::super::rk_policy::reset_session();
    }

    #[tokio::test]
    async fn npu_without_fallback_fails_closed_on_missing_metadata() {
        let _guard = SESSION_TEST_LOCK.lock().unwrap();
        super::super::rk_policy::reset_session();
        let (rk, requests) = mock_service(vec![("200 OK", r#"{"ready":true}"#)]).await;
        assert!(npu_provider(&rk, None)
            .check_ready()
            .await
            .unwrap_err()
            .to_string()
            .contains("sensevoice-rknn"));
        requests.await.unwrap();
    }

    #[tokio::test]
    async fn successful_http_with_cpu_backend_is_not_npu_success() {
        let _guard = SESSION_TEST_LOCK.lock().unwrap();
        super::super::rk_policy::reset_session();
        let (rk, requests) = mock_service(vec![
            (
                "200 OK",
                r#"{"ready":true,"backend":"sensevoice-rknn","device":"npu"}"#,
            ),
            (
                "200 OK",
                r#"{"backend":"paraformer","device":"cpu","segments":[{"text":"cpu"}]}"#,
            ),
        ])
        .await;
        assert!(npu_provider(&rk, None)
            .transcribe(vec![0.0; 16000], None)
            .await
            .unwrap_err()
            .to_string()
            .contains("did not report"));
        requests.await.unwrap();
    }

    #[tokio::test]
    async fn missing_segments_or_error_envelope_cannot_drop_speech_silently() {
        let _guard = SESSION_TEST_LOCK.lock().unwrap();
        for body in [
            r#"{"backend":"sensevoice-rknn","device":"npu","error":"failed"}"#,
            r#"{"backend":"sensevoice-rknn","device":"npu"}"#,
            r#"{"backend":"sensevoice-rknn","device":"npu","segments":[],"error":"failed"}"#,
        ] {
            super::super::rk_policy::reset_session();
            let (rk, requests) = mock_service(vec![
                (
                    "200 OK",
                    r#"{"ready":true,"backend":"sensevoice-rknn","device":"npu"}"#,
                ),
                ("200 OK", body),
            ])
            .await;
            assert!(npu_provider(&rk, None)
                .transcribe(vec![0.0; 16000], None)
                .await
                .is_err());
            requests.await.unwrap();
        }
    }

    #[tokio::test]
    async fn unavailable_cpu_does_not_claim_success_or_enable_calibration() {
        let _guard = SESSION_TEST_LOCK.lock().unwrap();
        super::super::rk_policy::reset_session();
        let (rk, requests) = mock_service(vec![("503 Service Unavailable", "failed")]).await;
        let (cpu, cpu_requests) = mock_service(vec![("200 OK", r#"{"ok":true,"backend":"funasr","fileLoaded":false,"device":"cpu"}"#)]).await;
        let provider = npu_provider(&rk, Some(&cpu));
        assert!(provider.check_ready().await.is_err());
        assert_eq!(super::super::rk_policy::runtime_status().unwrap().active_backend, "unavailable");
        assert!(super::super::rk_policy::has_incomplete_rk_session());
        requests.await.unwrap(); cpu_requests.await.unwrap();
        super::super::rk_policy::reset_session();
    }

    #[tokio::test]
    async fn valid_empty_segments_remain_silence_without_fallback() {
        let _guard = SESSION_TEST_LOCK.lock().unwrap();
        super::super::rk_policy::reset_session();
        let (rk, requests) = mock_service(vec![
            (
                "200 OK",
                r#"{"ready":true,"backend":"sensevoice-rknn","device":"npu"}"#,
            ),
            (
                "200 OK",
                r#"{"backend":"sensevoice-rknn","device":"npu","segments":[]}"#,
            ),
        ])
        .await;
        assert!(npu_provider(&rk, None)
            .transcribe(vec![0.0; 16000], None)
            .await
            .unwrap()
            .text
            .is_empty());
        assert!(!super::super::rk_policy::has_incomplete_rk_session());
        requests.await.unwrap();
    }

    #[test]
    fn explicit_backend_selection_preserves_rk_default() {
        assert_eq!(
            RkAsrProvider::configured("rk", None)
                .unwrap()
                .provider_name(),
            "RK ASR"
        );
        assert_eq!(
            RkAsrProvider::configured("bossai", None)
                .unwrap()
                .provider_name(),
            "BossAI FunASR"
        );
        assert!(RkAsrProvider::configured("unknown", None).is_err());
        assert!(RkAsrProvider::configured("bossai", Some("bad url")).is_err());
    }

    #[tokio::test]
    async fn bossai_finalization_clears_session_without_rk_request() {
        let _guard = SESSION_TEST_LOCK.lock().unwrap();
        CURRENT_MEETING_ID.store(42, Ordering::SeqCst);
        CURRENT_CHUNK_START_MS.store(1000, Ordering::SeqCst);
        let provider = RkAsrProvider::configured("bossai", None).unwrap();
        let result = provider.finalize_current_meeting().await.unwrap();
        assert!(!result.finalized);
        assert!(result.segments.is_empty());
        assert_eq!(CURRENT_MEETING_ID.load(Ordering::SeqCst), 0);
        assert_eq!(CURRENT_CHUNK_START_MS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn wav_encoder_emits_16khz_mono_pcm16() {
        let wav = encode_pcm16_wav(&[-2.0, 0.0, 2.0]);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(wav.len(), 50);
        assert_eq!(i16::from_le_bytes(wav[44..46].try_into().unwrap()), -32_767);
        assert_eq!(i16::from_le_bytes(wav[48..50].try_into().unwrap()), 32_767);
    }

    #[test]
    fn asr_response_keeps_segment_order() {
        let response: AsrResponse =
            serde_json::from_str(r#"{"segments":[{"text":"第一段"},{"text":"第二段"}]}"#).unwrap();
        assert_eq!(
            format_transcript_text(response.segments.as_deref().unwrap_or_default()),
            "第一段 第二段"
        );
    }

    #[test]
    fn asr_response_prefixes_identified_speakers() {
        let response: AsrResponse = serde_json::from_str(
            r#"{"segments":[{"text":"你好","speaker":"发言人1","speaker_id":1},{"text":"请坐","speaker":"发言人待识别"},{"text":"好的","speaker":"发言人2","speaker_id":2}],"diarization":"active"}"#,
        )
        .unwrap();
        assert_eq!(
            format_transcript_text(response.segments.as_deref().unwrap_or_default()),
            "发言人1：你好 请坐 发言人2：好的"
        );
    }

    pub(super) static SESSION_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn meeting_session_id_is_positive() {
        let _guard = SESSION_TEST_LOCK.lock().unwrap();
        let meeting_id = begin_meeting_session();
        assert!(meeting_id > 0);
        assert_eq!(current_meeting_id(), meeting_id);
        set_chunk_start_ms(3500);
        assert_eq!(CURRENT_CHUNK_START_MS.load(Ordering::SeqCst), 3500);
        CURRENT_MEETING_ID.store(0, Ordering::SeqCst);
    }

    #[test]
    fn rk_asr_is_the_default_batch_provider() {
        assert!(is_rk_asr_provider(None));
        assert!(is_rk_asr_provider(Some("rk-asr")));
        assert!(!is_rk_asr_provider(Some("whisper")));
        assert!(!is_rk_asr_provider(Some("parakeet")));
    }

    #[test]
    fn skip_vad_guard_resets_on_drop() {
        let _guard = SESSION_TEST_LOCK.lock().unwrap();
        set_skip_vad(false);
        {
            let _guard = SkipVadGuard::enable();
            assert!(SKIP_VAD.load(Ordering::SeqCst));
        }
        assert!(!SKIP_VAD.load(Ordering::SeqCst));
    }

    #[test]
    fn empty_vad_index_error_is_detected() {
        let detail = r#"{"detail":"IndexError: list index out of range fsmnvad-offline.onnx"}"#;
        assert!(is_empty_vad_failure(detail));
        assert_eq!(sanitize_asr_detail(detail), "no speech in this chunk");
    }

    #[test]
    fn prepare_asr_audio_raises_quiet_peak() {
        let prepared = prepare_asr_audio(&[0.05, -0.04, 0.03]);
        let peak = prepared.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        // The configured gain cap is 12x: a 0.05 peak becomes 0.6.
        assert!((peak - 0.6).abs() < 1e-6);
        assert!(prepared.len() > 3);
    }

    #[test]
    fn completed_finalize_response_preserves_final_speaker_segments() {
        let payload: FinalizeResponse = serde_json::from_str(
            r#"{"status":"completed","segments":[{"start_ms":0,"end_ms":1200,"text":"你好","speaker_id":2,"speaker":"发言人 2"}]}"#,
        )
        .unwrap();
        let result = calibration_result(payload);
        assert!(result.finalized);
        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.segments[0].speaker.as_deref(), Some("发言人 2"));
    }
}
