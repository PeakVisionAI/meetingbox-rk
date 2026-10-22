//! Product-facing productivity features shared by the files and meeting-detail views.
//!
//! The commands in this module deliberately keep the data local. They provide a
//! small persistence boundary for speaker labels, hotwords and action items while
//! reusing the existing meetings/transcripts tables and access-control checks.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};
use std::fs;
use std::path::Path;
use uuid::Uuid;

use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeakerContact {
    pub id: String,
    pub name: String,
    pub note: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeakerLabel {
    pub meeting_id: String,
    pub speaker_key: String,
    pub display_name: String,
    pub contact_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hotword {
    pub id: String,
    pub phrase: String,
    pub replacement: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingAction {
    pub id: String,
    pub meeting_id: String,
    pub content: String,
    pub assignee: Option<String>,
    pub due_date: Option<String>,
    pub status: String,
    pub source_transcript_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingSearchResult {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub folder_path: Option<String>,
    pub transcript_count: i64,
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedMeetingSearchResponse {
    pub meetings: Vec<MeetingSearchResult>,
    pub total_count: i64,
    pub has_more: bool,
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn metadata_duration_seconds(folder_path: Option<&str>) -> Option<f64> {
    let folder_path = folder_path?.trim();
    if folder_path.is_empty() {
        return None;
    }
    let metadata_path = Path::new(folder_path).join("metadata.json");
    let raw = fs::read_to_string(metadata_path).ok()?;
    let metadata = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    let duration = metadata.get("duration_seconds")?.as_f64()?;
    (duration.is_finite() && duration > 0.0).then_some(duration)
}

fn non_empty(value: String, field: &str) -> Result<String, String> {
    let value = value.trim().to_string();
    if value.is_empty() {
        Err(format!("{}不能为空", field))
    } else {
        Ok(value)
    }
}

fn ensure_meeting_id(meeting_id: &str) -> Result<(), String> {
    if meeting_id.trim().is_empty() {
        Err("会议 ID 不能为空".to_string())
    } else {
        Ok(())
    }
}

pub async fn load_enabled_hotwords(
    pool: &SqlitePool,
) -> Result<Vec<(String, Option<String>)>, String> {
    sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT phrase, replacement FROM hotwords WHERE enabled = 1 ORDER BY phrase COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())
}

async fn meeting_folder(pool: &SqlitePool, meeting_id: &str) -> Result<(String, String), String> {
    sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT title, folder_path FROM meetings WHERE id = ?",
    )
    .bind(meeting_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "会议文件不存在".to_string())
    .and_then(|(title, path)| {
        path.map(|path| (title, path))
            .ok_or_else(|| "本次会议没有可用的本地录音目录".to_string())
    })
}

#[tauri::command]
pub async fn api_search_meetings(
    state: tauri::State<'_, AppState>,
    query: String,
    speaker: String,
    date_from: String,
    date_to: String,
    limit: i64,
    offset: i64,
) -> Result<PaginatedMeetingSearchResponse, String> {
    crate::access_control::require_admin_authenticated()?;
    let pool = state.db_manager.pool();
    let query = query.trim().to_lowercase();
    let speaker = speaker.trim().to_lowercase();
    let date_from = date_from.trim().to_string();
    let date_to = date_to.trim().to_string();
    let safe_limit = limit.clamp(1, 100);
    let safe_offset = offset.max(0);

    let mut count_builder = QueryBuilder::<Sqlite>::new(
        "SELECT COUNT(DISTINCT m.id) AS total FROM meetings m LEFT JOIN transcripts t ON t.meeting_id = m.id LEFT JOIN meeting_speaker_labels l ON l.meeting_id = m.id AND l.speaker_key = t.speaker WHERE ",
    );
    append_meeting_filters(&mut count_builder, &query, &speaker, &date_from, &date_to);
    let total: i64 = count_builder
        .build()
        .fetch_one(pool)
        .await
        .map_err(|error| error.to_string())?
        .try_get("total")
        .map_err(|error| error.to_string())?;

    let mut rows_builder = QueryBuilder::<Sqlite>::new(
        "SELECT m.id, m.title, m.created_at, m.updated_at, m.folder_path, COUNT(t.id) AS transcript_count, MAX(COALESCE(t.audio_end_time, CASE WHEN t.audio_start_time IS NOT NULL AND t.duration IS NOT NULL THEN t.audio_start_time + t.duration ELSE NULL END)) AS duration_seconds FROM meetings m LEFT JOIN transcripts t ON t.meeting_id = m.id LEFT JOIN meeting_speaker_labels l ON l.meeting_id = m.id AND l.speaker_key = t.speaker WHERE ",
    );
    append_meeting_filters(&mut rows_builder, &query, &speaker, &date_from, &date_to);
    rows_builder
        .push(" GROUP BY m.id ORDER BY m.created_at DESC LIMIT ")
        .push_bind(safe_limit)
        .push(" OFFSET ")
        .push_bind(safe_offset);

    let rows = rows_builder
        .build()
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
    let meetings = rows
        .into_iter()
        .map(|row| {
            let folder_path = row
                .try_get::<Option<String>, _>("folder_path")
                .unwrap_or(None);
            let transcript_duration = row
                .try_get::<Option<f64>, _>("duration_seconds")
                .unwrap_or(None);
            MeetingSearchResult {
                id: row.try_get("id").unwrap_or_default(),
                title: row.try_get("title").unwrap_or_default(),
                created_at: row.try_get("created_at").unwrap_or_default(),
                updated_at: row.try_get("updated_at").unwrap_or_default(),
                folder_path: folder_path.clone(),
                transcript_count: row.try_get("transcript_count").unwrap_or_default(),
                duration_seconds: metadata_duration_seconds(folder_path.as_deref())
                    .or(transcript_duration),
            }
        })
        .collect::<Vec<_>>();
    Ok(PaginatedMeetingSearchResponse {
        has_more: safe_offset + (meetings.len() as i64) < total,
        meetings,
        total_count: total,
    })
}

fn append_meeting_filters(
    builder: &mut QueryBuilder<'_, Sqlite>,
    query: &str,
    speaker: &str,
    date_from: &str,
    date_to: &str,
) {
    let query = query.to_owned();
    let speaker = speaker.to_owned();
    let date_from = date_from.to_owned();
    let date_to = date_to.to_owned();
    builder
        .push("(")
        .push_bind(query.clone())
        .push(" = '' OR lower(m.title) LIKE '%' || ")
        .push_bind(query.clone())
        .push(" || '%' OR lower(COALESCE(t.transcript, '')) LIKE '%' || ")
        .push_bind(query.clone())
        .push(" || '%' OR lower(COALESCE(t.speaker, '')) LIKE '%' || ")
        .push_bind(query.clone())
        .push(" || '%' OR lower(COALESCE(l.display_name, '')) LIKE '%' || ")
        .push_bind(query)
        .push(" || '%') AND (")
        .push_bind(speaker.clone())
        .push(" = '' OR lower(COALESCE(t.speaker, '')) LIKE '%' || ")
        .push_bind(speaker.clone())
        .push(" || '%' OR lower(COALESCE(t.transcript, '')) LIKE '%' || ")
        .push_bind(speaker.clone())
        .push(" || '%' OR lower(COALESCE(l.display_name, '')) LIKE '%' || ")
        .push_bind(speaker)
        .push(" || '%') AND (")
        .push_bind(date_from.clone())
        .push(" = '' OR substr(m.created_at, 1, 10) >= ")
        .push_bind(date_from)
        .push(") AND (")
        .push_bind(date_to.clone())
        .push(" = '' OR substr(m.created_at, 1, 10) <= ")
        .push_bind(date_to)
        .push(")");
}

#[tauri::command]
pub async fn api_list_speaker_labels(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<SpeakerLabel>, String> {
    ensure_meeting_id(&meeting_id)?;
    crate::access_control::require_meeting_unlocked(&meeting_id)?;
    let saved = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "SELECT meeting_id, speaker_key, display_name, contact_id FROM meeting_speaker_labels WHERE meeting_id = ? ORDER BY speaker_key",
    )
    .bind(&meeting_id)
    .fetch_all(state.db_manager.pool())
    .await
    .map_err(|error| error.to_string())?;
    let mut labels = saved
        .into_iter()
        .map(
            |(meeting_id, speaker_key, display_name, contact_id)| SpeakerLabel {
                meeting_id,
                speaker_key,
                display_name,
                contact_id,
            },
        )
        .collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT transcript, speaker FROM transcripts WHERE meeting_id = ? ORDER BY COALESCE(audio_start_time, 0), timestamp",
    )
    .bind(&meeting_id)
    .fetch_all(state.db_manager.pool())
    .await
    .map_err(|error| error.to_string())?;
    for (text, speaker) in rows {
        let speaker_key = speaker.or_else(|| speaker_key_from_text(&text));
        if let Some(speaker_key) = speaker_key {
            if !labels.iter().any(|label| label.speaker_key == speaker_key) {
                labels.push(SpeakerLabel {
                    meeting_id: meeting_id.clone(),
                    display_name: speaker_key.clone(),
                    speaker_key,
                    contact_id: None,
                });
            }
        }
    }
    labels.sort_by(|left, right| left.speaker_key.cmp(&right.speaker_key));
    Ok(labels)
}

fn speaker_key_from_text(text: &str) -> Option<String> {
    let separator = text.find(|character| character == ':' || character == '：')?;
    let candidate = text[..separator].trim();
    if (candidate.starts_with("候选发言人") || candidate.starts_with("发言人"))
        && candidate.chars().count() <= 24
    {
        Some(candidate.to_string())
    } else {
        None
    }
}

#[tauri::command]
pub async fn api_save_speaker_label(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    speaker_key: String,
    display_name: String,
    contact_id: Option<String>,
) -> Result<SpeakerLabel, String> {
    ensure_meeting_id(&meeting_id)?;
    crate::access_control::require_meeting_unlocked(&meeting_id)?;
    let speaker_key = non_empty(speaker_key, "说话人")?;
    let display_name = non_empty(display_name, "显示名称")?;
    let timestamp = now();
    sqlx::query(
        "INSERT INTO meeting_speaker_labels (meeting_id, speaker_key, display_name, contact_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(meeting_id, speaker_key) DO UPDATE SET display_name = excluded.display_name, contact_id = excluded.contact_id, updated_at = excluded.updated_at",
    )
    .bind(&meeting_id)
    .bind(&speaker_key)
    .bind(&display_name)
    .bind(&contact_id)
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(state.db_manager.pool())
    .await
    .map_err(|error| error.to_string())?;
    Ok(SpeakerLabel {
        meeting_id,
        speaker_key,
        display_name,
        contact_id,
    })
}

#[tauri::command]
pub async fn api_merge_speaker_labels(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    from_speaker: String,
    to_speaker: String,
) -> Result<(), String> {
    ensure_meeting_id(&meeting_id)?;
    crate::access_control::require_meeting_unlocked(&meeting_id)?;
    let from_speaker = non_empty(from_speaker, "源说话人")?;
    let to_speaker = non_empty(to_speaker, "目标说话人")?;
    if from_speaker == to_speaker {
        return Err("源说话人与目标说话人不能相同".to_string());
    }
    let mut tx = state
        .db_manager
        .pool()
        .begin()
        .await
        .map_err(|e| e.to_string())?;
    sqlx::query("UPDATE transcripts SET speaker = ? WHERE meeting_id = ? AND speaker = ?")
        .bind(&to_speaker)
        .bind(&meeting_id)
        .bind(&from_speaker)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    let prefixed_rows = sqlx::query_as::<_, (String, String)>(
        "SELECT id, transcript FROM transcripts WHERE meeting_id = ? AND speaker IS NULL",
    )
    .bind(&meeting_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    for (id, transcript) in prefixed_rows {
        let stripped = [format!("{}:", from_speaker), format!("{}：", from_speaker)]
            .iter()
            .find_map(|prefix| transcript.strip_prefix(prefix))
            .map(str::trim_start);
        if let Some(text) = stripped {
            sqlx::query("UPDATE transcripts SET transcript = ?, speaker = ? WHERE id = ?")
                .bind(text)
                .bind(&to_speaker)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    sqlx::query("DELETE FROM meeting_speaker_labels WHERE meeting_id = ? AND speaker_key = ?")
        .bind(&meeting_id)
        .bind(&from_speaker)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn api_list_speaker_contacts(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<SpeakerContact>, String> {
    crate::access_control::require_admin_authenticated()?;
    sqlx::query_as::<_, (String, String, Option<String>, String, String)>(
        "SELECT id, name, note, created_at, updated_at FROM speaker_contacts ORDER BY name COLLATE NOCASE",
    )
    .fetch_all(state.db_manager.pool())
    .await
    .map(|rows| rows.into_iter().map(|(id, name, note, created_at, updated_at)| SpeakerContact { id, name, note, created_at, updated_at }).collect())
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn api_save_speaker_contact(
    state: tauri::State<'_, AppState>,
    id: Option<String>,
    name: String,
    note: Option<String>,
) -> Result<SpeakerContact, String> {
    crate::access_control::require_admin_authenticated()?;
    let name = non_empty(name, "联系人名称")?;
    let id = id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("contact-{}", Uuid::new_v4()));
    let timestamp = now();
    sqlx::query("INSERT INTO speaker_contacts (id, name, note, created_at, updated_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET name = excluded.name, note = excluded.note, updated_at = excluded.updated_at")
        .bind(&id).bind(&name).bind(&note).bind(&timestamp).bind(&timestamp)
        .execute(state.db_manager.pool()).await.map_err(|e| e.to_string())?;
    Ok(SpeakerContact {
        id,
        name,
        note,
        created_at: timestamp.clone(),
        updated_at: timestamp,
    })
}

#[tauri::command]
pub async fn api_list_hotwords(state: tauri::State<'_, AppState>) -> Result<Vec<Hotword>, String> {
    crate::access_control::require_admin_authenticated()?;
    sqlx::query_as::<_, (String, String, Option<String>, i64)>(
        "SELECT id, phrase, replacement, enabled FROM hotwords ORDER BY phrase COLLATE NOCASE",
    )
    .fetch_all(state.db_manager.pool())
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|(id, phrase, replacement, enabled)| Hotword {
                id,
                phrase,
                replacement,
                enabled: enabled != 0,
            })
            .collect()
    })
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn api_save_hotword(
    state: tauri::State<'_, AppState>,
    id: Option<String>,
    phrase: String,
    replacement: Option<String>,
    enabled: bool,
) -> Result<Hotword, String> {
    crate::access_control::require_admin_authenticated()?;
    let phrase = non_empty(phrase, "热词")?;
    let id = id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("hotword-{}", Uuid::new_v4()));
    let timestamp = now();
    sqlx::query("INSERT INTO hotwords (id, phrase, replacement, enabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET phrase = excluded.phrase, replacement = excluded.replacement, enabled = excluded.enabled, updated_at = excluded.updated_at")
        .bind(&id).bind(&phrase).bind(&replacement).bind(if enabled { 1 } else { 0 }).bind(&timestamp).bind(&timestamp)
        .execute(state.db_manager.pool()).await.map_err(|e| e.to_string())?;
    Ok(Hotword {
        id,
        phrase,
        replacement,
        enabled,
    })
}

#[tauri::command]
pub async fn api_delete_hotword(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    crate::access_control::require_admin_authenticated()?;
    sqlx::query("DELETE FROM hotwords WHERE id = ?")
        .bind(id)
        .execute(state.db_manager.pool())
        .await
        .map_err(|e| e.to_string())
        .map(|_| ())
}

#[tauri::command]
pub async fn api_list_meeting_actions(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<MeetingAction>, String> {
    ensure_meeting_id(&meeting_id)?;
    crate::access_control::require_meeting_unlocked(&meeting_id)?;
    list_meeting_actions(state.db_manager.pool(), &meeting_id).await
}

async fn list_meeting_actions(
    pool: &SqlitePool,
    meeting_id: &str,
) -> Result<Vec<MeetingAction>, String> {
    let rows = sqlx::query_as::<_, (String, String, String, Option<String>, Option<String>, String, Option<String>, String, String)>(
        "SELECT id, meeting_id, content, assignee, due_date, status, source_transcript_id, created_at, updated_at FROM meeting_actions WHERE meeting_id = ? ORDER BY created_at",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                meeting_id,
                content,
                assignee,
                due_date,
                status,
                source_transcript_id,
                created_at,
                updated_at,
            )| MeetingAction {
                id,
                meeting_id,
                content,
                assignee,
                due_date,
                status,
                source_transcript_id,
                created_at,
                updated_at,
            },
        )
        .collect())
}

#[tauri::command]
pub async fn api_extract_meeting_actions(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<MeetingAction>, String> {
    ensure_meeting_id(&meeting_id)?;
    crate::access_control::require_meeting_unlocked(&meeting_id)?;
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT id, transcript FROM transcripts WHERE meeting_id = ? ORDER BY COALESCE(audio_start_time, 0), timestamp",
    )
    .bind(&meeting_id)
    .fetch_all(state.db_manager.pool())
    .await
    .map_err(|e| e.to_string())?;
    let mut extracted = Vec::new();
    for (source_id, text) in rows {
        if !is_action_candidate(&text)
            || extracted
                .iter()
                .any(|item: &MeetingAction| item.content == text.trim())
        {
            continue;
        }
        let id = format!("action-{}", Uuid::new_v4());
        let timestamp = now();
        sqlx::query("INSERT OR IGNORE INTO meeting_actions (id, meeting_id, content, status, source_transcript_id, created_at, updated_at) VALUES (?, ?, ?, 'open', ?, ?, ?)")
            .bind(&id)
            .bind(&meeting_id)
            .bind(text.trim())
            .bind(&source_id)
            .bind(&timestamp)
            .bind(&timestamp)
            .execute(state.db_manager.pool())
            .await
            .map_err(|e| e.to_string())?;
        extracted.push(MeetingAction {
            id,
            meeting_id: meeting_id.clone(),
            content: text.trim().to_string(),
            assignee: None,
            due_date: None,
            status: "open".to_string(),
            source_transcript_id: Some(source_id),
            created_at: timestamp.clone(),
            updated_at: timestamp,
        });
        if extracted.len() >= 50 {
            break;
        }
    }
    list_meeting_actions(state.db_manager.pool(), &meeting_id).await
}

fn is_action_candidate(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && [
            "待办",
            "需要",
            "负责",
            "跟进",
            "下一步",
            "截止",
            "请",
            "todo",
            "action item",
        ]
        .iter()
        .any(|keyword| text.to_lowercase().contains(keyword))
}

#[tauri::command]
pub async fn api_update_meeting_action(
    state: tauri::State<'_, AppState>,
    id: String,
    content: String,
    assignee: Option<String>,
    due_date: Option<String>,
    status: String,
) -> Result<(), String> {
    let meeting_id: String =
        sqlx::query_scalar("SELECT meeting_id FROM meeting_actions WHERE id = ?")
            .bind(&id)
            .fetch_optional(state.db_manager.pool())
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "行动项不存在".to_string())?;
    crate::access_control::require_meeting_unlocked(&meeting_id)?;
    let content = non_empty(content, "行动项")?;
    sqlx::query("UPDATE meeting_actions SET content = ?, assignee = ?, due_date = ?, status = ?, updated_at = ? WHERE id = ?")
        .bind(content)
        .bind(assignee)
        .bind(due_date)
        .bind(status)
        .bind(now())
        .bind(id)
        .execute(state.db_manager.pool())
        .await
        .map_err(|e| e.to_string())
        .map(|_| ())
}

#[tauri::command]
pub async fn api_update_transcript(
    state: tauri::State<'_, AppState>,
    transcript_id: String,
    text: String,
    speaker: Option<String>,
) -> Result<(), String> {
    let transcript_id = non_empty(transcript_id, "转写段落")?;
    let text = non_empty(text, "转写内容")?;
    let meeting_id: String = sqlx::query_scalar("SELECT meeting_id FROM transcripts WHERE id = ?")
        .bind(&transcript_id)
        .fetch_optional(state.db_manager.pool())
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "转写段落不存在".to_string())?;
    crate::access_control::require_meeting_unlocked(&meeting_id)?;
    let speaker = speaker.and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    });
    let mut tx = state
        .db_manager
        .pool()
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    sqlx::query("UPDATE transcripts SET transcript = ?, speaker = ? WHERE id = ?")
        .bind(text)
        .bind(speaker)
        .bind(transcript_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| error.to_string())?;
    sqlx::query("UPDATE meetings SET updated_at = ? WHERE id = ?")
        .bind(now())
        .bind(meeting_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| error.to_string())?;
    tx.commit().await.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn api_export_meeting_format(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    format: String,
) -> Result<String, String> {
    ensure_meeting_id(&meeting_id)?;
    crate::access_control::require_meeting_unlocked(&meeting_id)?;
    let format = format.trim().to_lowercase();
    if !["md", "txt", "srt", "docx", "pdf"].contains(&format.as_str()) {
        return Err("不支持的导出格式".to_string());
    }
    let pool = state.db_manager.pool();
    let (title, folder_path) = meeting_folder(pool, &meeting_id).await?;
    let transcripts = sqlx::query_as::<_, (String, String, Option<f64>, Option<f64>, Option<String>)>("SELECT timestamp, transcript, audio_start_time, audio_end_time, speaker FROM transcripts WHERE meeting_id = ? ORDER BY COALESCE(audio_start_time, 0), timestamp")
        .bind(&meeting_id)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;
    let summary = sqlx::query_scalar::<_, Option<String>>(
        "SELECT result FROM summary_processes WHERE meeting_id = ? AND status = 'completed'",
    )
    .bind(&meeting_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?
    .flatten();
    let actions = list_meeting_actions(pool, &meeting_id).await?;
    let markdown = build_markdown(&title, &transcripts, summary.as_deref(), &actions);
    let base = Path::new(&folder_path).join(format!("会议成果.{}", format));
    match format.as_str() {
        "md" => fs::write(&base, markdown.as_bytes()).map_err(|e| e.to_string()),
        "txt" => fs::write(&base, markdown.replace('#', "").as_bytes()).map_err(|e| e.to_string()),
        "srt" => fs::write(&base, build_srt(&transcripts).as_bytes()).map_err(|e| e.to_string()),
        "docx" => write_docx(&base, &markdown),
        "pdf" => fs::write(&base, build_pdf(&markdown)).map_err(|e| e.to_string()),
        _ => unreachable!(),
    }
    .map_err(|e| format!("无法保存导出文件: {}", e))?;
    Ok(base.to_string_lossy().to_string())
}

fn build_markdown(
    title: &str,
    transcripts: &[(String, String, Option<f64>, Option<f64>, Option<String>)],
    summary: Option<&str>,
    actions: &[MeetingAction],
) -> String {
    let mut output = format!("# {}\n\n## 完整转写\n\n", title);
    for (timestamp, text, start, _, speaker) in transcripts {
        let speaker = speaker
            .as_deref()
            .map(|value| format!("{}: ", value))
            .unwrap_or_default();
        let timestamp = start
            .map(format_seconds)
            .unwrap_or_else(|| timestamp.clone());
        output.push_str(&format!("[{}] {}{}\n", timestamp, speaker, text));
    }
    if let Some(summary) = summary.filter(|value| !value.trim().is_empty()) {
        output.push_str("\n## AI 会议总结\n\n");
        output.push_str(&summary_markdown(summary));
        output.push('\n');
    }
    if !actions.is_empty() {
        output.push_str("\n## 行动项\n\n");
        for action in actions {
            let checked = if action.status == "done" { "x" } else { " " };
            output.push_str(&format!("- [{}] {}", checked, action.content));
            if let Some(assignee) = action.assignee.as_deref().filter(|value| !value.is_empty()) {
                output.push_str(&format!(" · 负责人：{}", assignee));
            }
            if let Some(due_date) = action.due_date.as_deref().filter(|value| !value.is_empty()) {
                output.push_str(&format!(" · 截止：{}", due_date));
            }
            output.push('\n');
        }
    }
    output
}

fn summary_markdown(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|value| value.get("markdown")?.as_str().map(str::to_owned))
        .filter(|markdown| !markdown.trim().is_empty())
        .unwrap_or_else(|| raw.to_string())
}

fn build_srt(transcripts: &[(String, String, Option<f64>, Option<f64>, Option<String>)]) -> String {
    transcripts
        .iter()
        .enumerate()
        .map(|(index, (_, text, start, end, speaker))| {
            let start = start.unwrap_or(index as f64 * 3.0);
            let end = end.unwrap_or(start + 3.0);
            let speaker = speaker
                .as_deref()
                .map(|value| format!("{}: ", value))
                .unwrap_or_default();
            format!(
                "{}\n{} --> {}\n{}{}\n",
                index + 1,
                format_srt_time(start),
                format_srt_time(end),
                speaker,
                text
            )
        })
        .collect()
}

fn format_seconds(seconds: f64) -> String {
    format!(
        "{:02}:{:02}",
        (seconds / 60.0).floor() as u64,
        seconds.floor() as u64 % 60
    )
}

fn format_srt_time(seconds: f64) -> String {
    let total_millis = (seconds.max(0.0) * 1000.0).round() as u64;
    let total_seconds = total_millis / 1000;
    format!(
        "{:02}:{:02}:{:02},{:03}",
        total_seconds / 3600,
        total_seconds / 60 % 60,
        total_seconds % 60,
        total_millis % 1000
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn write_docx(path: &Path, markdown: &str) -> Result<(), String> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    let file = fs::File::create(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    zip.start_file("[Content_Types].xml", options)
        .map_err(|e| e.to_string())?;
    zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#).map_err(|e| e.to_string())?;
    zip.start_file("_rels/.rels", options)
        .map_err(|e| e.to_string())?;
    zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#).map_err(|e| e.to_string())?;
    zip.start_file("word/document.xml", options)
        .map_err(|e| e.to_string())?;
    let body = markdown
        .lines()
        .map(|line| {
            format!(
                "<w:p><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
                xml_escape(line)
            )
        })
        .collect::<String>();
    zip.write_all(format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{}<w:sectPr/></w:body></w:document>"#, body).as_bytes()).map_err(|e| e.to_string())?;
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

fn build_pdf(markdown: &str) -> Vec<u8> {
    let lines = markdown
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(52)
        .collect::<Vec<_>>();
    let content = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let encoded = line
                .encode_utf16()
                .map(|unit| format!("{:04X}", unit))
                .collect::<String>();
            format!(
                "BT /F1 10 Tf 40 {} Td <{}> Tj ET\n",
                790 - index as i32 * 14,
                encoded
            )
        })
        .collect::<String>();
    let objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_string(),
        format!("<< /Length {} >>\nstream\n{}endstream", content.as_bytes().len(), content),
        "<< /Type /Font /Subtype /Type0 /BaseFont /STSong-Light /Encoding /UniGB-UCS2-H /DescendantFonts [6 0 R] >>".to_string(),
        "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /STSong-Light /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 2 >> /FontDescriptor 7 0 R /DW 1000 >>".to_string(),
        "<< /Type /FontDescriptor /FontName /STSong-Light /Flags 6 /FontBBox [-25 -254 1000 880] /ItalicAngle 0 /Ascent 880 /Descent -120 /CapHeight 880 /StemV 80 >>".to_string(),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{:010} 00000 n \n", offset).as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            objects.len() + 1,
            xref
        )
        .as_bytes(),
    );
    pdf
}

#[cfg(test)]
mod tests {
    use super::{build_markdown, build_pdf, build_srt, format_srt_time, is_action_candidate};

    fn transcripts() -> Vec<(String, String, Option<f64>, Option<f64>, Option<String>)> {
        vec![(
            "10:30:00".to_string(),
            "请小王明天跟进客户".to_string(),
            Some(61.25),
            Some(64.5),
            Some("发言人 1".to_string()),
        )]
    }

    #[test]
    fn action_candidate_requires_an_action_phrase() {
        assert!(is_action_candidate("下一步需要跟进报价"));
        assert!(!is_action_candidate("今天讨论了报价"));
    }

    #[test]
    fn srt_keeps_timing_and_speaker() {
        let output = build_srt(&transcripts());
        assert!(output.contains("00:01:01,250 --> 00:01:04,500"));
        assert!(output.contains("发言人 1: 请小王明天跟进客户"));
        assert_eq!(format_srt_time(3661.25), "01:01:01,250");
    }

    #[test]
    fn markdown_and_pdf_keep_chinese_content() {
        let markdown = build_markdown("项目会议", &transcripts(), Some("会议总结"), &[]);
        assert!(markdown.contains("项目会议"));
        let pdf = build_pdf(&markdown);
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(String::from_utf8_lossy(&pdf).contains("987976EE4F1A8BAE"));
    }
}
