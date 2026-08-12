pub fn format_timestamp(seconds: f64) -> String {
    let total_seconds = seconds as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let secs = total_seconds % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, secs)
}

fn try_stdin_cmd(program: &str, args: &[&str], text: &str, display: Option<&str>) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(display) = display {
        command.env("DISPLAY", display);
    }

    let mut child = command
        .spawn()
        .map_err(|e| format!("spawn {program}: {e}"))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("{program} stdin unavailable"))?;
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("write {program} stdin: {e}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("wait {program}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("{program} failed: {stderr}"))
    }
}

#[tauri::command]
pub fn copy_to_clipboard(text: String) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string());
        if try_stdin_cmd(
            "xclip",
            &["-selection", "clipboard", "-i"],
            &text,
            Some(&display),
        )
        .is_ok()
        {
            return Ok(());
        }
        if try_stdin_cmd("xsel", &["--clipboard", "--input"], &text, Some(&display)).is_ok() {
            return Ok(());
        }
        const PYTHON_CLIPBOARD: &str = r#"
import os, sys
os.environ.setdefault("DISPLAY", ":0")
text = sys.stdin.read()
import gi
gi.require_version("Gtk", "3.0")
from gi.repository import Gtk, Gdk
clipboard = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
clipboard.set_text(text, -1)
clipboard.store()
while Gtk.events_pending():
    Gtk.main_iteration()
"#;
        try_stdin_cmd("python3", &["-c", PYTHON_CLIPBOARD], &text, Some(&display))
            .map_err(|_| "复制失败：本机没有可用的剪贴板工具".to_string())
    }

    #[cfg(target_os = "macos")]
    {
        try_stdin_cmd("pbcopy", &[], &text, None)
    }

    #[cfg(target_os = "windows")]
    {
        try_stdin_cmd("clip", &[], &text, None)
    }
}

/// Opens macOS System Settings to a specific privacy preference pane
#[cfg(target_os = "macos")]
#[tauri::command]
pub async fn open_system_settings(preference_pane: String) -> Result<(), String> {
    use std::process::Command;

    let url = format!(
        "x-apple.systempreferences:com.apple.preference.security?{}",
        preference_pane
    );

    Command::new("open")
        .arg(&url)
        .spawn()
        .map_err(|e| format!("Failed to open system settings: {}", e))?;

    Ok(())
}
