//! Optional BossAI streaming preview. RK transcripts remain the saved source.
//! `timestamp` is the first sample's recording-relative time, in seconds.

#[path = "live_transcript_protocol.rs"]
mod protocol;

use protocol::{pcm16_wav, AudioBuffer, AudioChunk, Preview, Utterance, CHUNK_SAMPLES, SAMPLE_RATE};
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::{sync::{mpsc, Notify}, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
const STOP_TIMEOUT: Duration = Duration::from_millis(5500);
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
// Bound audio buffering to 38.4 seconds (about 2.4 MiB of PCM), absorbing
// capture bursts and temporary inference slowdowns. A full queue invalidates
// the preview; authoritative recording and transcription continue independently.
const QUEUE_CAPACITY: usize = 64;
static ACTIVE: Mutex<Option<Arc<Input>>> = Mutex::new(None);
// The capture callback cannot wait even for the short registration lock. Report
// a missed submission on the next callback (or stop) rather than hiding a gap.
static MISSED_AUDIO: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize)]
struct TranscriptEvent {
    session_id: String,
    text: String,
    start_time: f64,
    end_time: f64,
    is_final: bool,
}

#[derive(Clone, Serialize)]
struct StatusEvent {
    session_id: String,
    status: &'static str,
    detail: &'static str,
}

fn status<R: Runtime>(app: &AppHandle<R>, session: &str, state: &'static str, detail: &'static str) {
    let _ = app.emit("live-transcript-status", StatusEvent {
        session_id: session.to_owned(), status: state, detail,
    });
}

fn publish<R: Runtime>(app: &AppHandle<R>, session: &str, preview: Preview) {
    let _ = app.emit("live-transcript", TranscriptEvent {
        session_id: session.to_owned(), text: preview.text,
        start_time: preview.start_time, end_time: preview.end_time,
        is_final: preview.is_final,
    });
}

struct Buffered {
    revision: u64,
    audio: AudioBuffer,
}

enum Command {
    Audio(AudioChunk),
    Pause { revision: u64, tail: Option<AudioChunk> },
}

struct Input {
    id: Uuid,
    buffer: Mutex<Buffered>,
    sender: mpsc::Sender<Command>,
    revision: AtomicU64,
    changed: Notify,
    accepting: AtomicBool,
    shutdown: CancellationToken,
    cancel: CancellationToken,
    report: Box<dyn Fn(&'static str, &'static str) + Send + Sync>,
}

impl Input {
    fn invalidate(&self) {
        self.revision.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_one();
    }

    async fn invalidated(&self, revision: u64) {
        loop {
            let notified = self.changed.notified();
            if self.revision.load(Ordering::SeqCst) != revision {
                return;
            }
            notified.await;
        }
    }

    fn detach(&self) {
        self.accepting.store(false, Ordering::SeqCst);
        let mut active = ACTIVE.lock().unwrap_or_else(|error| error.into_inner());
        if active.as_ref().is_some_and(|input| input.id == self.id) {
            if MISSED_AUDIO.swap(false, Ordering::SeqCst) {
                self.invalidate();
            }
            *active = None;
        }
    }
}

/// Keep this owner for the recording's lifetime. Drop cancels immediately;
/// `stop().await` drains queued audio and flushes the server, with a 5.5 s limit.
pub struct LiveTranscript {
    input: Arc<Input>,
    task: Option<JoinHandle<()>>,
}

/// No URL (or an empty URL) disables preview. Invalid configuration emits a
/// degraded status and returns None, so recording can continue independently.
/// The URL is a BossAI base URL, optionally ending in `/asr/transcribe`.
/// Start after validation and before starting capture; no network probe is made.
pub async fn start<R: Runtime>(app: AppHandle<R>, recording_session: String) -> Option<LiveTranscript> {
    let value = match std::env::var("MEETINGBOX_STREAM_ASR_URL") {
        Ok(value) if !value.trim().is_empty() => value,
        Ok(_) | Err(std::env::VarError::NotPresent) => return None,
        Err(_) => {
            status(&app, &recording_session, "degraded", "Invalid streaming ASR URL");
            return None;
        }
    };
    let endpoint = match endpoint(&value) {
        Ok(endpoint) => endpoint,
        Err(detail) => {
            status(&app, &recording_session, "degraded", detail);
            return None;
        }
    };
    if Uuid::parse_str(&recording_session).is_err() {
        status(&app, &recording_session, "degraded", "Invalid recording session UUID");
        return None;
    }
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(1))
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            status(&app, &recording_session, "degraded", "Cannot create streaming ASR client");
            return None;
        }
    };
    let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
    let report_app = app.clone();
    let report_session = recording_session.clone();
    let input = Arc::new(Input {
        id: Uuid::new_v4(),
        buffer: Mutex::new(Buffered { revision: 0, audio: AudioBuffer::new() }),
        sender, revision: AtomicU64::new(0), changed: Notify::new(),
        accepting: AtomicBool::new(true),
        shutdown: CancellationToken::new(), cancel: CancellationToken::new(),
        report: Box::new(move |state, detail| status(&report_app, &report_session, state, detail)),
    });
    let mut active = ACTIVE.lock().unwrap_or_else(|error| error.into_inner());
    if active.is_some() {
        status(&app, &recording_session, "degraded", "A live preview is already active");
        return None;
    }
    MISSED_AUDIO.store(false, Ordering::SeqCst);
    *active = Some(input.clone());
    drop(active);
    let worker_input = input.clone();
    let task = tokio::spawn(async move {
        run(app, recording_session, endpoint, client, worker_input, receiver).await;
    });
    Some(LiveTranscript { input, task: Some(task) })
}

/// Capture-thread entry point: no waits, HTTP, event emission, or unbounded
/// allocation. Feed every mono 16 kHz mix window, including silence.
/// Calls larger than 600 ms are rejected as a discontinuity.
pub fn submit_audio(samples_16k: &[f32], timestamp: f64) {
    if samples_16k.is_empty() {
        return;
    }
    let input = match ACTIVE.try_lock() {
        Ok(active) => active.clone(),
        Err(_) => {
            MISSED_AUDIO.store(true, Ordering::SeqCst);
            return;
        }
    };
    let Some(input) = input else { return };
    if MISSED_AUDIO.swap(false, Ordering::SeqCst) {
        input.invalidate();
    }
    if !input.accepting.load(Ordering::SeqCst) {
        return;
    }
    if samples_16k.len() > CHUNK_SAMPLES || !timestamp.is_finite() || timestamp < 0.0
        || samples_16k.iter().any(|sample| !sample.is_finite())
    {
        input.invalidate();
        return;
    }
    let Ok(mut buffer) = input.buffer.try_lock() else {
        input.invalidate();
        return;
    };
    // A fault may have happened while capture was accumulating this window.
    let revision = input.revision.load(Ordering::SeqCst);
    if buffer.revision != revision {
        buffer.audio.clear();
        buffer.revision = revision;
    }
    let mut offset = 0;
    while offset < samples_16k.len() {
        offset += buffer.audio.append(&samples_16k[offset..], timestamp + offset as f64 / SAMPLE_RATE as f64);
        if buffer.audio.is_full() {
            let chunk = buffer.audio.take(revision).expect("full preview window");
            if input.sender.try_send(Command::Audio(chunk)).is_err() {
                buffer.audio.clear();
                input.invalidate();
                return;
            }
        }
    }
}

/// Call after pausing the pipeline. Flushes the pre-pause tail in sequence, then
/// creates a fresh ASR cache for resume. Does not wait for HTTP. On queue pressure
/// it drops the unfinished preview and resets instead of blocking capture.
pub fn pause() {
    let input = ACTIVE.lock().unwrap_or_else(|error| error.into_inner()).clone();
    let Some(input) = input else { return };
    if !input.accepting.load(Ordering::SeqCst) {
        return;
    }
    let mut buffer = input.buffer.lock().unwrap_or_else(|error| error.into_inner());
    let revision = input.revision.load(Ordering::SeqCst);
    let tail = if buffer.revision == revision { buffer.audio.take(revision) } else { None };
    buffer.audio.clear();
    if input.sender.try_send(Command::Pause { revision, tail }).is_err() {
        input.invalidate();
    }
}

impl LiveTranscript {
    /// Call after pipeline shutdown so all final mix windows have been submitted.
    /// Flush is best-effort within the deadline; a timeout cancels and joins the
    /// task and emits degraded status. Dropping this future also cancels the task.
    pub async fn stop(mut self) {
        self.input.detach();
        self.input.shutdown.cancel();
        if let Some(task) = self.task.as_mut() {
            if tokio::time::timeout(STOP_TIMEOUT, &mut *task).await.is_err() {
                (self.input.report)("degraded", "Streaming preview shutdown timed out");
                self.input.cancel.cancel();
                task.abort();
                let _ = task.await;
            }
        }
        self.task = None;
    }
}

impl Drop for LiveTranscript {
    fn drop(&mut self) {
        self.input.detach();
        self.input.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn endpoint(value: &str) -> Result<String, &'static str> {
    let url = reqwest::Url::parse(value.trim()).map_err(|_| "Invalid streaming ASR URL")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none()
        || !url.username().is_empty() || url.password().is_some()
        || url.query().is_some() || url.fragment().is_some()
    {
        return Err("Streaming ASR URL must be HTTP(S) without credentials, query or fragment");
    }
    let base = url.as_str().trim_end_matches('/');
    Ok(if base.ends_with("/asr/transcribe") { base.to_owned() } else { format!("{base}/asr/transcribe") })
}

#[derive(Deserialize)]
struct AsrResponse {
    text: String,
    #[serde(default, rename = "final", alias = "is_final")]
    is_final: bool,
    #[serde(default, rename = "autoFinal", alias = "auto_final")]
    auto_final: bool,
    #[serde(default)]
    vad: Option<Vad>,
}

#[derive(Deserialize)]
struct Vad {
    #[serde(default)]
    speech: bool,
}

async fn request(client: &reqwest::Client, endpoint: &str, cache: &str, index: u64,
    samples: &[f32], is_final: bool) -> Result<AsrResponse, &'static str>
{
    let audio = Part::bytes(pcm16_wav(samples)).file_name("preview.wav")
        .mime_str("audio/wav").map_err(|_| "Cannot encode preview WAV")?;
    let form = Form::new().part("audio", audio)
        .text("session_id", cache.to_owned()).text("chunk_index", index.to_string())
        .text("min_rms", "0.003").text("is_final", is_final.to_string());
    let mut response = client.post(endpoint).multipart(form).send().await
        .map_err(|_| "Streaming ASR request failed or timed out")?;
    if !response.status().is_success() {
        return Err("Streaming ASR returned an HTTP error");
    }
    if response.content_length().is_some_and(|length| length > MAX_RESPONSE_BYTES as u64) {
        return Err("Streaming ASR response is too large");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "Cannot read streaming ASR response")? {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err("Streaming ASR response is too large");
        }
        body.extend_from_slice(&chunk);
    }
    // Never propagate response bodies or deserializer errors: they may contain text.
    serde_json::from_slice(&body).map_err(|_| "Invalid streaming ASR response")
}

struct Stream {
    cache: String,
    index: u64,
    revision: u64,
    utterance: Utterance,
    end_time: f64,
    used: bool,
    healthy: bool,
}

impl Stream {
    fn new() -> Self {
        Self { cache: Uuid::new_v4().to_string(), index: 0, revision: 0,
            utterance: Utterance::default(), end_time: 0.0, used: false, healthy: false }
    }

    fn reset(&mut self) {
        self.cache = Uuid::new_v4().to_string();
        self.index = 0;
        self.used = false;
    }

    fn fault<R: Runtime>(&mut self, app: &AppHandle<R>, session: &str, input: &Input, detail: &'static str) {
        if let Some(clear) = self.utterance.discard() { publish(app, session, clear); }
        self.reset();
        self.revision = input.revision.load(Ordering::SeqCst);
        self.healthy = false;
        status(app, session, "degraded", detail);
    }

    fn refresh<R: Runtime>(&mut self, app: &AppHandle<R>, session: &str, input: &Input) {
        if input.revision.load(Ordering::SeqCst) != self.revision {
            self.fault(app, session, input, "Preview audio interrupted; stream restarted");
        }
    }

    async fn send<R: Runtime>(&mut self, app: &AppHandle<R>, session: &str,
        client: &reqwest::Client, endpoint: &str, input: &Input,
        samples: &[f32], start: f64, end: f64, final_request: bool) -> bool
    {
        if input.revision.load(Ordering::SeqCst) != self.revision {
            self.refresh(app, session, input);
            return false;
        }
        // A revision change makes this response stale, but must not cancel the
        // request: the sidecar can keep inference and its lock after disconnect.
        // Let it settle (or hit the client timeout) before using a fresh cache.
        // Owner cancellation still interrupts immediately for drop/shutdown.
        let response = tokio::select! {
            biased;
            _ = input.cancel.cancelled() => return false,
            response = request(client, endpoint, &self.cache, self.index, samples, final_request) => response,
        };
        // Suppress stale successes and errors before publishing or resetting.
        if input.revision.load(Ordering::SeqCst) != self.revision {
            self.refresh(app, session, input);
            return false;
        }
        match response {
            Ok(response) => {
                self.index += 1;
                self.used = true;
                self.end_time = end;
                if !self.healthy {
                    status(app, session, "healthy", "Streaming preview connected");
                    self.healthy = true;
                }
                if response.vad.as_ref().is_some_and(|vad| vad.speech) {
                    self.utterance.observe_speech(start, end);
                }
                // In particular, consume final text even with vad.speech=false.
                if let Some(preview) = self.utterance.update(&response.text, start, end,
                    final_request || response.is_final || response.auto_final)
                { publish(app, session, preview); }
                true
            }
            Err(detail) => {
                input.invalidate();
                self.fault(app, session, input, detail);
                false
            }
        }
    }
}

async fn run<R: Runtime>(app: AppHandle<R>, session: String, endpoint: String,
    client: reqwest::Client, input: Arc<Input>, mut receiver: mpsc::Receiver<Command>)
{
    let mut stream = Stream::new();
    status(&app, &session, "connecting", "Waiting for streaming preview response");
    loop {
        stream.refresh(&app, &session, &input);
        let command = tokio::select! {
            biased;
            _ = input.cancel.cancelled() => return,
            _ = input.shutdown.cancelled() => break,
            _ = input.invalidated(stream.revision) => continue,
            command = receiver.recv() => match command { Some(command) => command, None => break },
        };
        process(&app, &session, &client, &endpoint, &input, &mut stream, command).await;
    }
    receiver.close();
    // Drain in capture order. Reserve time for an explicit silence final even if
    // backlog is slow. The owner imposes the overall 5.5 s shutdown deadline.
    let drain = async {
        while let Ok(command) = receiver.try_recv() {
            process(&app, &session, &client, &endpoint, &input, &mut stream, command).await;
        }
        let tail = {
            let mut buffer = input.buffer.lock().unwrap_or_else(|error| error.into_inner());
            let revision = input.revision.load(Ordering::SeqCst);
            if buffer.revision == revision { buffer.audio.take(revision) } else { None }
        };
        if let Some(tail) = tail {
            process(&app, &session, &client, &endpoint, &input, &mut stream, Command::Audio(tail)).await;
        }
    };
    if tokio::time::timeout(Duration::from_millis(1000), drain).await.is_err() {
        input.invalidate();
        stream.fault(&app, &session, &input, "Preview shutdown backlog exceeded its deadline");
    }
    if stream.used {
        let end = stream.end_time;
        // 100 ms of valid silence also works with sidecars rejecting empty WAVs.
        stream.send(&app, &session, &client, &endpoint, &input, &[0.0; 1600], end, end, true).await;
    }
}

async fn process<R: Runtime>(app: &AppHandle<R>, session: &str, client: &reqwest::Client,
    endpoint: &str, input: &Input, stream: &mut Stream, command: Command)
{
    stream.refresh(app, session, input);
    let (chunk, pause) = match command {
        Command::Audio(chunk) => (Some(chunk), false),
        Command::Pause { revision, tail } => {
            if revision != stream.revision { return; }
            (tail, true)
        }
    };
    if let Some(chunk) = chunk {
        if chunk.revision != stream.revision { return; }
        stream.send(app, session, client, endpoint, input,
            &chunk.samples, chunk.start_time, chunk.end_time, false).await;
    }
    if pause {
        if stream.used {
            let end = stream.end_time;
            stream.send(app, session, client, endpoint, input, &[0.0; 1600], end, end, true).await;
        }
        stream.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_accepts_base_or_full_stream_url() {
        assert_eq!(endpoint("http://127.0.0.1:8002/").unwrap(), "http://127.0.0.1:8002/asr/transcribe");
        assert_eq!(endpoint("http://localhost:8002/asr/transcribe").unwrap(), "http://localhost:8002/asr/transcribe");
        for value in ["file:///tmp/asr", "http://user:secret@localhost", "http://localhost?token=secret", "invalid"] {
            assert!(endpoint(value).is_err());
        }
    }

    #[test]
    fn queue_overrun_invalidates_preview_and_old_owner_cannot_detach_new_one() {
        let (sender, mut receiver) = mpsc::channel(QUEUE_CAPACITY);
        let make_input = |id, sender| Arc::new(Input {
            id, buffer: Mutex::new(Buffered { revision: 0, audio: AudioBuffer::new() }),
            sender, revision: AtomicU64::new(0), changed: Notify::new(),
            accepting: AtomicBool::new(true), shutdown: CancellationToken::new(),
            cancel: CancellationToken::new(), report: Box::new(|_, _| {}),
        });
        let old = make_input(Uuid::new_v4(), sender.clone());
        *ACTIVE.lock().unwrap() = Some(old.clone());
        MISSED_AUDIO.store(false, Ordering::SeqCst);
        for index in 0..QUEUE_CAPACITY {
            submit_audio(&vec![0.0; CHUNK_SAMPLES], index as f64 * 0.6);
        }
        assert_eq!(old.revision.load(Ordering::SeqCst), 0);
        submit_audio(&vec![0.0; CHUNK_SAMPLES], QUEUE_CAPACITY as f64 * 0.6);
        assert_eq!(old.revision.load(Ordering::SeqCst), 1);
        assert_eq!(receiver.len(), QUEUE_CAPACITY);
        // All queued commands still carry the obsolete revision and are dropped.
        while let Ok(command) = receiver.try_recv() {
            match command {
                Command::Audio(chunk) => assert_eq!(chunk.revision, 0),
                _ => panic!("expected audio"),
            }
        }
        let new = make_input(Uuid::new_v4(), sender);
        *ACTIVE.lock().unwrap() = Some(new.clone());
        old.detach();
        assert_eq!(ACTIVE.lock().unwrap().as_ref().unwrap().id, new.id);
        new.detach();
        assert!(ACTIVE.lock().unwrap().is_none());
    }

    #[test]
    fn silence_auto_final_keeps_text_and_ignores_untimed_segments() {
        let response: AsrResponse = serde_json::from_str(r#"{"text":"final words","final":true,"autoFinal":true,"vad":{"speech":false},"segments":[{"text":"raw model text"}]}"#).unwrap();
        assert_eq!(response.text, "final words");
        assert!(response.is_final && response.auto_final);
        assert!(!response.vad.unwrap().speech);
    }
}
