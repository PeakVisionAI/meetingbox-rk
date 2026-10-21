//! Linux voice capture for starting and ending meetings. Owns and reaps parec.
use serde::Serialize;
use std::sync::{atomic::{AtomicBool, AtomicUsize, Ordering}, RwLock};
use tauri::{AppHandle, Emitter, Runtime};

#[derive(Clone, Debug, Serialize)]
pub struct VoiceWakeStatus {
    pub enabled: bool,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub wake_word: String,
    pub context: String,
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static EXITING: AtomicBool = AtomicBool::new(false);
static MICROPHONE_REVISION: AtomicUsize = AtomicUsize::new(0);

pub fn microphone_changed() {
    MICROPHONE_REVISION.fetch_add(1, Ordering::SeqCst);
}
static STATUS: RwLock<Option<VoiceWakeStatus>> = RwLock::new(None);

pub fn status() -> VoiceWakeStatus {
    STATUS.read().unwrap().clone().unwrap_or(VoiceWakeStatus {
        enabled: false,
        state: "disabled".into(),
        detail: Some(if cfg!(target_os = "linux") {
            "需要显式启用空闲语音唤醒；会议中可语音结束会议".into()
        } else {
            "原生空闲语音唤醒仅支持 Linux".into()
        }),
        wake_word: "小峰小峰".into(),
        context: voice_context().as_str().into(),
    })
}

fn publish<R: Runtime>(app: &AppHandle<R>, state: &str, detail: Option<&str>) {
    let mut current = status();
    let context = if state == "paused" {
        super::wake_policy::WakeContext::Transition
    } else {
        voice_context()
    };
    if current.state != state || current.enabled != ENABLED.load(Ordering::SeqCst)
        || current.context != context.as_str() {
        // Fault states are the hard ones to diagnose in the field, so log why.
        if matches!(state, "error" | "unavailable") {
            log::warn!("Idle voice wake state={} enabled={} detail={:?}",
                state, ENABLED.load(Ordering::SeqCst), detail);
        } else {
            log::info!("Idle voice wake state={} enabled={}", state, ENABLED.load(Ordering::SeqCst));
        }
    }
    current.enabled = ENABLED.load(Ordering::SeqCst);
    current.state = state.into();
    current.context = context.as_str().into();
    current.detail = detail.map(str::to_owned);
    *STATUS.write().unwrap() = Some(current.clone());
    let _ = app.emit("voice-wake-status", current);
}

fn event<R: Runtime>(app: &AppHandle<R>, kind: &str, detail: &str) {
    let _ = app.emit("voice-wake-event", serde_json::json!({
        "type": kind, "detail": detail, "wake_word": status().wake_word,
        "context": voice_context().as_str(),
    }));
}

/// A running meeting keeps the same capture open instead of stopping it, so the
/// meeting itself can be ended by voice.
///
/// This is an explicit signal rather than a read of the recording flag: that
/// flag flips only after device setup and model validation, and the gap in
/// between would tear the capture down and rebuild it for no reason.
static MEETING_ACTIVE: AtomicBool = AtomicBool::new(false);
static MEETING_STOPPING: AtomicBool = AtomicBool::new(false);

fn voice_context() -> super::wake_policy::WakeContext {
    use super::wake_policy::WakeContext;
    if MEETING_STOPPING.load(Ordering::SeqCst) {
        WakeContext::Transition
    } else if meeting_active() {
        if super::recording_commands::recording_active() {
            WakeContext::Meeting
        } else {
            WakeContext::Transition
        }
    } else {
        WakeContext::Idle
    }
}

pub struct MeetingStop;

impl Drop for MeetingStop {
    fn drop(&mut self) {
        MEETING_STOPPING.store(false, Ordering::SeqCst);
    }
}

pub fn meeting_stopping() -> MeetingStop {
    MEETING_STOPPING.store(true, Ordering::SeqCst);
    MeetingStop
}

/// Held for as long as a meeting is starting or running. Dropping it (including
/// on any early return from the start path) hands the capture back to the idle
/// policy, so the flag can never be left set.
pub struct MeetingVoice;

impl Drop for MeetingVoice {
    fn drop(&mut self) {
        MEETING_ACTIVE.store(false, Ordering::SeqCst);
    }
}

pub fn meeting_started() -> MeetingVoice {
    MEETING_ACTIVE.store(true, Ordering::SeqCst);
    MeetingVoice
}

fn meeting_active() -> bool {
    MEETING_ACTIVE.load(Ordering::SeqCst)
}

pub fn start<R: Runtime>(app: AppHandle<R>) {
    #[cfg(target_os = "linux")]
    {
        ENABLED.store(std::env::var("MEETINGBOX_WAKE_ENABLED").ok().as_deref() == Some("1"), Ordering::SeqCst);
        publish(&app, if ENABLED.load(Ordering::SeqCst) { "unavailable" } else { "disabled" },
            Some(if ENABLED.load(Ordering::SeqCst) { "正在检查语音唤醒服务" } else { "空闲语音唤醒已关闭" }));
        tauri::async_runtime::spawn(linux::supervise(app));
    }
    #[cfg(not(target_os = "linux"))]
    publish(&app, "disabled", Some("原生空闲语音唤醒仅支持 Linux"));
}

pub async fn set_enabled<R: Runtime>(app: &AppHandle<R>, enabled: bool) -> VoiceWakeStatus {
    #[cfg(target_os = "linux")]
    {
        // The same lock serializes enable/disable and capture restart.
        let mut control = linux::CONTROL.lock().await;
        ENABLED.store(enabled && !EXITING.load(Ordering::SeqCst), Ordering::SeqCst);
        linux::stop_locked(&mut control).await;
        publish(app, if ENABLED.load(Ordering::SeqCst) { "paused" } else { "disabled" },
            if enabled { Some("正在准备空闲监听") } else { Some("空闲语音唤醒已关闭") });
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = enabled;
        publish(app, "disabled", Some("原生空闲语音唤醒仅支持 Linux"));
    }
    status()
}

pub async fn shutdown() {
    EXITING.store(true, Ordering::SeqCst);
    ENABLED.store(false, Ordering::SeqCst);
    #[cfg(target_os = "linux")]
    linux::stop_capture().await;
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use super::super::{transcription::rk_policy, wake_policy::WakeWindow};
    use crate::audio::recording_preferences::load_recording_preferences;
    use serde::Deserialize;
    use std::{process::Stdio, time::{Duration, Instant}};
    use tokio::{io::AsyncReadExt, process::Command, sync::Mutex, task::JoinHandle};
    use tokio_util::sync::CancellationToken;

    const FRAME_BYTES: usize = 16000 * 2 / 5;
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
    pub(super) static CONTROL: Mutex<Option<Capture>> = Mutex::const_new(None);

    pub(super) struct Capture {
        cancel: CancellationToken,
        task: JoinHandle<()>,
    }

    pub(super) async fn stop_locked(control: &mut Option<Capture>) {
        // Leave the handle registered until joined: cancellation of this caller
        // cannot allow a new child to overlap the old child's cleanup.
        if let Some(capture) = control.as_mut() {
            capture.cancel.cancel();
            if let Err(error) = (&mut capture.task).await {
                log::error!("Wake capture task failed: {}", error);
            }
        }
        *control = None;
    }

    pub(super) async fn stop_capture() {
        stop_locked(&mut *CONTROL.lock().await).await;
    }

    fn busy() -> bool {
        // A running meeting is not "busy": the same capture stays open so the
        // meeting can still be ended by voice. Import and retranscription do
        // take the ASR engine and must pause listening.
        rk_policy::operation_active() && !meeting_active()
    }

    pub(super) async fn supervise<R: Runtime>(app: AppHandle<R>) {
        let mut retry_at = Instant::now();
        let mut microphone_revision = MICROPHONE_REVISION.load(Ordering::SeqCst);
        loop {
            {
                let mut control = CONTROL.lock().await;
                if EXITING.load(Ordering::SeqCst) {
                    stop_locked(&mut control).await;
                    return;
                }
                let revision = MICROPHONE_REVISION.load(Ordering::SeqCst);
                if revision != microphone_revision {
                    // Use the existing serialized stop to reap the old child first.
                    stop_locked(&mut control).await;
                    microphone_revision = revision;
                }
                if !ENABLED.load(Ordering::SeqCst) || busy() {
                    stop_locked(&mut control).await;
                    let state = if ENABLED.load(Ordering::SeqCst) { "paused" } else { "disabled" };
                    if status().state != state {
                        publish(&app, state, Some(if state == "paused" {
                            "录音、导入或重新转写期间暂停空闲监听"
                        } else { "空闲语音唤醒已关闭" }));
                    }
                } else {
                    if control.as_ref().is_some_and(|capture| capture.task.is_finished()) {
                        stop_locked(&mut control).await;
                        retry_at = Instant::now() + Duration::from_secs(2);
                    }
                    if control.is_none() && Instant::now() >= retry_at {
                        let cancel = CancellationToken::new();
                        let task = tokio::spawn(capture(app.clone(), cancel.clone()));
                        *control = Some(Capture { cancel, task });
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    #[derive(Deserialize)]
    struct FeedResponse {
        seq: u64,
        events: Vec<VoiceEvent>,
    }
    #[derive(Deserialize)]
    struct VoiceEvent {
        #[serde(rename = "type")]
        kind: String,
        text: Option<String>,
        trigger: Option<String>,
        reason: Option<String>,
        keyword: Option<String>,
    }

    async fn json_response<T: serde::de::DeserializeOwned>(request: reqwest::RequestBuilder) -> Result<T, String> {
        let mut response = request.send().await.map_err(|_| "语音服务连接失败".to_string())?
            .error_for_status().map_err(|_| "语音服务不可用".to_string())?;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "语音服务响应失败".to_string())? {
            if body.len() + chunk.len() > 65536 { return Err("语音服务响应过大".into()); }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| "语音服务响应无效".into())
    }

    fn endpoint() -> Result<String, String> {
        let value = std::env::var("MEETINGBOX_VOICE_URL").unwrap_or_else(|_| "http://127.0.0.1:8021".into());
        let url = reqwest::Url::parse(&value).map_err(|_| "MEETINGBOX_VOICE_URL 无效")?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none()
            || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
            return Err("MEETINGBOX_VOICE_URL 必须是无凭据的 HTTP(S) 地址".into());
        }
        Ok(url.as_str().trim_end_matches('/').into())
    }

    /// Every frame is an independent 200ms mono PCM16 little-endian WAV.
    fn wav(pcm: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(44 + pcm.len());
        output.extend_from_slice(b"RIFF");
        output.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
        output.extend_from_slice(b"WAVEfmt ");
        output.extend_from_slice(&16u32.to_le_bytes());
        output.extend_from_slice(&1u16.to_le_bytes());
        output.extend_from_slice(&1u16.to_le_bytes());
        output.extend_from_slice(&16000u32.to_le_bytes());
        output.extend_from_slice(&32000u32.to_le_bytes());
        output.extend_from_slice(&2u16.to_le_bytes());
        output.extend_from_slice(&16u16.to_le_bytes());
        output.extend_from_slice(b"data");
        output.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        output.extend_from_slice(pcm);
        output
    }

    async fn reset(client: &reqwest::Client, url: &str, session: &str) {
        let _ = tokio::time::timeout(REQUEST_TIMEOUT, client.post(format!("{url}/voice/reset"))
            .form(&[("session_id", session)]).send()).await;
    }

    async fn capture<R: Runtime>(app: AppHandle<R>, cancel: CancellationToken) {
        let result = run_capture(&app, &cancel).await;
        if let Err(detail) = result {
            if !cancel.is_cancelled() && ENABLED.load(Ordering::SeqCst) && !busy() {
                publish(&app, "error", Some(&detail));
            }
        }
    }

    async fn pulse_output(args: &[&str]) -> Result<String, String> {
        let mut command = Command::new("pactl");
        command.args(args).kill_on_drop(true).stdin(Stdio::null()).stderr(Stdio::null());
        let output = tokio::time::timeout(Duration::from_secs(2), command.output()).await
            .map_err(|_| "查询麦克风超时".to_string())?
            .map_err(|_| "无法查询 PulseAudio 麦克风".to_string())?;
        if !output.status.success() || output.stdout.len() > 65536 {
            return Err("PulseAudio 麦克风列表不可用".into());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    async fn microphone_source(requested: Option<&str>) -> Result<String, String> {
        let output = pulse_output(&["list", "short", "sources"]).await?;
        let sources: Vec<String> = output.lines().filter_map(|line| line.split_whitespace().nth(1).map(str::to_owned)).collect();
        if let Some(source) = std::env::var("MEETINGBOX_WAKE_SOURCE").ok().filter(|s| !s.trim().is_empty()) {
            let source = source.trim();
            if sources.iter().any(|s| s == source) {
                // Explicit deployment overrides also support virtual sources for controlled acceptance.
                return Ok(source.to_string());
            }
            return Err("指定的唤醒麦克风当前不可用".into());
        }
        let default = pulse_output(&["get-default-source"]).await.ok();
        super::super::devices::capture_guard::resolve_pulse_microphone_source(requested, &sources, default.as_deref())
    }

    fn publish_listening<R: Runtime>(app: &AppHandle<R>, selected: &str) {
        use super::super::wake_policy::WakeContext;
        let (state, detail) = match voice_context() {
            WakeContext::Idle => ("listening", "空闲监听中"),
            WakeContext::Meeting => ("meeting", "会议进行中，可说“结束会议”"),
            WakeContext::Transition => ("paused", "正在启动或保存会议"),
        };
        publish(app, state, Some(&format!("{detail}；麦克风：{selected}")));
    }

    async fn run_capture<R: Runtime>(app: &AppHandle<R>, cancel: &CancellationToken) -> Result<(), String> {
        let url = endpoint()?;
        let client = reqwest::Client::builder().timeout(REQUEST_TIMEOUT)
            .connect_timeout(Duration::from_secs(1)).no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build().map_err(|_| "无法创建语音服务客户端")?;
        let health: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            response = tokio::time::timeout(REQUEST_TIMEOUT, json_response(client.get(format!("{url}/voice/health")))) => {
                match response {
                    Ok(Ok(value)) => value,
                    _ => { publish(app, "unavailable", Some("语音服务尚未就绪")); return Ok(()); }
                }
            }
        };
        if health.get("available").and_then(|v| v.as_bool()) != Some(true)
            || health.pointer("/kws/loaded").and_then(|v| v.as_bool()) != Some(true)
            || health.pointer("/vad/loaded").and_then(|v| v.as_bool()) != Some(true) {
            publish(app, "unavailable", Some("语音唤醒模型或服务尚未就绪"));
            return Ok(());
        }
        if let Some(word) = health.pointer("/kws/wakeWords/0").and_then(|v| v.as_str()) {
            let mut current = status();
            current.wake_word = word.to_owned();
            *STATUS.write().unwrap() = Some(current);
        }
        if cancel.is_cancelled() || busy() { return Ok(()); }
        let revision = MICROPHONE_REVISION.load(Ordering::SeqCst);
        let preferred = load_recording_preferences(app).await.ok()
            .and_then(|preferences| preferences.preferred_mic_device);
        let selected = microphone_source(preferred.as_deref()).await;
        let selected = match selected {
            Ok(source) => source,
            Err(detail) => { publish(app, "unavailable", Some(&detail)); return Ok(()); }
        };
        let mut command = Command::new("/usr/bin/parec");
        command.args(["--format=s16le", "--rate=16000", "--channels=1", "--raw", "--latency-msec=100"])
            .arg("--device").arg(&selected)
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
        if MICROPHONE_REVISION.load(Ordering::SeqCst) != revision {
            return Ok(()); // Microphone changed while resolving; restart with the new choice.
        }
        let mut child = command.spawn().map_err(|_| "无法启动 /usr/bin/parec 空闲音频采集")?;
        log::info!("Idle voice capture started pid={:?} source={}", child.id(), selected);
        // Once spawned, all exits below pass through kill + wait and session reset.
        let mut stdout = child.stdout.take().expect("piped parec stdout");
        let mut session = uuid::Uuid::new_v4().to_string();
        let mut seq = 0u64;
        let mut window = WakeWindow::default();
        let mut pcm = [0u8; FRAME_BYTES];
        let mut source_check_at = Instant::now() + Duration::from_secs(5);
        let mut published_context = voice_context();
        publish_listening(app, &selected);
        let outcome = async {
            loop {
                let context = voice_context();
                if context != published_context {
                    // A wake from the previous mode must not act on a new meeting.
                    reset(&client, &url, &session).await;
                    session = uuid::Uuid::new_v4().to_string(); seq = 0;
                    window = WakeWindow::default();
                    published_context = context;
                    publish_listening(app, &selected);
                }
                if Instant::now() >= source_check_at {
                    source_check_at = Instant::now() + Duration::from_secs(5);
                    match microphone_source(preferred.as_deref()).await {
                        Ok(source) if source == selected => {}
                        Ok(_) => {
                            log::info!("Idle voice microphone changed; restarting capture");
                            return Ok(());
                        }
                        Err(detail) => {
                            publish(app, "unavailable", Some(&detail));
                            return Ok(());
                        }
                    }
                }
                if let Some(reason) = window.expiry_reason(Instant::now()) {
                    log::info!("Idle voice wake decision reason={}", reason);
                    event(app, "timeout", "等待语音指令或录音启动超时");
                    reset(&client, &url, &session).await;
                    session = uuid::Uuid::new_v4().to_string();
                    seq = 0;
                    window = WakeWindow::default();
                    publish_listening(app, &selected);
                }
                tokio::time::timeout(Duration::from_secs(2), stdout.read_exact(&mut pcm)).await
                    .map_err(|_| "空闲音频采集超时".to_string())?
                    .map_err(|_| "空闲音频采集已结束".to_string())?;
                let now = Instant::now();
                let form = reqwest::multipart::Form::new()
                    .text("session_id", session.clone()).text("seq", seq.to_string())
                    .text("mode", "wake").text("expect_followup", window.awaiting(now).to_string())
                    .text("suppress", (window.suppressed(now)
                        || voice_context() == super::super::wake_policy::WakeContext::Transition).to_string())
                    .text("barge_active", "false")
                    .part("audio", reqwest::multipart::Part::bytes(wav(&pcm)).file_name("idle.wav")
                        .mime_str("audio/wav").map_err(|_| "WAV 编码失败".to_string())?);
                let response: FeedResponse = tokio::time::timeout(REQUEST_TIMEOUT,
                    json_response(client.post(format!("{url}/voice/feed")).multipart(form)))
                    .await.map_err(|_| "语音服务请求超时".to_string())??;
                if response.seq != seq { return Err("语音服务序号不匹配".into()); }
                seq += 1;
                for item in response.events {
                    let now = Instant::now();
                    if busy() || cancel.is_cancelled() || window.suppressed(now) { break; }
                    use super::super::wake_policy::WakeAction;
                    let context = voice_context();
                    if context != published_context { break; }
                    let decision = window.handle_event(context, item.kind.as_str(), item.trigger.as_deref(),
                        item.text.as_deref(), item.reason.as_deref(), now);
                    if !matches!(decision.reason, "partial" | "unhandled_event" | "meeting_transition") {
                        log::info!("Idle voice wake decision reason={}", decision.reason);
                    }
                    match decision.action {
                        WakeAction::Wake => {
                            if let Some(word) = item.keyword {
                                let mut current = status(); current.wake_word = word;
                                *STATUS.write().unwrap() = Some(current);
                            }
                            publish(app, "awaiting_command", None);
                            crate::tray::focus_main_window(app);
                            event(app, "wake", if meeting_active() {
                                "请说结束会议或停止录音"
                            } else {
                                "请说开始会议或开始录音"
                            });
                        }
                        WakeAction::Start => {
                            publish(app, "paused", Some("正在等待录音启动"));
                            event(app, "start", "正在开始会议录音");
                            app.emit("request-start-recording", ()).map_err(|_| "无法请求开始录音".to_string())?;
                        }
                        WakeAction::Stop => {
                            let recording_session = super::super::recording_commands::recording_session_id();
                            publish(app, "paused", Some("正在结束会议"));
                            event(app, "stop", "正在结束会议并保存");
                            // Clear service audio while retaining native action
                            // suppression until shutdown changes the context.
                            reset(&client, &url, &session).await;
                            session = uuid::Uuid::new_v4().to_string(); seq = 0;
                            if let Some(session_id) = recording_session {
                                app.emit("request-stop-recording", serde_json::json!({ "session_id": session_id }))
                                    .map_err(|_| "无法请求结束会议".to_string())?;
                            }
                        }
                        WakeAction::KeepAwaiting => {
                            publish(app, "awaiting_command", None);
                        }
                        WakeAction::Cancel | WakeAction::Timeout => {
                            if decision.reason == "unsupported_command" {
                                event(app, "cancel", if meeting_active() {
                                    "仅支持结束会议或停止录音指令"
                                } else {
                                    "仅支持开始会议或开始录音指令"
                                });
                            } else {
                                event(app, if decision.action == WakeAction::Timeout { "timeout" } else { "cancel" },
                                    "语音指令已取消");
                            }
                            reset(&client, &url, &session).await;
                            session = uuid::Uuid::new_v4().to_string(); seq = 0;
                            window = WakeWindow::default(); publish_listening(app, &selected);
                            break;
                        }
                        WakeAction::Ignore => {} // Partial text can never trigger an action.
                    }
                }
            }
        };
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Ok(()),
            _ = async { loop { if busy() || MICROPHONE_REVISION.load(Ordering::SeqCst) != revision { break; } tokio::time::sleep(Duration::from_millis(100)).await; } } => Ok(()),
            result = outcome => result,
        };
        // stdout is local and the child cannot escape the owner task.
        drop(stdout);
        let _ = child.kill().await;
        child.wait().await.map_err(|_| "无法回收空闲音频采集进程".to_string())?;
        log::info!("Idle voice capture stopped and reaped");
        reset(&client, &url, &session).await;
        result
    }
}
