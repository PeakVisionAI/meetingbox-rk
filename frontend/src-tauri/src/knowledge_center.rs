//! Administrator-gated bridge to the local knowledge service.
//!
//! No model or shell tools are exposed to the renderer: the frontend may only call
//! the whitelisted actions below, and the service token never leaves this process.
//! Paths and endpoint come from the platform profile.

use crate::platform;
use crate::state::AppState;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{Manager, State};

static ACTIVE: AtomicBool = AtomicBool::new(false);
static SESSION: Mutex<Option<String>> = Mutex::new(None);

pub fn require_meeting_mode() -> Result<(), String> {
    if ACTIVE.load(Ordering::SeqCst) {
        Err("请先退出企业知识服务中心，再开始会议".into())
    } else {
        Ok(())
    }
}

fn ensure_enabled() -> Result<(), String> {
    if platform::current().knowledge_enabled {
        Ok(())
    } else {
        Err("此设备未启用企业知识服务中心".to_string())
    }
}

/// Portable fallback when the profile does not pin an absolute install path.
fn default_root(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join("knowledge-center"))
        .unwrap_or_else(|_| PathBuf::from("knowledge-center"))
}

fn token_path(app: &tauri::AppHandle) -> PathBuf {
    platform::current()
        .knowledge_token_path
        .clone()
        .unwrap_or_else(|| default_root(app).join("service.token"))
}

fn audio_cache_root(app: &tauri::AppHandle) -> PathBuf {
    platform::current()
        .knowledge_audio_cache
        .clone()
        .unwrap_or_else(|| default_root(app).join("audio-cache"))
}

async fn request(
    app: &tauri::AppHandle,
    action: &str,
    payload: Value,
    session: &str,
) -> Result<Value, String> {
    let token = std::fs::read_to_string(token_path(app))
        .map_err(|_| "知识服务尚未安装，请检查本地服务".to_string())?;
    let endpoint = format!("{}/rpc", platform::current().knowledge_endpoint);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .post(endpoint)
        .header("X-Knowledge-Token", token.trim())
        .header("X-Knowledge-Session", session)
        .json(&json!({"action": action, "payload": payload}))
        .send()
        .await
        .map_err(|e| format!("知识服务连接失败：{e}"))?;
    let status = response.status();
    let value = response.json::<Value>().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(value["detail"]
            .as_str()
            .unwrap_or("知识服务请求失败")
            .to_string());
    }
    Ok(value)
}

#[tauri::command]
pub async fn knowledge_enter(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    password: String,
) -> Result<Value, String> {
    ensure_enabled()?;
    if !crate::access_control::verify_admin_password(state, password).await? {
        return Err("管理员密码不正确".into());
    }
    let _guard = crate::audio::common::acquire_engine_lifecycle_lock().await;
    if crate::audio::recording_commands::is_recording().await {
        return Err("请先结束并保存当前会议".into());
    }
    if ACTIVE.load(Ordering::SeqCst) {
        let previous = SESSION.lock().map_err(|_| "知识会话不可用")?.clone();
        if let Some(previous) = previous {
            if let Ok(status) = request(&app, "status", json!({}), &previous).await {
                return Ok(status);
            }
        }
    }
    ACTIVE.store(true, Ordering::SeqCst);
    let session = uuid::Uuid::new_v4().to_string();
    match request(&app, "enter", json!({}), &session).await {
        Ok(result) => {
            *SESSION.lock().map_err(|_| "知识会话不可用")? = Some(session);
            Ok(result)
        }
        Err(error) => {
            ACTIVE.store(false, Ordering::SeqCst);
            Err(error)
        }
    }
}

#[tauri::command]
pub async fn knowledge_request(
    app: tauri::AppHandle,
    action: String,
    payload: Value,
) -> Result<Value, String> {
    ensure_enabled()?;
    crate::access_control::require_admin_authenticated()?;
    let allowed = [
        "status",
        "meetings",
        "detail",
        "archive_draft",
        "archive_save",
        "archive_list",
        "search",
        "chat",
        "chat_poll",
        "document",
        "speaker_save",
        "speaker_list",
        "document_read",
        "document_export",
        "document_open",
        "document_list",
        "document_import",
    ];
    if !allowed.contains(&action.as_str()) {
        return Err("不支持的知识操作".into());
    }
    let session = SESSION
        .lock()
        .map_err(|_| "知识会话不可用")?
        .clone()
        .ok_or("请先验证管理员密码")?;
    request(&app, &action, payload, &session).await
}

#[tauri::command]
pub async fn knowledge_audio(
    app: tauri::AppHandle,
    meeting_id: String,
) -> Result<String, String> {
    ensure_enabled()?;
    crate::access_control::require_admin_authenticated()?;
    let session = SESSION
        .lock()
        .map_err(|_| "知识会话不可用")?
        .clone()
        .ok_or("请先验证管理员密码")?;
    let response = request(&app, "audio_path", json!({ "meeting_id": meeting_id }), &session).await?;
    let source = PathBuf::from(response["path"].as_str().ok_or("录音缓存不可用")?);
    let root = audio_cache_root(&app);
    if source.parent() != Some(root.as_path())
        || source.extension().and_then(|s| s.to_str()) != Some("mp3")
    {
        return Err("录音路径无效".into());
    }
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("playback");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let target = dir.join(source.file_name().ok_or("录音文件名无效")?);
    if !target.exists() {
        tokio::fs::copy(source, &target)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(target.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn knowledge_leave(app: tauri::AppHandle) -> Result<(), String> {
    let session = SESSION.lock().map_err(|_| "知识会话不可用")?.clone();
    if let Some(session) = session {
        request(&app, "leave", json!({}), &session).await?;
    }
    *SESSION.lock().map_err(|_| "知识会话不可用")? = None;
    ACTIVE.store(false, Ordering::SeqCst);
    Ok(())
}
