use tauri::{AppHandle, Emitter, Runtime};
use super::provider::TranscriptionError;
use super::worker::{TranscriptUpdate, emit_continuous_segment};
use base64::Engine;

pub fn enabled() -> bool {
    std::env::var("MEETINGBOX_ASR_STREAM").map(|v| v == "1").unwrap_or(false)
}

fn endpoint() -> String {
    std::env::var("MEETINGBOX_ASR_URL").unwrap_or_else(|_| "http://127.0.0.1:8020".into()).trim_end_matches('/').to_string()
}

fn emit<R: Runtime>(app: &AppHandle<R>, response: &serde_json::Value) {
    if let Some(segments) = response["segments"].as_array() {
        for segment in segments {
            let text = segment["text"].as_str().unwrap_or("").trim();
            if text.is_empty() { continue; }
            let speaker = segment["speaker"].as_str().filter(|s| !s.is_empty()).or_else(|| if response["upsert"].as_bool().unwrap_or(false) { Some("发言人待识别") } else { None });
            let start = segment["speech_start_ms"].as_f64().or_else(|| segment["start_ms"].as_f64()).unwrap_or(0.0) / 1000.0;
            let end = segment["speech_end_ms"].as_f64().or_else(|| segment["end_ms"].as_f64()).unwrap_or(0.0).max(start * 1000.0) / 1000.0;
            let update = TranscriptUpdate {
                text: super::rk_asr_provider::apply_hotword_corrections(speaker.map(|s| format!("{}：{}", s, text)).unwrap_or_else(|| text.to_string())),
                timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
                source: if response["upsert"].as_bool().unwrap_or(false) { "LiveStream".into() } else { "Audio".into() }, sequence_id: if response["upsert"].as_bool().unwrap_or(false) { segment["utterance"].as_u64().unwrap_or(0) + 1 } else { 0 }, chunk_start_time: start,
                is_partial: segment["is_partial"].as_bool().unwrap_or(false), confidence: 0.85,
                audio_start_time: start, audio_end_time: end, duration: (end-start).max(0.0),
            };
            emit_continuous_segment(app, update);
        }
    }
}

pub async fn feed<R: Runtime>(app: &AppHandle<R>, samples: &[f32], index: u64) -> Result<(), TranscriptionError> {
    let mut pcm = Vec::with_capacity(samples.len()*2);
    for sample in samples { pcm.extend_from_slice(&((sample.clamp(-1.0,1.0)*32767.0).round() as i16).to_le_bytes()); }
    let meeting = super::rk_asr_provider::current_meeting_id();
    let response = reqwest::Client::new().post(format!("{}/v1/asr/stream/{}?chunk_index={}",endpoint(),meeting,index))
        .header("Content-Type","application/octet-stream").body(pcm)
        .header("X-Meeting-Hotwords",base64::engine::general_purpose::STANDARD.encode(super::rk_asr_provider::configured_hotwords_json().as_bytes()))
        .timeout(std::time::Duration::from_secs(120)).send().await
        .map_err(|e| TranscriptionError::EngineFailed(e.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        let detail=response.text().await.unwrap_or_default();
        let _=app.emit("transcription-error",serde_json::json!({"userMessage":"实时转写中断，原始录音仍保留，请结束会议后校准。","error":detail}));
        return Err(TranscriptionError::EngineFailed(format!("continuous ASR HTTP {}",status)));
    }
    let data = response.json::<serde_json::Value>().await.map_err(|e| TranscriptionError::EngineFailed(e.to_string()))?;
    emit(app,&data);
    Ok(())
}

pub async fn end<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let response=reqwest::Client::new().post(format!("{}/v1/asr/stream/{}/end",endpoint(),super::rk_asr_provider::current_meeting_id()))
        .timeout(std::time::Duration::from_secs(320)).send().await.map_err(|e|e.to_string())?
        .error_for_status().map_err(|e|e.to_string())?;
    let data=response.json::<serde_json::Value>().await.map_err(|e|e.to_string())?;
    emit(app,&data);
    Ok(())
}
