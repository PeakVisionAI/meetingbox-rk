use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use chrono::Utc;
use once_cell::sync::Lazy;
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use tauri::State;

use crate::state::AppState;

static ADMIN_AUTHENTICATED: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));
static UNLOCKED_MEETINGS: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));

#[derive(Serialize)]
pub struct AdminStatus {
    initialized: bool,
}

fn hash_admin_password(password: &str) -> Result<String, String> {
    if password.len() != 6 || !password.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("管理员密码必须是 6 位数字".to_string());
    }
    hash_password(password)
}

fn hash_meeting_password(password: &str) -> Result<String, String> {
    if password.len() != 6 || !password.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("会议密码必须是 6 位数字".to_string());
    }
    hash_password(password)
}

fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| "无法安全保存密码".to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        hash_admin_password, hash_meeting_password, require_meeting_unlocked, summary_markdown,
        verify_password, transcript_paragraphs, UNLOCKED_MEETINGS,
    };

    #[test]
    fn password_hash_is_salted_and_verifiable() {
        let first = hash_admin_password("123456").expect("password should hash");
        let second = hash_admin_password("123456").expect("password should hash");

        assert_ne!(first, second);
        assert!(verify_password("123456", &first));
        assert!(!verify_password("654321", &first));
        assert!(!first.contains("123456"));
    }

    #[test]
    fn admin_password_requires_exactly_six_ascii_digits() {
        assert!(hash_admin_password("123456").is_ok());
        assert!(hash_admin_password("12345").is_err());
        assert!(hash_admin_password("1234567").is_err());
        assert!(hash_admin_password("12345a").is_err());
    }

    #[test]
    fn meeting_password_requires_exactly_six_ascii_digits() {
        assert!(hash_meeting_password("123456").is_ok());
        assert!(hash_meeting_password("12345").is_err());
        assert!(hash_meeting_password("1234567").is_err());
        assert!(hash_meeting_password("12345a").is_err());
        assert!(hash_meeting_password("１２３４５６").is_err());
    }

    #[test]
    fn uninitialized_empty_hash_never_verifies() {
        assert!(!verify_password("any-password", ""));
    }

    #[test]
    fn meeting_content_requires_an_unlocked_session() {
        let meeting_id = "access-control-test-meeting";
        UNLOCKED_MEETINGS.lock().unwrap().remove(meeting_id);
        assert!(require_meeting_unlocked(meeting_id).is_err());
        UNLOCKED_MEETINGS
            .lock()
            .unwrap()
            .insert(meeting_id.to_string());
        assert!(require_meeting_unlocked(meeting_id).is_ok());
        UNLOCKED_MEETINGS.lock().unwrap().remove(meeting_id);
    }


    #[test]
    fn transcript_export_merges_chunks_and_preserves_punctuation_and_speakers() {
        let rows = vec![("00:00".into(), "发言人1：今天讨论，".into()), ("00:07".into(), "发言人1：会议方案。".into()), ("00:08".into(), "发言人2：好的！".into())];
        assert_eq!(transcript_paragraphs(&rows), "发言人1：\n今天讨论，会议方案。\n\n发言人2：\n好的！");
    }

    #[test]
    fn summary_export_uses_rendered_markdown_instead_of_json() {
        let raw =
            r###"{"markdown":"## 决策\n保持本地处理","english_cache":{"markdown":"ignored"}}"###;
        assert_eq!(summary_markdown(raw), "## 决策\n保持本地处理");
        assert_eq!(summary_markdown("已有纯文本总结"), "已有纯文本总结");
    }
}

fn verify_password(password: &str, hash: &str) -> bool {
    if hash.trim().is_empty() {
        return false;
    }
    PasswordHash::new(hash)
        .ok()
        .and_then(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .ok()
        })
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

    // Keep the uninitialized row unusable until the operator chooses a password.
    // No bootstrap password is stored in the binary or repository.
    sqlx::query(
        "INSERT INTO local_admin_credentials (id, password_hash, initialized, updated_at) VALUES (1, ?, 0, ?)",
    )
    .bind("")
    .bind(Utc::now().to_rfc3339())
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok((String::new(), false))
}

#[tauri::command]
pub async fn get_admin_status(state: State<'_, AppState>) -> Result<AdminStatus, String> {
    let (_, initialized) = ensure_admin_row(&state).await?;
    Ok(AdminStatus { initialized })
}

#[tauri::command]
pub async fn initialize_admin_password(
    state: State<'_, AppState>,
    new_password: String,
) -> Result<(), String> {
    let (_, initialized) = ensure_admin_row(&state).await?;
    if initialized {
        return Err("管理员已经完成初始化".to_string());
    }
    let new_hash = hash_admin_password(&new_password)?;
    sqlx::query("UPDATE local_admin_credentials SET password_hash = ?, initialized = 1, updated_at = ? WHERE id = 1")
        .bind(new_hash)
        .bind(Utc::now().to_rfc3339())
        .execute(state.db_manager.pool())
        .await
        .map_err(|error| error.to_string())?;
    *ADMIN_AUTHENTICATED
        .lock()
        .map_err(|_| "安全状态不可用".to_string())? = true;
    Ok(())
}

#[tauri::command]
pub async fn verify_admin_password(
    state: State<'_, AppState>,
    password: String,
) -> Result<bool, String> {
    let (hash, initialized) = ensure_admin_row(&state).await?;
    if !initialized {
        return Ok(false);
    }
    let valid = verify_password(&password, &hash);
    if valid {
        *ADMIN_AUTHENTICATED
            .lock()
            .map_err(|_| "安全状态不可用".to_string())? = true;
    }
    Ok(valid)
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
    let new_hash = hash_admin_password(&new_password)?;
    sqlx::query("UPDATE local_admin_credentials SET password_hash = ?, initialized = 1, updated_at = ? WHERE id = 1")
        .bind(new_hash)
        .bind(Utc::now().to_rfc3339())
        .execute(state.db_manager.pool())
        .await
        .map_err(|error| error.to_string())?;
    *ADMIN_AUTHENTICATED
        .lock()
        .map_err(|_| "安全状态不可用".to_string())? = true;
    Ok(())
}

fn admin_is_authenticated() -> Result<bool, String> {
    ADMIN_AUTHENTICATED
        .lock()
        .map(|authenticated| *authenticated)
        .map_err(|_| "安全状态不可用".to_string())
}

pub(crate) fn require_admin_authenticated() -> Result<(), String> {
    admin_is_authenticated()?
        .then_some(())
        .ok_or_else(|| "需要管理员登录后才能执行此操作".to_string())
}

pub fn require_meeting_unlocked(meeting_id: &str) -> Result<(), String> {
    UNLOCKED_MEETINGS
        .lock()
        .map_err(|_| "安全状态不可用".to_string())?
        .contains(meeting_id)
        .then_some(())
        .ok_or_else(|| "请先验证会议密码".to_string())
}

pub fn is_meeting_unlocked(meeting_id: &str) -> bool {
    UNLOCKED_MEETINGS
        .lock()
        .map(|meetings| meetings.contains(meeting_id))
        .unwrap_or(false)
}

#[tauri::command]
pub async fn meeting_uses_independent_password(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<bool, String> {
    sqlx::query_scalar::<_, bool>(
        "SELECT CASE WHEN password_hash IS NOT NULL AND uses_admin_password = 0 THEN 1 ELSE 0 END FROM meeting_access_credentials WHERE meeting_id = ?",
    )
        .bind(meeting_id)
        .fetch_optional(state.db_manager.pool())
        .await
        .map_err(|_| "安全状态不可用".to_string())
        .map(|value| value.unwrap_or(false))
}

#[tauri::command]
pub async fn set_meeting_password(
    state: State<'_, AppState>,
    meeting_id: String,
    password: Option<String>,
) -> Result<(), String> {
    if !admin_is_authenticated()? {
        return Err("需要管理员登录后才能设置会议密码".to_string());
    }
    let password = password.filter(|value| !value.trim().is_empty());
    let password_hash = password.as_deref().map(hash_meeting_password).transpose()?;
    let uses_admin = password_hash.is_none();
    sqlx::query(
        "INSERT INTO meeting_access_credentials (meeting_id, password_hash, uses_admin_password, updated_at) VALUES (?, ?, ?, ?) ON CONFLICT(meeting_id) DO UPDATE SET password_hash = excluded.password_hash, uses_admin_password = excluded.uses_admin_password, updated_at = excluded.updated_at",
    )
    .bind(&meeting_id)
    .bind(password_hash)
    .bind(uses_admin)
    .bind(Utc::now().to_rfc3339())
    .execute(state.db_manager.pool())
    .await
    .map_err(|error| error.to_string())?;
    UNLOCKED_MEETINGS
        .lock()
        .map_err(|_| "安全状态不可用".to_string())?
        .insert(meeting_id);
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
    let valid = match credential {
        Some((Some(hash), false)) => verify_password(&password, &hash),
        _ => verify_admin_password(state, password).await?,
    };
    if valid {
        UNLOCKED_MEETINGS
            .lock()
            .map_err(|_| "安全状态不可用".to_string())?
            .insert(meeting_id);
    }
    Ok(valid)
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
    *ADMIN_AUTHENTICATED
        .lock()
        .map_err(|_| "安全状态不可用".to_string())? = true;
    set_meeting_password(state, meeting_id, Some(new_password)).await
}


fn transcript_paragraphs(rows: &[(String, String)]) -> String {
    let prefix = regex::Regex::new(r"^((?:候选发言人|发言人待识别|发言人(?:\s*[^：:\s]{1,20})?))[：:]\s*([\s\S]*)$").expect("speaker pattern");
    let mut paragraphs: Vec<(String, String)> = Vec::new();
    for (_, raw) in rows {
        let (speaker, text) = match prefix.captures(raw) {
            Some(c) => (c[1].to_string(), c[2].to_string()),
            None => ("发言人待识别".to_string(), raw.to_string()),
        };
        if text.trim().is_empty() { continue; }
        let merge = paragraphs.last().map(|(previous, body)| {
            let complete = body.trim_end_matches(['”', '’', '」', '』', '）', ')']).ends_with(['。', '！', '？', '!', '?']);
            previous == &speaker && !speaker.contains("待识别") && !speaker.contains("候选") && !(body.chars().count() >= 200 && complete)
        }).unwrap_or(false);
        if merge {
            let body = &mut paragraphs.last_mut().unwrap().1;
            if body.chars().last().map(|c| c.is_ascii_alphanumeric()).unwrap_or(false) && text.chars().next().map(|c| c.is_ascii_alphanumeric()).unwrap_or(false) { body.push(' '); }
            body.push_str(&text);
        } else { paragraphs.push((speaker, text)); }
    }
    paragraphs.into_iter().map(|(speaker, text)| format!("{}：\n{}", speaker, text)).collect::<Vec<_>>().join("\n\n")
}

#[tauri::command]
pub async fn export_meeting_text(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<String, String> {
    require_meeting_unlocked(&meeting_id)?;
    let pool = state.db_manager.pool();
    let meeting = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT title, folder_path FROM meetings WHERE id = ?",
    )
    .bind(&meeting_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "会议文件不存在".to_string())?;
    let folder_path = meeting
        .1
        .ok_or_else(|| "本次会议没有可用的本地录音目录".to_string())?;
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
    let transcript_body = transcript_paragraphs(&transcripts);
    content.push_str(&transcript_body);
    content.push_str("\n\n");
    fs::write(Path::new(&folder_path).join("完整转录稿.txt"), &transcript_body).map_err(|error| error.to_string())?;
    if let Some(summary) = summary {
        content.push_str("\n## AI 会议总结\n\n");
        let markdown = summary_markdown(&summary);
        fs::write(Path::new(&folder_path).join("会议纪要.md"), &markdown).map_err(|error| error.to_string())?;
        content.push_str(&markdown);
        content.push('\n');
    }
    let output_path = Path::new(&folder_path).join("会议文字成果.md");
    fs::write(&output_path, content).map_err(|error| format!("无法保存文字成果: {}", error))?;
    Ok(output_path.to_string_lossy().to_string())
}
