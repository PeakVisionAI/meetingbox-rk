use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerAction {
    Reboot,
    Poweroff,
}

impl PowerAction {
    fn systemctl_verb(&self) -> &'static str {
        match self {
            Self::Reboot => "reboot",
            Self::Poweroff => "poweroff",
        }
    }
}

#[tauri::command]
pub async fn system_power_action(action: PowerAction) -> Result<(), String> {
    if crate::audio::recording_commands::is_recording().await {
        return Err("会议录制中，请先结束并保存会议".to_string());
    }

    #[cfg(target_os = "linux")]
    {
        let verb = action.systemctl_verb();
        let output = tokio::process::Command::new("systemctl")
            .args(["--no-block", verb])
            .output()
            .await
            .map_err(|error| format!("无法执行系统电源操作: {error}"))?;

        if output.status.success() {
            Ok(())
        } else {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if detail.is_empty() {
                Err(format!("系统电源操作失败，退出状态: {}", output.status))
            } else {
                Err(format!("系统电源操作失败：{detail}"))
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = action;
        Err("当前系统不支持此电源操作".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::PowerAction;

    #[test]
    fn maps_power_actions_to_fixed_systemctl_verbs() {
        assert_eq!(PowerAction::Reboot.systemctl_verb(), "reboot");
        assert_eq!(PowerAction::Poweroff.systemctl_verb(), "poweroff");
    }
}

#[tauri::command]
pub async fn close_application(app: tauri::AppHandle) -> Result<(), String> {
    if crate::audio::recording_commands::is_recording().await {
        return Err("会议录制中，请先结束并保存会议".to_string());
    }
    app.exit(0);
    Ok(())
}
