use crate::event::{AgentEvent, EventType};
use crate::redact::truncate_chars;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationDecision {
    Notify,
    Suppress,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationView {
    pub title: String,
    pub body: String,
    pub detail: Option<String>,
}

pub fn should_notify(event_type: EventType) -> bool {
    matches!(
        event_type,
        EventType::TaskCompleted
            | EventType::TaskFailed
            | EventType::UserConfirmationRequired
            | EventType::UserInputRequired
            | EventType::ToolBlocked
    )
}

pub fn notification_view(event: &AgentEvent) -> Option<NotificationView> {
    should_notify(event.event_type).then(|| NotificationView {
        title: event.message.title.clone(),
        body: event.message.body.clone(),
        detail: event
            .message
            .detail
            .as_deref()
            .map(|detail| truncate_chars(detail, 160)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_does_not_notify() {
        assert!(!should_notify(EventType::Heartbeat));
        assert!(!should_notify(EventType::TaskStarted));
    }

    #[test]
    fn required_events_notify() {
        assert!(should_notify(EventType::TaskCompleted));
        assert!(should_notify(EventType::TaskFailed));
        assert!(should_notify(EventType::UserConfirmationRequired));
        assert!(should_notify(EventType::UserInputRequired));
        assert!(should_notify(EventType::ToolBlocked));
    }
}
