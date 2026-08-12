use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: Option<String>,
    pub title: String,
    pub body: String,
    pub notification_type: NotificationType,
    pub priority: NotificationPriority,
    pub timeout: NotificationTimeout,
    pub icon: Option<String>,
    pub sound: bool,
    pub actions: Vec<NotificationAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotificationType {
    RecordingStarted,
    RecordingStopped,
    RecordingPaused,
    RecordingResumed,
    TranscriptionComplete,
    MeetingReminder(u64), // Duration in minutes
    SystemError(String),
    Test, // For testing notifications
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotificationPriority {
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotificationTimeout {
    Never,
    Seconds(u64),
    Default,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationAction {
    pub id: String,
    pub title: String,
    pub action_type: NotificationActionType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotificationActionType {
    Button,
    Reply,
}

impl Notification {
    pub fn new(title: impl Into<String>, body: impl Into<String>, notification_type: NotificationType) -> Self {
        Self {
            id: None,
            title: title.into(),
            body: body.into(),
            notification_type,
            priority: NotificationPriority::Normal,
            timeout: NotificationTimeout::Default,
            icon: None,
            sound: true,
            actions: vec![],
        }
    }

    pub fn with_priority(mut self, priority: NotificationPriority) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_timeout(mut self, timeout: NotificationTimeout) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_sound(mut self, sound: bool) -> Self {
        self.sound = sound;
        self
    }

    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn add_action(mut self, action: NotificationAction) -> Self {
        self.actions.push(action);
        self
    }
}

impl Default for NotificationPriority {
    fn default() -> Self {
        NotificationPriority::Normal
    }
}

impl Default for NotificationTimeout {
    fn default() -> Self {
        NotificationTimeout::Default
    }
}

// Helper functions for creating common notifications
impl Notification {
    pub fn recording_started(meeting_name: Option<String>) -> Self {
        let body = match meeting_name {
            Some(name) => format!("已开始录制会议：{}", name),
            None => "录音已开始。请告知会场其他人正在录音。".to_string(),
        };

        Notification::new("会议助手", body, NotificationType::RecordingStarted)
            .with_priority(NotificationPriority::High)
            .with_timeout(NotificationTimeout::Seconds(5))
    }

    pub fn recording_stopped() -> Self {
        Notification::new(
            "会议助手",
            "录音已停止并保存",
            NotificationType::RecordingStopped
        )
        .with_priority(NotificationPriority::Normal)
        .with_timeout(NotificationTimeout::Seconds(3))
    }

    pub fn recording_paused() -> Self {
        Notification::new(
            "会议助手",
            "录音已暂停",
            NotificationType::RecordingPaused
        )
        .with_priority(NotificationPriority::Normal)
        .with_timeout(NotificationTimeout::Seconds(3))
    }

    pub fn recording_resumed() -> Self {
        Notification::new(
            "会议助手",
            "录音已继续",
            NotificationType::RecordingResumed
        )
        .with_priority(NotificationPriority::Normal)
        .with_timeout(NotificationTimeout::Seconds(3))
    }

    pub fn transcription_complete(file_path: Option<String>) -> Self {
        let body = match file_path {
            Some(path) => format!("转写已完成并保存到：{}", path),
            None => "转写已完成".to_string(),
        };

        Notification::new("会议助手", body, NotificationType::TranscriptionComplete)
            .with_priority(NotificationPriority::Normal)
            .with_timeout(NotificationTimeout::Seconds(5))
    }

    pub fn meeting_reminder(minutes_until: u64, meeting_title: Option<String>) -> Self {
        let body = match meeting_title {
            Some(title) => format!("会议「{}」将在 {} 分钟后开始", title, minutes_until),
            None => format!("会议将在 {} 分钟后开始", minutes_until),
        };

        Notification::new("会议助手", body, NotificationType::MeetingReminder(minutes_until))
            .with_priority(NotificationPriority::High)
            .with_timeout(NotificationTimeout::Seconds(10))
    }

    pub fn system_error(error: impl Into<String>) -> Self {
        let error_string = error.into();
        Notification::new(
            "会议助手出错",
            error_string.clone(),
            NotificationType::SystemError(error_string)
        )
        .with_priority(NotificationPriority::Critical)
        .with_timeout(NotificationTimeout::Never)
    }

    pub fn test_notification() -> Self {
        Notification::new(
            "会议助手",
            "这是一条测试通知，用于确认通知系统正常工作",
            NotificationType::Test
        )
        .with_priority(NotificationPriority::Normal)
        .with_timeout(NotificationTimeout::Seconds(5))
    }
}