use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use chrono::Utc;
use serde::Serialize;
use tauri::State;
use std::fs;
use std::path::Path;

use crate::state::AppState;

const DEFAULT_ADMIN_PASSWORD: &str = "mihui2026";

#[derive(Serialize)]
pub struct AdminStatus {
    initialized: bool,
}

fn hash_password(password: &str) -> Result<String, String> {
    if password.trim().len() < 8 {
        return Err("密码至少需要 8 个字符".to_string());
    }
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| "无法安全保存密码".to_string())
}

#[cfg(test)]
mod tests {
    use super::{hash_password, summary_markdown, verify_password};

    #[test]
    fn password_hash_is_salted_and_verifiable() {
        let first = hash_password("secure-pass").expect("password should hash");
        let second = hash_password("secure-pass").expect("password should hash");

        assert_ne!(first, second);
        assert!(verify_password("secure-pass", &first));
        assert!(!verify_password("wrong-pass", &first));
        assert!(!first.contains("secure-pass"));
    }

    #[test]
    fn password_rejects_short_or_whitespace_only_values() {
        assert!(hash_password("short").is_err());
        assert!(hash_password("        ").is_err());
    }

    #[test]
    fn summary_export_uses_rendered_markdown_instead_of_json() {
        let raw = r###"{"markdown":"## 决策\n保持本地处理","english_cache":{"markdown":"ignored"}}"###;
        assert_eq!(summary_markdown(raw), "## 决策\n保持本地处理");
        assert_eq!(summary_markdown("已有纯文本总结"), "已有纯文本总结");
    }
}

fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .ok()
        .and_then(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).ok())
        .is_some()
}

fn summary_markdown(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|value| value.get("markdown")?.as_str().map(str::to_owned))
        .filter(|markdown| !markdown.trim().is_empty())
        .unwrap_or_else(|| raw.to_string())
}

async fn ensure_admin_row(state: &State<'_, AppState>) -> Result<(String, bool), String> {
    let pool = state.db_manager.pool();
    if let Some((hash, initialized)) = sqlx::query_as::<_, (String, bool)>(
        "SELECT password_hash, initialized FROM local_admin_credentials WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?
    {
        return Ok((hash, initialized));
    }

    let hash = hash_password(DEFAULT_ADMIN_PASSWORD)?;
    sqlx::query(
        "INSERT INTO local_admin_credentials (id, password_hash, initialized, updated_at) VALUES (1, ?, 0, ?)",
    )
    .bind(&hash)
    .bind(Utc::now().to_rfc3339())
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok((hash, false))
}

#[tauri::command]
pub async fn get_admin_status(state: State<'_, AppState>) -> Result<AdminStatus, String> {
    let (_, initialized) = ensure_admin_row(&state).await?;
    Ok(AdminStatus { initialized })
}

#[tauri::command]
pub async fn initialize_admin_password(
    state: State<'_, AppState>,
    initial_password: String,
    new_password: String,
) -> Result<(), String> {
    let (hash, initialized) = ensure_admin_row(&state).await?;
    if initialized {
        return Err("管理员已经完成初始化".to_string());
    }
    if !verify_password(&initial_password, &hash) {
        return Err("初始密码不正确".to_string());
    }
    let new_hash = hash_password(&new_password)?;
    sqlx::query("UPDATE local_admin_credentials SET password_hash = ?, initialized = 1, updated_at = ? WHERE id = 1")
        .bind(new_hash)
        .bind(Utc::now().to_rfc3339())
        .execute(state.db_manager.pool())
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn verify_admin_password(state: State<'_, AppState>, password: String) -> Result<bool, String> {
    let (hash, _) = ensure_admin_row(&state).await?;
    Ok(verify_password(&password, &hash))
}

#[tauri::command]
pub async fn change_admin_password(
    state: State<'_, AppState>,
    current_password: String,
    new_password: String,
) -> Result<(), String> {
    let (hash, _) = ensure_admin_row(&state).await?;
    if !verify_password(&current_password, &hash) {
        return Err("当前管理员密码不正确".to_string());
    }
    let new_hash = hash_password(&new_password)?;
    sqlx::query("UPDATE local_admin_credentials SET password_hash = ?, initialized = 1, updated_at = ? WHERE id = 1")
        .bind(new_hash)
        .bind(Utc::now().to_rfc3339())
        .execute(state.db_manager.pool())
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn set_meeting_password(
    state: State<'_, AppState>,
    meeting_id: String,
    password: Option<String>,
) -> Result<(), String> {
    let password = password.filter(|value| !value.trim().is_empty());
    let password_hash = password.as_deref().map(hash_password).transpose()?;
    let uses_admin = password_hash.is_none();
    sqlx::query(
        "INSERT INTO meeting_access_credentials (meeting_id, password_hash, uses_admin_password, updated_at) VALUES (?, ?, ?, ?) ON CONFLICT(meeting_id) DO UPDATE SET password_hash = excluded.password_hash, uses_admin_password = excluded.uses_admin_password, updated_at = excluded.updated_at",
    )
    .bind(meeting_id)
    .bind(password_hash)
    .bind(uses_admin)
    .bind(Utc::now().to_rfc3339())
    .execute(state.db_manager.pool())
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn verify_meeting_password(
    state: State<'_, AppState>,
    meeting_id: String,
    password: String,
) -> Result<bool, String> {
    let credential = sqlx::query_as::<_, (Option<String>, bool)>(
        "SELECT password_hash, uses_admin_password FROM meeting_access_credentials WHERE meeting_id = ?",
    )
    .bind(&meeting_id)
    .fetch_optional(state.db_manager.pool())
    .await
    .map_err(|error| error.to_string())?;
    match credential {
        Some((Some(hash), false)) => Ok(verify_password(&password, &hash)),
        _ => verify_admin_password(state, password).await,
    }
}

#[tauri::command]
pub async fn reset_meeting_password(
    state: State<'_, AppState>,
    meeting_id: String,
    admin_password: String,
    new_password: String,
) -> Result<(), String> {
    let (admin_hash, _) = ensure_admin_row(&state).await?;
    if !verify_password(&admin_password, &admin_hash) {
        return Err("管理员密码不正确".to_string());
    }
    set_meeting_password(state, meeting_id, Some(new_password)).await
}

#[tauri::command]
pub async fn export_meeting_text(state: State<'_, AppState>, meeting_id: String) -> Result<String, String> {
    let pool = state.db_manager.pool();
    let meeting = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT title, folder_path FROM meetings WHERE id = ?",
    )
    .bind(&meeting_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "会议文件不存在".to_string())?;
    let folder_path = meeting.1.ok_or_else(|| "本次会议没有可用的本地录音目录".to_string())?;
    let transcripts = sqlx::query_as::<_, (String, String)>(
        "SELECT timestamp, transcript FROM transcripts WHERE meeting_id = ? ORDER BY COALESCE(audio_start_time, 0), timestamp",
    )
    .bind(&meeting_id)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    let summary = sqlx::query_scalar::<_, Option<String>>(
        "SELECT result FROM summary_processes WHERE meeting_id = ? AND status = 'completed'",
    )
    .bind(&meeting_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?
    .flatten();

    let mut content = format!("# {}\n\n## 完整转写\n\n", meeting.0);
    for (timestamp, transcript) in transcripts {
        content.push_str(&format!("[{}] {}\n", timestamp, transcript));
    }
    if let Some(summary) = summary {
        content.push_str("\n## AI 会议总结\n\n");
        content.push_str(&summary_markdown(&summary));
        content.push('\n');
    }
    let output_path = Path::new(&folder_path).join("会议文字成果.md");
    fs::write(&output_path, content).map_err(|error| format!("无法保存文字成果: {}", error))?;
    Ok(output_path.to_string_lossy().to_string())
}
