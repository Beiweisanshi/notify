use crate::event::{AgentEvent, EventType};
use crate::redact::summary_hash;
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct DedupeCache {
    window: Duration,
    seen: HashMap<String, Instant>,
    last_event_type: HashMap<String, EventType>,
}

impl DedupeCache {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            seen: HashMap::new(),
            last_event_type: HashMap::new(),
        }
    }

    pub fn should_emit(&mut self, event: &AgentEvent, now: Instant) -> bool {
        self.expire(now);
        let state_changed = self
            .last_event_type
            .insert(event.session_id.clone(), event.event_type)
            .is_some_and(|previous| previous != event.event_type);
        if state_changed {
            self.remember(event, now);
            return true;
        }

        let duplicate = self.keys(event).into_iter().any(|key| {
            self.seen
                .get(&key)
                .is_some_and(|first_seen| now.duration_since(*first_seen) <= self.window)
        });
        self.remember(event, now);
        !duplicate
    }

    fn remember(&mut self, event: &AgentEvent, now: Instant) {
        for key in self.keys(event) {
            self.seen.insert(key, now);
        }
    }

    fn keys(&self, event: &AgentEvent) -> [String; 2] {
        let event_key = format!(
            "{}:{}:{}",
            event.session_id,
            event.event_type.as_str(),
            event.event_id
        );
        let summary = summary_hash(&[
            &event.message.title,
            &event.message.body,
            event.message.detail.as_deref().unwrap_or_default(),
        ]);
        let summary_key = format!(
            "{}:{}:{}",
            event.session_id,
            event.event_type.as_str(),
            summary
        );
        [event_key, summary_key]
    }

    fn expire(&mut self, now: Instant) {
        self.seen
            .retain(|_, first_seen| now.duration_since(*first_seen) <= self.window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{MessageInfo, ProjectInfo, Severity};

    fn event(event_type: EventType, event_id: &str) -> AgentEvent {
        AgentEvent {
            version: 1,
            event_id: event_id.to_string(),
            event_type,
            severity: Severity::Info,
            tool: "codex".to_string(),
            session_id: "s1".to_string(),
            project: ProjectInfo::from_cwd("D:\\repo"),
            process: None,
            window: None,
            message: MessageInfo {
                title: "Codex 任务完成".to_string(),
                body: "repo · s1 · 点击查看结果".to_string(),
                detail: Some("退出码 0".to_string()),
            },
        }
    }

    #[test]
    fn suppresses_duplicate_events_within_window() {
        let mut cache = DedupeCache::new(Duration::from_secs(30));
        let now = Instant::now();
        let event = event(EventType::TaskCompleted, "e1");

        assert!(cache.should_emit(&event, now));
        assert!(!cache.should_emit(&event, now + Duration::from_secs(5)));
    }

    #[test]
    fn lets_state_changes_bypass_dedupe() {
        let mut cache = DedupeCache::new(Duration::from_secs(30));
        let now = Instant::now();

        assert!(cache.should_emit(&event(EventType::UserConfirmationRequired, "e1"), now));
        assert!(cache.should_emit(
            &event(EventType::TaskCompleted, "e1"),
            now + Duration::from_secs(1)
        ));
    }
}
