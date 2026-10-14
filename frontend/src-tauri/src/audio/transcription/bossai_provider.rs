// Adapter for BossAI-ASR's standalone FunASR file sidecar.

use super::provider::{TranscriptResult, TranscriptionError, TranscriptionProvider};
use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use std::time::Duration;

pub const BOSSAI_ASR_ENDPOINT: &str = "http://127.0.0.1:8002";
const MINIMUM_SAMPLES: usize = 1600; // 100ms at 16kHz
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
struct HealthResponse {
    ok: bool,
    backend: String,
    #[serde(default)]
    device: Option<String>,
    #[serde(default, rename = "fileLoaded")]
    file_loaded: bool,
}

#[derive(Deserialize)]
struct Segment {
    text: String,
}

#[derive(Deserialize)]
struct FileResponse {
    text: String,
    #[serde(default)]
    segments: Vec<Segment>,
}

pub struct BossAiProvider {
    endpoint: String,
    client: reqwest::Client,
}

impl BossAiProvider {
    pub fn new(endpoint: &str) -> Result<Self, TranscriptionError> {
        let url = reqwest::Url::parse(endpoint)
            .map_err(|_| failure("Invalid MEETINGBOX_BOSSAI_ASR_URL"))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(failure(
                "BossAI ASR URL must be HTTP(S), without credentials, query or fragment",
            ));
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(180))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| failure(error.to_string()))?;
        Ok(Self {
            endpoint: url.as_str().trim_end_matches('/').to_string(),
            client,
        })
    }

    pub async fn check_ready(&self) -> Result<bool, TranscriptionError> {
        self.check_health(false).await
    }

    pub async fn check_cpu_ready(&self) -> Result<bool, TranscriptionError> {
        self.check_health(true).await
    }

    async fn check_health(&self, require_cpu: bool) -> Result<bool, TranscriptionError> {
        let response = self
            .client
            .get(format!("{}/health", self.endpoint))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(transport_error)?;
        let health: HealthResponse = read_response(response).await?;
        // /health can be OK while only the streaming model is loaded.
        Ok(health.ok
            && health.backend == "funasr"
            && health.file_loaded
            && (!require_cpu || health.device.as_deref() == Some("cpu")))
    }
}

#[async_trait]
impl TranscriptionProvider for BossAiProvider {
    async fn transcribe(
        &self,
        audio: Vec<f32>,
        language: Option<String>,
    ) -> Result<TranscriptResult, TranscriptionError> {
        if audio.len() < MINIMUM_SAMPLES {
            return Err(TranscriptionError::AudioTooShort {
                samples: audio.len(),
                minimum: MINIMUM_SAMPLES,
            });
        }
        if audio.iter().any(|sample| !sample.is_finite()) {
            return Err(failure("BossAI ASR audio contains non-finite samples"));
        }
        // Keep the original timeline and amplitude. BossAI owns VAD and normalization;
        // the RK adapter's gain and leading padding must not be applied twice.
        let wav = super::rk_asr_provider::encode_pcm16_wav(&audio);
        let part = Part::bytes(wav)
            .file_name("meetingbox-chunk.wav")
            .mime_str("audio/wav")
            .map_err(|error| failure(error.to_string()))?;
        let language = language
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "auto".to_string());
        let response = self
            .client
            .post(format!("{}/asr/transcribe-file", self.endpoint))
            .multipart(Form::new().part("audio", part).text("language", language))
            .send()
            .await
            .map_err(transport_error)?;
        let payload: FileResponse = read_response(response).await?;
        // BossAI's top-level text includes file-relative timestamps and speaker
        // labels; prefer the plain segment text for meetingbox's own timeline.
        let segments_text = payload
            .segments
            .iter()
            .map(|segment| segment.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let text = if payload.segments.is_empty() {
            payload.text.trim().to_string()
        } else {
            segments_text
        };
        // File-sidecar speaker IDs restart on every upload. Do not present these as
        // meeting-wide identities or overwrite the meeting's playback timestamps.
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
        self.is_model_loaded()
            .await
            .then(|| "bossai-funasr-file".to_string())
    }

    fn provider_name(&self) -> &'static str {
        "BossAI FunASR"
    }
}

fn failure(message: impl Into<String>) -> TranscriptionError {
    TranscriptionError::EngineFailed(message.into())
}

fn transport_error(error: reqwest::Error) -> TranscriptionError {
    failure(format!(
        "BossAI ASR request failed: {}",
        error.without_url()
    ))
}

async fn read_response<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, TranscriptionError> {
    let status = response.status();
    if !status.is_success() {
        // Sidecar errors can contain transcripts or local model paths. Keep those
        // out of desktop logs and UI, while preserving an actionable status.
        return Err(failure(format!("BossAI ASR returned HTTP {status}")));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(failure("BossAI ASR response exceeds 1 MiB"));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| failure("BossAI ASR returned an invalid response"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn server(status: &str, body: &str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0; 8192];
                let count = socket.read(&mut buffer).await.unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
                if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap_or("0")
                        .parse()
                        .unwrap();
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&request).to_string()
        });
        (endpoint, task)
    }

    #[tokio::test]
    async fn health_requires_file_model_loaded() {
        for (body, ready) in [
            (
                r#"{"ok":true,"backend":"funasr","ready":true,"loaded":true,"fileLoaded":false}"#,
                false,
            ),
            (
                r#"{"ok":true,"backend":"funasr","ready":false,"fileLoaded":true}"#,
                true,
            ),
            (
                r#"{"ok":false,"backend":"funasr","fileLoaded":true}"#,
                false,
            ),
        ] {
            let (endpoint, request) = server("200 OK", body).await;
            assert_eq!(
                BossAiProvider::new(&endpoint)
                    .unwrap()
                    .check_ready()
                    .await
                    .unwrap(),
                ready
            );
            assert!(request.await.unwrap().starts_with("GET /health "));
        }
    }

    #[tokio::test]
    async fn uploads_short_speech_as_wav_and_preserves_language() {
        let (endpoint, request) = server("200 OK", r#"{"text":"[00:00-00:01] Speaker A: 你好","segments":[{"speaker":"Speaker A","text":"你好","startMs":null,"endMs":null}]}"#).await;
        let result = BossAiProvider::new(&endpoint)
            .unwrap()
            .transcribe(vec![0.0; 3200], Some("zh".to_string()))
            .await
            .unwrap();
        assert_eq!(result.text, "你好");
        assert_eq!(result.confidence, None);
        assert!(!result.is_partial);
        let request = request.await.unwrap();
        assert!(request.starts_with("POST /asr/transcribe-file "));
        assert!(request.contains("name=\"audio\"; filename=\"meetingbox-chunk.wav\""));
        assert!(request.contains("audio/wav"));
        assert!(request.contains("RIFF"));
        assert!(request.contains("name=\"language\"\r\n\r\nzh"));
        assert!(!request.contains("meeting_id"));
    }

    #[tokio::test]
    async fn segments_fallback_does_not_invent_cross_chunk_speakers() {
        let (endpoint, request) = server("200 OK", r#"{"text":"","segments":[{"speaker":"Speaker A","text":"第一句"},{"speaker":"Speaker B","text":"第二句"}]}"#).await;
        let result = BossAiProvider::new(&endpoint)
            .unwrap()
            .transcribe(vec![0.0; 1600], None)
            .await
            .unwrap();
        assert_eq!(result.text, "第一句 第二句");
        assert!(request
            .await
            .unwrap()
            .contains("name=\"language\"\r\n\r\nauto"));
    }

    #[tokio::test]
    async fn surfaces_http_and_schema_errors() {
        for (status, body, expected) in [
            ("503 Service Unavailable", "private transcript", "503"),
            ("200 OK", r#"{"error":"failed"}"#, "invalid response"),
        ] {
            let (endpoint, request) = server(status, body).await;
            let error = BossAiProvider::new(&endpoint)
                .unwrap()
                .transcribe(vec![0.0; 1600], None)
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected));
            assert!(!error.contains("private transcript"));
            request.await.unwrap();
        }
    }

    #[tokio::test]
    async fn rejects_invalid_audio_before_network_request() {
        let provider = BossAiProvider::new(BOSSAI_ASR_ENDPOINT).unwrap();
        assert!(matches!(
            provider.transcribe(vec![0.0; 100], None).await,
            Err(TranscriptionError::AudioTooShort { .. })
        ));
        assert!(provider
            .transcribe(vec![f32::NAN; 1600], None)
            .await
            .unwrap_err()
            .to_string()
            .contains("non-finite"));
    }

    #[test]
    fn rejects_unsupported_or_credential_bearing_urls() {
        for url in [
            "file:///tmp/asr",
            "http://user:secret@localhost:8002",
            "http://localhost:8002?token=secret",
            "bad url",
        ] {
            assert!(BossAiProvider::new(url).is_err());
        }
    }
}
