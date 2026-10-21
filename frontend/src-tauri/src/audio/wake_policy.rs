//! Conservative idle voice command policy, independent of audio and Tauri.
use std::time::{Duration, Instant};

pub const WAKE_WINDOW: Duration = Duration::from_secs(20);
pub const START_WAIT: Duration = Duration::from_secs(15);

/// Normalize separators only; never strip words, negation, or arbitrary prefixes.
pub fn is_start_command(text: &str) -> bool {
    let normalized: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '，' | ',' | '。' | '.' | '！' | '!' | '？' | '?' | '、' | '：' | ':' | '；' | ';'))
        .collect();
    ["", "请", "请帮我", "麻烦", "麻烦你", "麻烦帮我", "请你"]
        .iter()
        .any(|prefix| {
            normalized.strip_prefix(*prefix).is_some_and(|command| {
                matches!(command, "开始会议" | "开始录音")
            })
        })
}

#[derive(Default)]
pub struct WakeWindow {
    woken_at: Option<Instant>,
    action_at: Option<Instant>,
}

impl WakeWindow {
    pub fn wake(&mut self, now: Instant) {
        if !self.suppressed(now) {
            self.woken_at = Some(now);
        }
    }

    pub fn awaiting(&self, now: Instant) -> bool {
        self.woken_at.is_some_and(|at| now.saturating_duration_since(at) < WAKE_WINDOW)
            && !self.suppressed(now)
    }

    pub fn accept(&mut self, text: &str, now: Instant) -> bool {
        if self.awaiting(now) && is_start_command(text) {
            self.woken_at = None;
            self.action_at = Some(now);
            true
        } else {
            false
        }
    }

    pub fn suppressed(&self, now: Instant) -> bool {
        self.action_at.is_some_and(|at| now.saturating_duration_since(at) < START_WAIT)
    }

    pub fn expired(&self, now: Instant) -> bool {
        self.woken_at.is_some_and(|at| now.saturating_duration_since(at) >= WAKE_WINDOW)
            || self.action_at.is_some_and(|at| now.saturating_duration_since(at) >= START_WAIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_complete_start_commands() {
        for text in ["开始会议", "开始录音。", "请，开始会议！", "请帮我开始录音", "麻烦你开始会议", " 开始 录音 "] {
            assert!(is_start_command(text), "{text}");
        }
        for text in ["开始", "开始会", "不开始会议", "不要开始录音", "请不要开始会议", "取消开始录音", "是否开始会议", "我说开始会议", "开始会议然后停止", "开始录音吗", "开始会议吧", "stop", "停止录音", "会议开始", "请帮我不要开始录音"] {
            assert!(!is_start_command(text), "{text}");
        }
    }

    #[test]
    fn requires_recent_wake_and_suppresses_duplicate_actions() {
        let now = Instant::now();
        let mut window = WakeWindow::default();
        assert!(!window.accept("开始会议", now));
        window.wake(now);
        assert!(!window.accept("开始", now));
        assert!(window.accept("开始会议", now + Duration::from_secs(19)));
        window.wake(now + Duration::from_secs(20));
        assert!(!window.accept("开始录音", now + Duration::from_secs(20)));
        assert!(window.suppressed(now + Duration::from_secs(33)));
        assert!(window.expired(now + Duration::from_secs(34)));
    }

    #[test]
    fn wake_window_expires_at_twenty_seconds() {
        let now = Instant::now();
        let mut window = WakeWindow::default();
        window.wake(now);
        assert!(window.awaiting(now + Duration::from_secs(19)));
        assert!(window.expired(now + WAKE_WINDOW));
        assert!(!window.accept("开始会议", now + WAKE_WINDOW));
    }
}
