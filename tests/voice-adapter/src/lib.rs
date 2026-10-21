#![allow(dead_code)]

// Compile the production modules directly; do not duplicate the adapter in tests.
#[path = "../../../frontend/src-tauri/src/audio/transcription/provider.rs"]
mod provider;
#[path = "../../../frontend/src-tauri/src/audio/transcription/rk_asr_provider.rs"]
mod rk_asr_provider;
#[path = "../../../frontend/src-tauri/src/audio/transcription/rk_policy.rs"]
mod rk_policy;
#[path = "../../../frontend/src-tauri/src/audio/transcription/bossai_provider.rs"]
mod bossai_provider;
#[path = "../../../frontend/src-tauri/src/audio/transcription/task.rs"]
mod task;
#[path = "../../../frontend/src-tauri/src/audio/wake_policy.rs"]
mod wake_policy;
#[path = "../../../frontend/src-tauri/src/audio/devices/capture_guard.rs"]
mod capture_guard;
#[path = "../../../frontend/src-tauri/src/audio/live_transcript_protocol.rs"]
mod live_transcript_protocol;

#[cfg(test)]
mod wake_policy_stop {
    use super::wake_policy::{
        is_start_command, is_stop_command, WakeAction, WakeContext, WakeWindow,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn stop_commands_are_separate_from_start_commands() {
        for text in ["结束会议", "结束录音。", "请，结束会议！", "麻烦帮我停止录音", "结束本次会议"] {
            assert!(is_stop_command(text), "{text}");
        }
        for text in ["结束", "会议结束", "要不要结束会议", "等会结束会议再聊", "不要结束录音",
                     "我说结束会议", "结束会议吗", "暂停录音", "开始会议"] {
            assert!(!is_stop_command(text), "{text}");
        }
        // Exact transcribed sentences observed from the deployed ASR model.
        for text in ["结束会议", "麻烦你结束会议", "请结束录音"] {
            assert!(is_stop_command(text), "{text}");
        }
        for text in ["等会儿再结束会议吧", "别结束会议", "结束会议之后我们再聊", "我建议结束会议"] {
            assert!(!is_stop_command(text), "{text}");
        }
        // The two sets never overlap in either direction.
        for text in ["开始会议", "开始录音", "请开始会议"] {
            assert!(!is_stop_command(text), "{text}");
        }
        for text in ["结束会议", "停止录音"] {
            assert!(!is_start_command(text), "{text}");
        }
    }

    #[test]
    fn meeting_context_switches_the_accepted_command_set() {
        let now = Instant::now();
        let mut meeting = WakeWindow::default();
        meeting.handle_event(WakeContext::Meeting, "wake", None, None, None, now);
        assert_eq!(meeting.handle_event(WakeContext::Meeting, "utterance", Some("wake"),
                   Some("结束会议"), None, now + Duration::from_secs(1)).action, WakeAction::Stop);
        let mut idle = WakeWindow::default();
        idle.handle_event(WakeContext::Idle, "wake", None, None, None, now);
        assert_eq!(idle.handle_event(WakeContext::Idle, "utterance", Some("wake"),
                   Some("开始会议"), None, now + Duration::from_secs(1)).action, WakeAction::Start);
        // Neither context accepts the other one's command.
        let mut meeting = WakeWindow::default();
        meeting.handle_event(WakeContext::Meeting, "wake", None, None, None, now);
        assert_eq!(meeting.handle_event(WakeContext::Meeting, "utterance", Some("wake"),
                   Some("开始会议"), None, now + Duration::from_secs(1)).action, WakeAction::Cancel);
        let mut idle = WakeWindow::default();
        idle.handle_event(WakeContext::Idle, "wake", None, None, None, now);
        assert_eq!(idle.handle_event(WakeContext::Idle, "utterance", Some("wake"),
                   Some("结束会议"), None, now + Duration::from_secs(1)).action, WakeAction::Cancel);
    }

    #[test]
    fn stop_still_requires_the_wake_word_within_the_window() {
        let now = Instant::now();
        let mut window = WakeWindow::default();
        // Ambient meeting talk alone must never end the meeting.
        assert_eq!(window.handle_event(WakeContext::Meeting, "utterance", Some("wake"),
                   Some("结束会议"), None, now).action, WakeAction::Ignore);
        window.handle_event(WakeContext::Meeting, "wake", None, None, None, now);
        assert_eq!(window.handle_event(WakeContext::Meeting, "utterance", Some("wake"),
                   Some("结束会议"), None, now + Duration::from_secs(19)).action, WakeAction::Stop);
        let mut late = WakeWindow::default();
        late.handle_event(WakeContext::Meeting, "wake", None, None, None, now);
        assert_eq!(late.handle_event(WakeContext::Meeting, "utterance", Some("wake"),
                   Some("结束会议"), None, now + Duration::from_secs(20)).action, WakeAction::Ignore);
    }
}
