use crate::event::{AgentEvent, EventType};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct DedupeCache {
    window: Duration,
    last_state: HashMap<String, StateEntry>,
}

#[derive(Debug, Clone, Copy)]
struct StateEntry {
    event_type: EventType,
    at: Instant,
}

impl DedupeCache {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            last_state: HashMap::new(),
        }
    }

    pub fn should_emit(&mut self, event: &AgentEvent, now: Instant) -> bool {
        self.last_state
            .retain(|_, entry| now.duration_since(entry.at) <= self.window);
        let previous = self.last_state.insert(
            event.session_id.clone(),
            StateEntry {
                event_type: event.event_type,
                at: now,
            },
        );
        previous.is_none_or(|previous| previous.event_type != event.event_type)
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

    fn event_with_summary(event_type: EventType, event_id: &str, body: &str) -> AgentEvent {
        let mut e = event(event_type, event_id);
        e.message.body = body.to_string();
        e
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

    #[test]
    fn suppresses_same_state_even_when_event_id_and_summary_differ() {
        let mut cache = DedupeCache::new(Duration::from_secs(30));
        let now = Instant::now();

        assert!(cache.should_emit(
            &event_with_summary(EventType::TaskCompleted, "e1", "first body"),
            now
        ));
        assert!(!cache.should_emit(
            &event_with_summary(EventType::TaskCompleted, "e2", "different body"),
            now + Duration::from_secs(5)
        ));
    }

    #[test]
    fn lets_same_state_through_after_window_expires() {
        let mut cache = DedupeCache::new(Duration::from_secs(30));
        let now = Instant::now();

        assert!(cache.should_emit(&event(EventType::TaskCompleted, "e1"), now));
        assert!(cache.should_emit(
            &event(EventType::TaskCompleted, "e2"),
            now + Duration::from_secs(31)
        ));
    }

    #[test]
    fn evicts_stale_entries_so_state_does_not_grow_unbounded() {
        let mut cache = DedupeCache::new(Duration::from_secs(30));
        let now = Instant::now();

        for index in 0..1000 {
            let mut e = event(EventType::TaskCompleted, "e1");
            e.session_id = format!("s{index}");
            cache.should_emit(&e, now);
        }

        let mut later = event(EventType::TaskCompleted, "e1");
        later.session_id = "s-later".into();
        cache.should_emit(&later, now + Duration::from_secs(31));

        assert_eq!(cache.last_state.len(), 1);
    }
}
