use crate::access_control::require_meeting_unlocked;
use crate::state::AppState;
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tauri::State;

const DEFAULT_SUMMARY_SERVICE_URL: &str = "http://127.0.0.1:8050";
const MAX_MINUTE_TRANSCRIPT_CHARS: usize = 12_000;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LiveSummarySegment {
    pub text: String,
    pub speaker: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MinuteSummaryDraft {
    pub minute_index: i64,
    pub summary: String,
    pub structure: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MinuteSummaryRecord {
    pub minute_index: i64,
    pub start_seconds: i64,
    pub end_seconds: i64,
    pub summary: String,
}

fn service_template(template_id: &str) -> &'static str {
    match template_id {
        "project_sync" | "retrospective" => "review",
        "sales_marketing_client_call" | "psychatric_session" => "interview",
        _ => "weekly",
    }
}

fn summary_text(structure: &Value) -> String {
    if let Some(summary) = structure.get("summary").and_then(Value::as_str) {
        if !summary.trim().is_empty() {
            return summary.trim().to_string();
        }
    }
    ["decisions", "agenda", "todos", "risks"]
        .iter()
        .filter_map(|key| structure.get(key).and_then(Value::as_array))
        .flatten()
        .filter_map(|item| {
            item.as_str()
                .or_else(|| item.get("task").and_then(Value::as_str))
        })
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("；")
}

#[tauri::command]
pub async fn api_generate_minute_summary(
    template_id: String,
    minute_index: i64,
    segments: Vec<LiveSummarySegment>,
) -> Result<MinuteSummaryDraft, String> {
    if minute_index < 0 {
        return Err("分时摘要序号无效".to_string());
    }
    let total_chars = segments
        .iter()
        .map(|segment| segment.text.chars().count())
        .sum::<usize>();
    if segments.is_empty() || total_chars == 0 {
        return Err("本时段没有有效转写".to_string());
    }
    if total_chars > MAX_MINUTE_TRANSCRIPT_CHARS {
        return Err("本时段转写内容过长".to_string());
    }

    let base_url = std::env::var("MEETINGBOX_SUMMARY_URL")
        .unwrap_or_else(|_| DEFAULT_SUMMARY_SERVICE_URL.to_string());
    let response = Client::builder()
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|error| format!("无法初始化摘要服务连接: {}", error))?
        .post(format!(
            "{}/v1/summary/generate",
            base_url.trim_end_matches('/')
        ))
        .json(&json!({
            "meeting_id": 0,
            "template": service_template(&template_id),
            "segments": segments,
            "incremental": true
        }))
        .send()
        .await
        .map_err(|error| format!("分时摘要服务不可用: {}", error))?;

    if !response.status().is_success() {
        let status = response.status();
        let message = response.text().await.unwrap_or_default();
        return Err(format!("分时摘要生成失败 ({}): {}", status, message));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|error| format!("摘要响应格式无效: {}", error))?;
    let structure = body.get("structure").cloned().unwrap_or_else(|| json!({}));
    let summary = summary_text(&structure);
    if summary.is_empty() {
        return Err("摘要服务返回了空内容".to_string());
    }
    Ok(MinuteSummaryDraft {
        minute_index,
        summary,
        structure,
    })
}

#[tauri::command]
pub async fn api_save_meeting_minute_summaries(
    state: State<'_, AppState>,
    meeting_id: String,
    summaries: Vec<MinuteSummaryRecord>,
) -> Result<(), String> {
    require_meeting_unlocked(&meeting_id)?;
    let mut transaction = state
        .db_manager
        .pool()
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    for summary in summaries {
        if summary.minute_index < 0
            || summary.start_seconds < 0
            || summary.end_seconds <= summary.start_seconds
        {
            return Err("分时摘要时间范围无效".to_string());
        }
        sqlx::query(
            "INSERT INTO meeting_minute_summaries (meeting_id, minute_index, start_seconds, end_seconds, summary, created_at) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(meeting_id, minute_index) DO UPDATE SET start_seconds = excluded.start_seconds, end_seconds = excluded.end_seconds, summary = excluded.summary, created_at = excluded.created_at",
        )
        .bind(&meeting_id)
        .bind(summary.minute_index)
        .bind(summary.start_seconds)
        .bind(summary.end_seconds)
        .bind(summary.summary)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    }
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn api_get_meeting_minute_summaries(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<MinuteSummaryRecord>, String> {
    require_meeting_unlocked(&meeting_id)?;
    let rows = sqlx::query_as::<_, (i64, i64, i64, String)>(
        "SELECT minute_index, start_seconds, end_seconds, summary FROM meeting_minute_summaries WHERE meeting_id = ? ORDER BY minute_index",
    )
    .bind(meeting_id)
    .fetch_all(state.db_manager.pool())
    .await
    .map_err(|error| error.to_string())?;
    Ok(rows
        .into_iter()
        .map(
            |(minute_index, start_seconds, end_seconds, summary)| MinuteSummaryRecord {
                minute_index,
                start_seconds,
                end_seconds,
                summary,
            },
        )
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{service_template, summary_text};
    use serde_json::json;

    #[test]
    fn maps_product_templates_to_summary_service_templates() {
        assert_eq!(service_template("standard_meeting"), "weekly");
        assert_eq!(service_template("project_sync"), "review");
        assert_eq!(service_template("custom-1"), "weekly");
    }

    #[test]
    fn extracts_summary_with_structured_fallback() {
        assert_eq!(
            summary_text(&json!({"summary": " 已确认交付范围 "})),
            "已确认交付范围"
        );
        assert_eq!(
            summary_text(&json!({"summary": "", "decisions": ["按期上线"]})),
            "按期上线"
        );
    }
}
