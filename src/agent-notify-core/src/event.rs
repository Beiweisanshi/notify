use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventType {
    #[serde(rename = "task.started")]
    TaskStarted,
    #[serde(rename = "task.completed")]
    TaskCompleted,
    #[serde(rename = "task.failed")]
    TaskFailed,
    #[serde(rename = "user.confirmation_required")]
    UserConfirmationRequired,
    #[serde(rename = "user.input_required")]
    UserInputRequired,
    #[serde(rename = "tool.blocked")]
    ToolBlocked,
    #[serde(rename = "heartbeat")]
    Heartbeat,
}

impl EventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TaskStarted => "task.started",
            Self::TaskCompleted => "task.completed",
            Self::TaskFailed => "task.failed",
            Self::UserConfirmationRequired => "user.confirmation_required",
            Self::UserInputRequired => "user.input_required",
            Self::ToolBlocked => "tool.blocked",
            Self::Heartbeat => "heartbeat",
        }
    }

    pub fn session_status(self) -> SessionStatus {
        match self {
            Self::TaskStarted | Self::Heartbeat => SessionStatus::Running,
            Self::TaskCompleted => SessionStatus::Completed,
            Self::TaskFailed | Self::ToolBlocked => SessionStatus::Failed,
            Self::UserConfirmationRequired | Self::UserInputRequired => SessionStatus::WaitingUser,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    WaitingUser,
    Completed,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub cwd: String,
    pub name: String,
}

impl ProjectInfo {
    pub fn from_cwd(cwd: impl Into<String>) -> Self {
        let cwd = cwd.into();
        let name = Path::new(&cwd)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("project")
            .to_string();
        Self { cwd, name }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hwnd: Option<isize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageInfo {
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub version: u8,
    pub event_id: String,
    pub event_type: EventType,
    pub severity: Severity,
    pub tool: String,
    pub session_id: String,
    pub project: ProjectInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<ProcessInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowInfo>,
    pub message: MessageInfo,
}

impl AgentEvent {
    pub fn validate(&self) -> Result<(), EventValidationError> {
        if self.version != 1 {
            return Err(EventValidationError::UnsupportedVersion(self.version));
        }
        if self.event_id.trim().is_empty() {
            return Err(EventValidationError::Missing("eventId"));
        }
        if self.tool.trim().is_empty() {
            return Err(EventValidationError::Missing("tool"));
        }
        if self.session_id.trim().is_empty() {
            return Err(EventValidationError::Missing("sessionId"));
        }
        if self.project.cwd.trim().is_empty() {
            return Err(EventValidationError::Missing("project.cwd"));
        }
        if self.message.title.trim().is_empty() {
            return Err(EventValidationError::Missing("message.title"));
        }
        if self.message.body.trim().is_empty() {
            return Err(EventValidationError::Missing("message.body"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub tool: String,
    pub project: ProjectInfo,
    pub status: SessionStatus,
    pub last_event_type: EventType,
    pub last_message: MessageInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<ProcessInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowInfo>,
    pub updated_at: String,
}

impl SessionInfo {
    pub fn from_event(event: &AgentEvent, updated_at: impl Into<String>) -> Self {
        Self {
            session_id: event.session_id.clone(),
            tool: event.tool.clone(),
            project: event.project.clone(),
            status: event.event_type.session_status(),
            last_event_type: event.event_type,
            last_message: event.message.clone(),
            process: event.process.clone(),
            window: event.window.clone(),
            updated_at: updated_at.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EventValidationError {
    #[error("unsupported event version {0}")]
    UnsupportedVersion(u8),
    #[error("missing required field {0}")]
    Missing(&'static str),
}
