//! Conservative idle voice command policy, independent of audio and Tauri.
use std::time::{Duration, Instant};

pub const WAKE_WINDOW: Duration = Duration::from_secs(20);
pub const START_WAIT: Duration = Duration::from_secs(15);

/// Stop-meeting phrases. During a meeting the microphone carries the whole
/// room, so the phrase must be the entire sentence: an aside that merely
/// mentions ending the meeting ("等会儿再结束会议") must not stop it.
pub fn is_stop_command(text: &str) -> bool {
    is_command(text, &["结束会议", "结束录音", "停止会议", "停止录音", "结束本次会议"])
}

fn normalize_command(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '，' | ',' | '。' | '.' | '！' | '!' | '？' | '?' | '、' | '：' | ':' | '；' | ';'))
        .collect()
}

fn is_command(text: &str, commands: &[&str]) -> bool {
    let normalized = normalize_command(text);
    ["", "请", "请帮我", "麻烦", "麻烦你", "麻烦帮我", "请你"]
        .iter()
        .any(|prefix| {
            normalized.strip_prefix(*prefix).is_some_and(|command| commands.contains(&command))
        })
}

/// Normalize separators only; never strip words, negation, or arbitrary prefixes.
pub fn is_start_command(text: &str) -> bool {
    is_command(text, &["开始会议", "开始录音"])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeAction {
    Ignore,
    Wake,
    Start,
    Stop,
    KeepAwaiting,
    Cancel,
    Timeout,
}

#[derive(Debug, PartialEq, Eq)]
pub struct WakeDecision {
    pub action: WakeAction,
    /// Stable diagnostic code; never contains recognized text or service input.
    pub reason: &'static str,
}

/// Which command set applies. Idle listens for starting a meeting; while a
/// meeting is running the only accepted command is stopping it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeContext {
    Idle,
    Meeting,
    Transition,
}

impl WakeContext {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Meeting => "meeting",
            Self::Transition => "transition",
        }
    }
}

#[derive(Default)]
pub struct WakeWindow {
    woken_at: Option<Instant>,
    action_at: Option<Instant>,
}

impl WakeWindow {
    /// Apply final service events to the native policy. Transient failures keep
    /// the original wake instant even when the service renews its own deadline.
    pub fn handle_event(
        &mut self,
        context: WakeContext,
        kind: &str,
        trigger: Option<&str>,
        text: Option<&str>,
        reason: Option<&str>,
        now: Instant,
    ) -> WakeDecision {
        let (action, reason) = if context == WakeContext::Transition {
            *self = Self::default();
            (WakeAction::Ignore, "meeting_transition")
        } else if self.suppressed(now) {
            (WakeAction::Ignore, "start_pending")
        } else {
            match kind {
                "wake" => {
                    self.wake(now);
                    (WakeAction::Wake, "wake_detected")
                }
                "utterance" if matches!(trigger, Some("wake" | "followup")) => {
                    let text = text.unwrap_or("");
                    if text.trim().is_empty() {
                        (WakeAction::Ignore, "empty_utterance")
                    } else if context == WakeContext::Meeting {
                        // The meeting microphone hears the whole room; only an
                        // exact stop sentence counts, never an aside about one.
                        if self.accept_stop(text, now) {
                            (WakeAction::Stop, "stop_command")
                        } else if self.awaiting(now) {
                            (WakeAction::Cancel, "unsupported_command")
                        } else {
                            (WakeAction::Ignore, "wake_window_closed")
                        }
                    } else if self.accept(text, now) {
                        (WakeAction::Start, "start_command")
                    } else if self.awaiting(now) {
                        (WakeAction::Cancel, "unsupported_command")
                    } else {
                        (WakeAction::Ignore, "wake_window_closed")
                    }
                }
                "dropped" if matches!(reason, Some("empty" | "too_short")) => {
                    let reason = if reason == Some("empty") { "empty" } else { "too_short" };
                    if self.awaiting(now) {
                        (WakeAction::KeepAwaiting, reason)
                    } else {
                        (WakeAction::Ignore, "wake_window_closed")
                    }
                }
                "dropped" => match reason {
                    Some("wake_timeout") => (WakeAction::Timeout, "wake_timeout"),
                    Some("overflow") => (WakeAction::Cancel, "overflow"),
                    Some("not_command") => (WakeAction::Cancel, "not_command"),
                    _ => (WakeAction::Cancel, "dropped"),
                },
                "barge" => (WakeAction::Cancel, "barge"),
                "partial" => (WakeAction::Ignore, "partial"),
                _ => (WakeAction::Ignore, "unhandled_event"),
            }
        };
        if matches!(action, WakeAction::Cancel | WakeAction::Timeout) {
            *self = Self::default();
        }
        WakeDecision { action, reason }
    }

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

    pub fn accept_stop(&mut self, text: &str, now: Instant) -> bool {
        if self.awaiting(now) && is_stop_command(text) {
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
        self.expiry_reason(now).is_some()
    }

    /// The absolute native deadline; service-side renewals never move it.
    pub fn expiry_reason(&self, now: Instant) -> Option<&'static str> {
        if self.woken_at.is_some_and(|at| now.saturating_duration_since(at) >= WAKE_WINDOW) {
            Some("native_deadline")
        } else if self.action_at.is_some_and(|at| now.saturating_duration_since(at) >= START_WAIT) {
            Some("start_wait_expired")
        } else {
            None
        }
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

    fn decide(window: &mut WakeWindow, kind: &str, trigger: Option<&str>, text: Option<&str>,
              reason: Option<&str>, at: Instant) -> (WakeAction, &'static str) {
        let decision = window.handle_event(WakeContext::Idle, kind, trigger, text, reason, at);
        (decision.action, decision.reason)
    }

    fn decide_in_meeting(window: &mut WakeWindow, kind: &str, trigger: Option<&str>,
                         text: Option<&str>, at: Instant) -> (WakeAction, &'static str) {
        let decision = window.handle_event(WakeContext::Meeting, kind, trigger, text, None, at);
        (decision.action, decision.reason)
    }

    #[test]
    fn transient_drops_keep_awaiting_without_extending_native_deadline() {
        let now = Instant::now();
        let mut window = WakeWindow::default();
        assert_eq!(decide(&mut window, "wake", None, None, None, now), (WakeAction::Wake, "wake_detected"));
        // The vendor renews its own deadline on these; the native wake instant must not move.
        for (offset, reason) in [(6, "empty"), (12, "too_short"), (18, "empty")] {
            assert_eq!(decide(&mut window, "dropped", None, None, Some(reason), now + Duration::from_secs(offset)),
                       (WakeAction::KeepAwaiting, reason));
        }
        assert_eq!(decide(&mut window, "utterance", Some("wake"), Some("  "), None, now + Duration::from_secs(19)),
                   (WakeAction::Ignore, "empty_utterance"));
        assert!(window.awaiting(now + Duration::from_secs(19)));
        assert_eq!(window.expiry_reason(now + Duration::from_secs(19)), None);
        assert_eq!(window.expiry_reason(now + WAKE_WINDOW), Some("native_deadline"));
        assert_eq!(decide(&mut window, "dropped", None, None, Some("empty"), now + WAKE_WINDOW),
                   (WakeAction::Ignore, "wake_window_closed"));
        assert!(!window.accept("开始会议", now + WAKE_WINDOW));
    }

    #[test]
    fn six_second_gap_after_wake_still_starts() {
        // Regression: the vendor's former 5 s wake timeout cancelled this at about 6 s.
        let now = Instant::now();
        let mut window = WakeWindow::default();
        window.handle_event(WakeContext::Idle, "wake", None, None, None, now);
        assert_eq!(decide(&mut window, "dropped", None, None, Some("too_short"), now + Duration::from_secs(6)).0,
                   WakeAction::KeepAwaiting);
        assert_eq!(decide(&mut window, "utterance", Some("followup"), Some("请开始录音。"), None, now + Duration::from_secs(9)),
                   (WakeAction::Start, "start_command"));
        assert!(window.suppressed(now + Duration::from_secs(10)));
        assert_eq!(decide(&mut window, "utterance", Some("wake"), Some("开始会议"), None, now + Duration::from_secs(10)),
                   (WakeAction::Ignore, "start_pending"));
        assert_eq!(window.expiry_reason(now + Duration::from_secs(9) + START_WAIT), Some("start_wait_expired"));
    }

    #[test]
    fn terminal_events_reset_and_only_exact_final_commands_start() {
        let now = Instant::now();
        let terminal = [
            ("barge", None, None, None, WakeAction::Cancel, "barge"),
            ("dropped", None, None, Some("wake_timeout"), WakeAction::Timeout, "wake_timeout"),
            ("dropped", None, None, Some("overflow"), WakeAction::Cancel, "overflow"),
            ("dropped", None, None, Some("not_command"), WakeAction::Cancel, "not_command"),
            ("dropped", None, None, None, WakeAction::Cancel, "dropped"),
            ("utterance", Some("wake"), Some("不要开始会议"), None, WakeAction::Cancel, "unsupported_command"),
            ("utterance", Some("followup"), Some("开始"), None, WakeAction::Cancel, "unsupported_command"),
        ];
        for (kind, trigger, text, reason, action, code) in terminal {
            let mut window = WakeWindow::default();
            window.handle_event(WakeContext::Idle, "wake", None, None, None, now);
            assert_eq!(decide(&mut window, kind, trigger, text, reason, now + Duration::from_secs(1)), (action, code), "{kind} {code}");
            assert!(!window.awaiting(now + Duration::from_secs(1)), "{kind} {code}");
            assert!(!window.accept("开始会议", now + Duration::from_secs(1)), "{kind} {code}");
        }
        let mut window = WakeWindow::default();
        window.handle_event(WakeContext::Idle, "wake", None, None, None, now);
        for (kind, trigger) in [("partial", Some("wake")), ("utterance", Some("free")), ("speech_end", Some("wake"))] {
            assert_eq!(decide(&mut window, kind, trigger, Some("开始会议"), None, now).0, WakeAction::Ignore, "{kind}");
        }
        assert!(window.awaiting(now));
        let mut idle = WakeWindow::default();
        assert_eq!(decide(&mut idle, "utterance", Some("wake"), Some("开始会议"), None, now),
                   (WakeAction::Ignore, "wake_window_closed"));
        assert!(!idle.suppressed(now));
    }

    #[test]
    fn meeting_context_accepts_only_whole_stop_sentences() {
        for text in ["结束会议", "结束录音。", "请，结束会议！", "麻烦帮我停止录音", "结束本次会议"] {
            assert!(is_stop_command(text), "{text}");
        }
        // The room microphone hears conversation; only the sentence itself counts.
        for text in ["结束", "会议结束", "要不要结束会议", "等会结束会议再聊", "不要结束录音", "我说结束会议",
                     "结束会议吗", "结束会议吧", "开始会议", "暂停录音", "退出会议", "取消会议"] {
            assert!(!is_stop_command(text), "{text}");
        }
    }

    #[test]
    fn meeting_context_never_starts_and_idle_never_stops() {
        let now = Instant::now();
        let mut window = WakeWindow::default();
        decide_in_meeting(&mut window, "wake", None, None, now);
        // "开始会议" is meaningless while the meeting already runs.
        assert_eq!(decide_in_meeting(&mut window, "utterance", Some("wake"), Some("开始会议"), now + Duration::from_secs(1)),
                   (WakeAction::Cancel, "unsupported_command"));
        let mut window = WakeWindow::default();
        decide_in_meeting(&mut window, "wake", None, None, now);
        assert_eq!(decide_in_meeting(&mut window, "utterance", Some("wake"), Some("结束会议"), now + Duration::from_secs(1)),
                   (WakeAction::Stop, "stop_command"));
        assert!(window.suppressed(now + Duration::from_secs(2)));
        // Idle must not treat a stop command as a start command.
        let mut idle = WakeWindow::default();
        decide(&mut idle, "wake", None, None, None, now);
        assert_eq!(decide(&mut idle, "utterance", Some("wake"), Some("结束会议"), None, now + Duration::from_secs(1)),
                   (WakeAction::Cancel, "unsupported_command"));
    }

    #[test]
    fn stop_command_requires_a_recent_wake() {
        let now = Instant::now();
        let mut window = WakeWindow::default();
        assert_eq!(decide_in_meeting(&mut window, "utterance", Some("wake"), Some("结束会议"), now),
                   (WakeAction::Ignore, "wake_window_closed"));
        decide_in_meeting(&mut window, "wake", None, None, now);
        // Past the window the policy stops listening rather than rejecting.
        assert_eq!(decide_in_meeting(&mut window, "utterance", Some("wake"), Some("结束会议"), now + WAKE_WINDOW),
                   (WakeAction::Ignore, "wake_window_closed"));
    }

    #[test]
    fn transitions_discard_wake_and_reject_all_actions() {
        let now = Instant::now();
        let mut window = WakeWindow::default();
        window.wake(now);
        for (kind, text) in [("wake", None), ("utterance", Some("结束会议")),
                             ("utterance", Some("开始会议"))] {
            let decision = window.handle_event(WakeContext::Transition, kind, Some("wake"), text, None, now);
            assert_eq!(decision.action, WakeAction::Ignore);
            assert_eq!(decision.reason, "meeting_transition");
            assert!(!window.awaiting(now));
        }
        assert_eq!(window.handle_event(WakeContext::Meeting, "utterance", Some("wake"),
            Some("结束会议"), None, now).action, WakeAction::Ignore);
        window.wake(now);
        assert_eq!(window.handle_event(WakeContext::Meeting, "utterance", Some("wake"),
            Some("结束会议"), None, now).action, WakeAction::Stop);
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
