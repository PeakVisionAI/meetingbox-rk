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

pub(super) fn configured_hotwords_json() -> String {
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

pub(super) fn apply_hotword_corrections(mut text: String) -> String {
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
    segments: Vec<AsrSegment>,
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
    pub transcription_mode: Option<String>,
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
    #[serde(default)]
    transcription_mode: Option<String>,
}

pub struct RkAsrProvider {
    endpoint: String,
    client: reqwest::Client,
}

impl RkAsrProvider {
    pub fn new(endpoint: impl Into<String>) -> Result<Self, TranscriptionError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|error| TranscriptionError::EngineFailed(error.to_string()))?;

        Ok(Self {
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
            client,
        })
    }

    pub fn local() -> Result<Self, TranscriptionError> {
        Self::new(std::env::var("MEETINGBOX_ASR_URL")
            .unwrap_or_else(|_| RK_ASR_ENDPOINT.to_string()))
    }

    pub async fn finalize_current_meeting(
        &self,
    ) -> Result<SpeakerCalibrationResult, TranscriptionError> {
        let meeting_id = CURRENT_MEETING_ID.swap(0, Ordering::SeqCst);
        CURRENT_CHUNK_START_MS.store(0, Ordering::SeqCst);
        if meeting_id <= 0 {
            return Ok(SpeakerCalibrationResult::default());
        }

        // Continuous stream end has already drained and persisted speaker-labelled
        // utterances. Save immediately; whole-audio reprocessing remains explicit.
        if super::asrserve_stream::enabled() {
            return Ok(SpeakerCalibrationResult {
                transcription_mode: Some("live-stream".to_string()),
                ..SpeakerCalibrationResult::default()
            });
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
        let timeout = std::env::var("MEETINGBOX_FINALIZE_TIMEOUT_SECONDS")
            .ok().and_then(|value| value.parse::<u64>().ok())
            .filter(|value| (60..=14400).contains(value))
            .map(Duration::from_secs).unwrap_or(FINALIZE_TIMEOUT);
        while started.elapsed() < timeout {
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
            timeout.as_secs()
        )))
    }

    pub async fn check_ready(&self) -> Result<bool, TranscriptionError> {
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
        _language: Option<String>,
    ) -> Result<TranscriptResult, TranscriptionError> {
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
                if SKIP_VAD.load(Ordering::SeqCst) { "1" } else { "0" }.to_string(),
            );

        let started = std::time::Instant::now();
        let response = self
            .client
            .post(format!("{}/v1/asr/transcribe-upload", self.endpoint))
            .multipart(form)
            .send()
            .await
            .map_err(|error| TranscriptionError::EngineFailed(error.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            if is_empty_vad_failure(&detail) {
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
        if let Some(diarization) = payload.diarization.as_deref() {
            info!(
                "RK ASR meeting_id={} chunk_start_ms={} diarization={}",
                meeting_id, chunk_start_ms, diarization
            );
        }
        let text = apply_hotword_corrections(format_transcript_text(&payload.segments));
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

    async fn is_model_loaded(&self) -> bool {
        self.check_ready().await.unwrap_or(false)
    }

    async fn get_current_model(&self) -> Option<String> {
        self.is_model_loaded().await.then(|| std::env::var("MEETINGBOX_ASR_MODEL")
            .unwrap_or_else(|_| RK_ASR_MODEL.to_string()))
    }

    fn provider_name(&self) -> &'static str {
        "RK ASR"
    }
}

pub fn begin_meeting_session() -> i64 {
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
        transcription_mode: payload.transcription_mode,
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
    let peak = audio.iter().map(|sample| sample.abs()).fold(0.0f32, f32::max);
    let gain = if peak > 1e-4 { (0.85 / peak).min(12.0) } else { 1.0 };
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

fn encode_pcm16_wav(audio: &[f32]) -> Vec<u8> {
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
        let response: AsrResponse = serde_json::from_str(
            r#"{"segments":[{"text":"第一段"},{"text":"第二段"}]}"#,
        )
        .unwrap();
        assert_eq!(format_transcript_text(&response.segments), "第一段 第二段");
    }

    #[test]
    fn asr_response_prefixes_identified_speakers() {
        let response: AsrResponse = serde_json::from_str(
            r#"{"segments":[{"text":"你好","speaker":"发言人1","speaker_id":1},{"text":"请坐","speaker":"发言人待识别"},{"text":"好的","speaker":"发言人2","speaker_id":2}],"diarization":"active"}"#,
        )
        .unwrap();
        assert_eq!(
            format_transcript_text(&response.segments),
            "发言人1：你好 请坐 发言人2：好的"
        );
    }

    #[test]
    fn meeting_session_id_is_positive() {
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
        assert!(peak > 0.7);
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
