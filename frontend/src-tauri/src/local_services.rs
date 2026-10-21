use serde::Serialize;
use tauri::Emitter;

#[derive(Clone, Serialize)]
struct Progress { stage: String, completed: usize, total: usize, ready: bool, error: bool }

#[tauri::command]
pub async fn get_local_summary_status() -> Result<serde_json::Value, String> {
    let client=reqwest::Client::builder().timeout(std::time::Duration::from_secs(2)).build().map_err(|e|e.to_string())?;
    let loaded=ready(&client,"http://192.168.44.1:8061/health",false).await;
    let stage=if loaded {"总结模型已就绪"} else {"总结模型在后台准备，不影响录音"};
    Ok(serde_json::json!({"ready":loaded,"stage":stage}))
}

async fn ready(client: &reqwest::Client, url: &str, bridge: bool) -> bool {
    match client.get(url).send().await {
        Ok(r) if r.status().is_success() => r.json::<serde_json::Value>().await
            .map(|v| v["ready"] == true && (!bridge || v["fast_realtime"] == true)).unwrap_or(false),
        _ => false,
    }
}

#[tauri::command]
pub async fn ensure_local_services(app: tauri::AppHandle) -> Result<(), String> {
    if std::env::var("MEETINGBOX_ASR_STREAM").unwrap_or_default() != "1" { return Ok(()); }
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(2)).build().map_err(|e| e.to_string())?;
    let bridge = format!("{}/healthz", std::env::var("MEETINGBOX_ASR_URL").unwrap_or_else(|_| "http://192.168.44.50:8020".into()).trim_end_matches('/'));
    // Speech gates entry; the summary worker warms independently after entry.
    let urls = ["http://192.168.44.1:8060/health", "http://192.168.44.1:8063/health", &bridge];
    let names = ["语音识别与字词对齐", "语音标点", "实时转写与说话人识别"];
    let started = std::time::Instant::now();
    #[cfg(target_os="windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = std::process::Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-WindowStyle", "Hidden", "-ExecutionPolicy", "Bypass", "-File", "D:\\MeetingBox-Intel\\app\\Ensure-LocalServices.ps1"]);
        cmd.creation_flags(0x08000000);
        cmd.spawn().map_err(|e| format!("无法启动语音服务：{e}"))?;
    }
    loop {
        let (a,b,c) = tokio::join!(ready(&client, urls[0], false), ready(&client, urls[1], false), ready(&client, urls[2], true));
        let states = [a,b,c];
        let completed = states.iter().filter(|s| **s).count();
        let stage = if completed == 3 { "语音服务已就绪，总结模型在后台准备".to_string() }
            else { format!("正在准备{}", names[states.iter().position(|s| !s).unwrap()]) };
        let _ = app.emit("local-services-progress", Progress {stage,completed,total:3,ready:completed==3,error:false});
        if completed == 3 { return Ok(()); }
        if started.elapsed().as_secs() >= 180 {
            let missing = names.iter().zip(states).filter_map(|(n,s)| if s {None} else {Some(*n)}).collect::<Vec<_>>().join("、");
            let message = format!("{}尚未就绪，请检查后台启动状态后重试", missing);
            let _ = app.emit("local-services-progress", Progress {stage:message.clone(),completed,total:3,ready:false,error:true});
            return Err(message);
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

