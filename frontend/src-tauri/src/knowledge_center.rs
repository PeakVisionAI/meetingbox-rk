//! Administrator-gated bridge to the local knowledge service. No model or shell tools are exposed to the renderer.
use std::sync::{Mutex, atomic::{AtomicBool, Ordering}};
use serde_json::{Value, json};
use tauri::{State, Manager};
use crate::state::AppState;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static SESSION: Mutex<Option<String>> = Mutex::new(None);

pub fn require_meeting_mode() -> Result<(), String> {
    if ACTIVE.load(Ordering::SeqCst) { Err("请先退出企业知识服务中心，再开始会议".into()) } else { Ok(()) }
}

async fn request(action: &str, payload: Value, session: &str) -> Result<Value,String> {
    let token=std::fs::read_to_string("D:\\MeetingBox-Intel\\knowledge-center\\service.token")
        .map_err(|_|"知识服务尚未安装，请检查本地服务".to_string())?;
    let client=reqwest::Client::builder().timeout(std::time::Duration::from_secs(600)).build().map_err(|e|e.to_string())?;
    let response=client.post("http://127.0.0.1:8069/rpc")
        .header("X-Knowledge-Token",token.trim()).header("X-Knowledge-Session",session)
        .json(&json!({"action":action,"payload":payload})).send().await.map_err(|e|format!("知识服务连接失败：{e}"))?;
    let status=response.status();
    let value=response.json::<Value>().await.map_err(|e|e.to_string())?;
    if !status.is_success(){return Err(value["detail"].as_str().unwrap_or("知识服务请求失败").to_string());}
    Ok(value)
}

#[tauri::command]
pub async fn knowledge_enter(state: State<'_,AppState>, password: String) -> Result<Value,String> {
    if !crate::access_control::verify_admin_password(state,password).await? {return Err("管理员密码不正确".into());}
    let _guard=crate::audio::common::acquire_engine_lifecycle_lock().await;
    if crate::audio::recording_commands::is_recording().await {return Err("请先结束并保存当前会议".into());}
    if ACTIVE.load(Ordering::SeqCst){
        let previous=SESSION.lock().map_err(|_|"知识会话不可用")?.clone();
        if let Some(previous)=previous {
            if let Ok(status)=request("status",json!({}),&previous).await{return Ok(status);}
        }
    }
    ACTIVE.store(true,Ordering::SeqCst);
    let session=uuid::Uuid::new_v4().to_string();
    match request("enter",json!({}),&session).await {
        Ok(result)=>{*SESSION.lock().map_err(|_|"知识会话不可用")?=Some(session);Ok(result)},
        Err(error)=>{ACTIVE.store(false,Ordering::SeqCst);Err(error)}
    }
}

#[tauri::command]
pub async fn knowledge_request(action:String,payload:Value)->Result<Value,String>{
    crate::access_control::require_admin_authenticated()?;
    let allowed=["status","meetings","detail","archive_draft","archive_save","archive_list","search","chat","document","speaker_save","speaker_list","document_read","document_export","document_open","document_list","document_import"];
    if !allowed.contains(&action.as_str()){return Err("不支持的知识操作".into());}
    let session=SESSION.lock().map_err(|_|"知识会话不可用")?.clone().ok_or("请先验证管理员密码")?;
    request(&action,payload,&session).await
}

#[tauri::command]
pub async fn knowledge_audio(app:tauri::AppHandle,meeting_id:String)->Result<String,String>{
    crate::access_control::require_admin_authenticated()?;
    let session=SESSION.lock().map_err(|_|"知识会话不可用")?.clone().ok_or("请先验证管理员密码")?;
    let response=request("audio_path",json!({"meeting_id":meeting_id}),&session).await?;
    let source=std::path::PathBuf::from(response["path"].as_str().ok_or("录音缓存不可用")?);
    let root=std::path::PathBuf::from("D:\\MeetingBox-Intel\\knowledge-center\\audio-cache");
    if source.parent()!=Some(root.as_path()) || source.extension().and_then(|s|s.to_str())!=Some("mp3"){return Err("录音路径无效".into());}
    let dir=app.path().app_data_dir().map_err(|e|e.to_string())?.join("playback");
    tokio::fs::create_dir_all(&dir).await.map_err(|e|e.to_string())?;
    let target=dir.join(source.file_name().ok_or("录音文件名无效")?);
    if !target.exists(){tokio::fs::copy(source,&target).await.map_err(|e|e.to_string())?;}
    Ok(target.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn knowledge_leave()->Result<(),String>{
    let session=SESSION.lock().map_err(|_|"知识会话不可用")?.clone();
    if let Some(session)=session { request("leave",json!({}),&session).await?; }
    *SESSION.lock().map_err(|_|"知识会话不可用")?=None;
    ACTIVE.store(false,Ordering::SeqCst);
    Ok(())
}
