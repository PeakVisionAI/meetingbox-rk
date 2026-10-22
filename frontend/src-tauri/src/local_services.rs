//! Local service readiness gate.
//!
//! Which services must be reachable (and whether we are allowed to bootstrap them)
//! is described by the platform profile, so this module holds no IP addresses or
//! install paths of its own.

use serde::Serialize;
use tauri::Emitter;

use crate::platform::{self, ServiceProbe};

#[derive(Clone, Serialize)]
struct Progress {
    stage: String,
    completed: usize,
    total: usize,
    ready: bool,
    error: bool,
}

#[tauri::command]
pub async fn get_local_summary_status() -> Result<serde_json::Value, String> {
    let profile = platform::current();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|e| e.to_string())?;
    let loaded = ready(&client, &profile.summary_health_url(), false).await;
    let stage = if loaded {
        "总结模型已就绪"
    } else {
        "总结模型在后台准备，不影响录音"
    };
    Ok(serde_json::json!({ "ready": loaded, "stage": stage }))
}

async fn ready(client: &reqwest::Client, url: &str, need_fast_realtime: bool) -> bool {
    match client.get(url).send().await {
        Ok(response) if response.status().is_success() => response
            .json::<serde_json::Value>()
            .await
            .map(|value| {
                value["ready"] == true
                    && (!need_fast_realtime || value["fast_realtime"] == true)
            })
            .unwrap_or(false),
        _ => false,
    }
}

/// Optional bootstrap of the service stack (Intel profile launches a PowerShell
/// helper; the RK profile has systemd units and returns immediately).
fn spawn_bootstrap() -> Result<(), String> {
    let profile = platform::current();
    let script = match profile.bootstrap_script.as_ref() {
        Some(script) => script,
        None => return Ok(()),
    };

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("powershell.exe");
        command.args([
            "-NoProfile",
            "-WindowStyle",
            "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ]);
        command.arg(script);
        command.creation_flags(0x08000000);
        command
            .spawn()
            .map_err(|e| format!("无法启动语音服务：{e}"))?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        log::info!(
            "Profile {} declares a bootstrap script ({}) but this platform does not launch it",
            profile.kind.as_str(),
            script.display()
        );
    }

    Ok(())
}

#[tauri::command]
pub async fn ensure_local_services(app: tauri::AppHandle) -> Result<(), String> {
    crate::knowledge_center::require_meeting_mode()?;

    let profile = platform::current();
    if !profile.caps.service_readiness || profile.services.is_empty() {
        return Ok(());
    }

    let probes: &[ServiceProbe] = &profile.services;
    let total = probes.len();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|e| e.to_string())?;

    let started = std::time::Instant::now();
    spawn_bootstrap()?;

    loop {
        let mut states = Vec::with_capacity(total);
        for probe in probes {
            states.push(ready(&client, &probe.url, probe.require_fast_realtime).await);
        }
        let completed = states.iter().filter(|state| **state).count();
        let stage = if completed == total {
            "语音服务已就绪，总结模型在后台准备".to_string()
        } else {
            let pending = probes
                .iter()
                .zip(states.iter())
                .find(|(_, state)| !**state)
                .map(|(probe, _)| probe.name.as_str())
                .unwrap_or("本地服务");
            format!("正在准备{pending}")
        };

        let _ = app.emit(
            "local-services-progress",
            Progress {
                stage,
                completed,
                total,
                ready: completed == total,
                error: false,
            },
        );

        if completed == total {
            return Ok(());
        }

        if started.elapsed().as_secs() >= 180 {
            let missing = probes
                .iter()
                .zip(states.iter())
                .filter_map(|(probe, state)| (!**state).then_some(probe.name.as_str()))
                .collect::<Vec<_>>()
                .join("、");
            let message = format!("{missing}尚未就绪，请检查后台启动状态后重试");
            let _ = app.emit(
                "local-services-progress",
                Progress {
                    stage: message.clone(),
                    completed,
                    total,
                    ready: false,
                    error: true,
                },
            );
            return Err(message);
        }

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
