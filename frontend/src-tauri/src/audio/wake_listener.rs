//! Linux idle-only capture. The capture task exclusively owns and reaps parec.
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
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static SUSPENSIONS: AtomicUsize = AtomicUsize::new(0);
static EXITING: AtomicBool = AtomicBool::new(false);
static STATUS: RwLock<Option<VoiceWakeStatus>> = RwLock::new(None);

pub fn status() -> VoiceWakeStatus {
    STATUS.read().unwrap().clone().unwrap_or(VoiceWakeStatus {
        enabled: false,
        state: "disabled".into(),
        detail: Some(if cfg!(target_os = "linux") {
            "需要显式启用空闲语音唤醒；会议期间暂停监听".into()
        } else {
            "原生空闲语音唤醒仅支持 Linux".into()
        }),
        wake_word: "小峰小峰".into(),
    })
}

fn publish<R: Runtime>(app: &AppHandle<R>, state: &str, detail: Option<&str>) {
    let mut current = status();
    current.enabled = ENABLED.load(Ordering::SeqCst);
    if current.state != state || current.enabled != ENABLED.load(Ordering::SeqCst) {
        log::info!("Idle voice wake state={} enabled={}", state, ENABLED.load(Ordering::SeqCst));
    }
    current.state = state.into();
    current.detail = detail.map(str::to_owned);
    *STATUS.write().unwrap() = Some(current.clone());
    let _ = app.emit("voice-wake-status", current);
}

fn event<R: Runtime>(app: &AppHandle<R>, kind: &str, detail: &str) {
    let _ = app.emit("voice-wake-event", serde_json::json!({
        "type": kind, "detail": detail, "wake_word": status().wake_word,
    }));
}

/// Held from before device acquisition through complete recording cleanup.
/// Drop also resumes after any early return or cancelled start future.
pub struct RecordingSuspension;
impl Drop for RecordingSuspension {
    fn drop(&mut self) {
        SUSPENSIONS.fetch_sub(1, Ordering::SeqCst);
    }
}

pub async fn suspend_for_recording() -> RecordingSuspension {
    SUSPENSIONS.fetch_add(1, Ordering::SeqCst);
    let guard = RecordingSuspension;
    #[cfg(target_os = "linux")]
    linux::stop_capture().await;
    guard
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
        // The same lock serializes enable/disable, recording suspension and restart.
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
        SUSPENSIONS.load(Ordering::SeqCst) != 0 || rk_policy::operation_active()
    }

    pub(super) async fn supervise<R: Runtime>(app: AppHandle<R>) {
        let mut retry_at = Instant::now();
        loop {
            {
                let mut control = CONTROL.lock().await;
                if EXITING.load(Ordering::SeqCst) {
                    stop_locked(&mut control).await;
                    return;
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
        let mut command = Command::new("/usr/bin/parec");
        command.args(["--format=s16le", "--rate=16000", "--channels=1", "--raw", "--latency-msec=100"])
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
        if let Ok(source) = std::env::var("MEETINGBOX_WAKE_SOURCE") {
            if !source.trim().is_empty() { command.arg("--device").arg(source); }
        }
        let mut child = command.spawn().map_err(|_| "无法启动 /usr/bin/parec 空闲音频采集")?;
        log::info!("Idle voice capture started pid={:?}", child.id());
        // Once spawned, all exits below pass through kill + wait and session reset.
        let mut stdout = child.stdout.take().expect("piped parec stdout");
        let mut session = uuid::Uuid::new_v4().to_string();
        let mut seq = 0u64;
        let mut window = WakeWindow::default();
        let mut pcm = [0u8; FRAME_BYTES];
        publish(app, "listening", Some("空闲监听中；会议期间暂停"));
        let outcome = async {
            loop {
                if window.expired(Instant::now()) {
                    event(app, "timeout", "等待语音指令或录音启动超时");
                    reset(&client, &url, &session).await;
                    session = uuid::Uuid::new_v4().to_string();
                    seq = 0;
                    window = WakeWindow::default();
                    publish(app, "listening", None);
                }
                tokio::time::timeout(Duration::from_secs(2), stdout.read_exact(&mut pcm)).await
                    .map_err(|_| "空闲音频采集超时".to_string())?
                    .map_err(|_| "空闲音频采集已结束".to_string())?;
                let now = Instant::now();
                let form = reqwest::multipart::Form::new()
                    .text("session_id", session.clone()).text("seq", seq.to_string())
                    .text("mode", "wake").text("expect_followup", window.awaiting(now).to_string())
                    .text("suppress", window.suppressed(now).to_string())
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
                    match item.kind.as_str() {
                        "wake" => {
                            window.wake(now);
                            if let Some(word) = item.keyword {
                                let mut current = status(); current.wake_word = word;
                                *STATUS.write().unwrap() = Some(current);
                            }
                            publish(app, "awaiting_command", None);
                            crate::tray::focus_main_window(app);
                            event(app, "wake", "请说开始会议或开始录音");
                        }
                        "utterance" if matches!(item.trigger.as_deref(), Some("wake" | "followup")) => {
                            if window.accept(item.text.as_deref().unwrap_or(""), now) {
                                publish(app, "paused", Some("正在等待录音启动"));
                                event(app, "start", "正在开始会议录音");
                                app.emit("request-start-recording", ()).map_err(|_| "无法请求开始录音".to_string())?;
                            } else if window.awaiting(now) {
                                event(app, "cancel", "仅支持开始会议或开始录音指令");
                                reset(&client, &url, &session).await;
                                session = uuid::Uuid::new_v4().to_string(); seq = 0;
                                window = WakeWindow::default(); publish(app, "listening", None);
                                break;
                            }
                        }
                        "dropped" | "barge" => {
                            event(app, if item.reason.as_deref() == Some("wake_timeout") { "timeout" } else { "cancel" },
                                "语音指令已取消");
                            reset(&client, &url, &session).await;
                            session = uuid::Uuid::new_v4().to_string(); seq = 0;
                            window = WakeWindow::default(); publish(app, "listening", None);
                            break;
                        }
                        _ => {} // Partial text can never trigger an action.
                    }
                }
            }
        };
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Ok(()),
            _ = async { loop { if busy() { break; } tokio::time::sleep(Duration::from_millis(100)).await; } } => Ok(()),
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
