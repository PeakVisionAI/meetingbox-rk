// RK3588 ASR service provider for the Meeting Box distribution.

use super::provider::{TranscriptResult, TranscriptionError, TranscriptionProvider};
use async_trait::async_trait;
use log::{info, warn};
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use serde_json::json;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const RK_ASR_ENDPOINT: &str = "http://127.0.0.1:8020";
pub const RK_ASR_MODEL: &str = "sensevoice-rknn";
const SAMPLE_RATE: u32 = 16_000;
const MINIMUM_SAMPLES: usize = 16_000; // 1s at 16kHz; shorter clips crash SenseVoice fsmnvad

static CURRENT_MEETING_ID: AtomicI64 = AtomicI64::new(0);
static CURRENT_CHUNK_START_MS: AtomicI64 = AtomicI64::new(0);
static MEETING_SEQ: AtomicI64 = AtomicI64::new(1);

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

pub struct RkAsrProvider {
    endpoint: String,
    client: reqwest::Client,
}

impl RkAsrProvider {
    pub fn new(endpoint: impl Into<String>) -> Result<Self, TranscriptionError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(25))
            .build()
            .map_err(|error| TranscriptionError::EngineFailed(error.to_string()))?;

        Ok(Self {
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
            client,
        })
    }

    pub fn local() -> Result<Self, TranscriptionError> {
        Self::new(RK_ASR_ENDPOINT)
    }

    pub async fn finalize_current_meeting(&self) -> Result<(), TranscriptionError> {
        let meeting_id = CURRENT_MEETING_ID.swap(0, Ordering::SeqCst);
        CURRENT_CHUNK_START_MS.store(0, Ordering::SeqCst);
        if meeting_id <= 0 {
            return Ok(());
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

        if !response.status().is_success() {
            let status = response.status();
            let detail = response.text().await.unwrap_or_default();
            return Err(TranscriptionError::EngineFailed(format!(
                "RK ASR finalize returned HTTP {}: {}",
                status,
                detail.chars().take(500).collect::<String>()
            )));
        }

        info!("RK ASR finalize requested for meeting_id={}", meeting_id);
        Ok(())
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
            .text("chunk_start_ms", chunk_start_ms.to_string());

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
        let text = format_transcript_text(&payload.segments);
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
        self.is_model_loaded().await.then(|| RK_ASR_MODEL.to_string())
    }

    fn provider_name(&self) -> &'static str {
        "RK ASR"
    }
}

pub fn begin_meeting_session() -> i64 {
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

pub fn current_meeting_id() -> i64 {
    CURRENT_MEETING_ID.load(Ordering::SeqCst)
}

pub fn is_rk_asr_provider(provider: Option<&str>) -> bool {
    !matches!(provider, Some("parakeet" | "whisper" | "localWhisper"))
}

pub async fn finalize_current_meeting() {
    match RkAsrProvider::local() {
        Ok(provider) => {
            if let Err(error) = provider.finalize_current_meeting().await {
                warn!("RK ASR finalize failed: {}", error);
            }
        }
        Err(error) => warn!("RK ASR finalize skipped: {}", error),
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
}
