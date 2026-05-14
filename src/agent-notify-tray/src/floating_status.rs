use agent_notify_core::{
    AgentEvent, EventType, MessageInfo, ProcessInfo, RuntimeConfig, SessionInfo, SessionStatus,
    WindowInfo, notification_view,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const DEFAULT_HOTKEY: &str = "Ctrl+Shift+Space";
const STALE_SESSION_SECONDS: i64 = 30 * 60;

fn default_hotkey() -> String {
    DEFAULT_HOTKEY.to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FloatingBarState {
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub expanded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<FloatingBarPosition>,
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
}

impl Default for FloatingBarState {
    fn default() -> Self {
        Self {
            hidden: false,
            expanded: false,
            position: None,
            hotkey: DEFAULT_HOTKEY.to_string(),
        }
    }
}

impl FloatingBarState {
    pub fn normalized(mut self) -> Self {
        self.hotkey = self.hotkey.trim().to_string();
        if self.hotkey.is_empty() {
            self.hotkey = DEFAULT_HOTKEY.to_string();
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FloatingBarPosition {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FloatingNotificationRecord {
    pub notification_id: String,
    pub session_id: String,
    #[serde(default, skip_serializing)]
    pub process: Option<ProcessInfo>,
    #[serde(default, skip_serializing)]
    pub window: Option<WindowInfo>,
    pub event_type: EventType,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clicked_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FloatingStatusSnapshot {
    pub summary: FloatingStatusSummary,
    pub state: FloatingBarState,
    pub sessions: Vec<FloatingStatusSession>,
    pub notifications: Vec<FloatingStatusNotification>,
    pub generated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FloatingStatusSummary {
    pub terminal_count: usize,
    pub running_count: usize,
    pub completed_count: usize,
    pub unopened_completed_notifications: usize,
    pub counts_best_effort: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FloatingStatusSession {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_session_ids: Vec<String>,
    pub tool: String,
    pub project_name: String,
    pub status: SessionStatus,
    pub last_event_type: EventType,
    pub last_message: MessageInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowInfo>,
    pub updated_at: String,
    pub unopened_notification_count: usize,
    pub terminal_liveness: TerminalLiveness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalLiveness {
    Exact,
    BestEffort,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FloatingStatusNotification {
    pub notification_id: String,
    pub session_id: String,
    pub event_type: EventType,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clicked_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationClickError {
    NotFound,
    SessionMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalState {
    countable: bool,
    liveness: TerminalLiveness,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum TerminalIdentity {
    Hwnd(isize),
    WindowPid(u32),
    Session(String),
}

pub fn floating_bar_state_path(config: &RuntimeConfig) -> PathBuf {
    PathBuf::from(&config.auth.token_file)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(agent_notify_core::agent_notify_dir)
        .join("floating-status-bar.json")
}

pub fn load_floating_bar_state(path: &Path) -> FloatingBarState {
    let Ok(bytes) = fs::read(path) else {
        return FloatingBarState::default();
    };
    serde_json::from_slice::<FloatingBarState>(strip_utf8_bom(&bytes))
        .map(FloatingBarState::normalized)
        .unwrap_or_default()
}

pub fn save_floating_bar_state(path: &Path, state: &FloatingBarState) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&state.clone().normalized()).map_err(invalid_data)?;
    fs::write(path, text)
}

pub fn notification_record_from_event(
    event: &AgentEvent,
    created_at: impl Into<String>,
) -> Option<FloatingNotificationRecord> {
    notification_view(event).map(|view| FloatingNotificationRecord {
        notification_id: event.event_id.clone(),
        session_id: event.session_id.clone(),
        process: event.process.clone(),
        window: event.window.clone(),
        event_type: event.event_type,
        title: view.title,
        body: view.body,
        detail: view.detail,
        created_at: created_at.into(),
        clicked_at: None,
    })
}

pub fn build_floating_status_snapshot(
    sessions: impl IntoIterator<Item = SessionInfo>,
    notifications: impl IntoIterator<Item = FloatingNotificationRecord>,
    state: FloatingBarState,
    now: DateTime<Utc>,
) -> FloatingStatusSnapshot {
    let sessions = sessions.into_iter().collect::<Vec<_>>();
    let notifications = notifications.into_iter().collect::<Vec<_>>();
    let terminal_states = sessions
        .iter()
        .map(|session| (session.session_id.clone(), terminal_state(session, now)))
        .collect::<HashMap<_, _>>();
    let session_terminal_ids = sessions
        .iter()
        .map(|session| (session.session_id.clone(), terminal_identity(session)))
        .collect::<HashMap<_, _>>();
    let mut session_ids_by_terminal = sessions.iter().fold(
        HashMap::<TerminalIdentity, Vec<String>>::new(),
        |mut groups, session| {
            if let Some(identity) = session_terminal_ids.get(&session.session_id) {
                groups
                    .entry(identity.clone())
                    .or_default()
                    .push(session.session_id.clone());
            }
            groups
        },
    );
    for session_ids in session_ids_by_terminal.values_mut() {
        session_ids.sort();
    }

    let mut latest_by_terminal = HashMap::<TerminalIdentity, (SessionInfo, TerminalState)>::new();
    for session in &sessions {
        let Some(terminal_state) = terminal_states.get(&session.session_id).copied() else {
            continue;
        };
        if !terminal_state.countable {
            continue;
        }
        let Some(identity) = session_terminal_ids.get(&session.session_id) else {
            continue;
        };
        latest_by_terminal
            .entry(identity.clone())
            .and_modify(|(current, state)| {
                if is_newer_session(session, current) {
                    *current = session.clone();
                    *state = terminal_state;
                }
            })
            .or_insert_with(|| (session.clone(), terminal_state));
    }

    let completed_terminal_ids = latest_by_terminal
        .iter()
        .filter_map(|(identity, (session, _))| {
            (session.status == SessionStatus::Completed).then_some(identity.clone())
        })
        .collect::<HashSet<_>>();

    let visible_notifications = notifications
        .iter()
        .filter(|notification| {
            notification.clicked_at.is_none()
                && session_terminal_ids
                    .get(&notification.session_id)
                    .is_some_and(|identity| completed_terminal_ids.contains(identity))
        })
        .cloned()
        .collect::<Vec<_>>();
    let unopened_by_terminal = visible_notifications.iter().fold(
        HashMap::<TerminalIdentity, usize>::new(),
        |mut counts, notification| {
            if let Some(identity) = session_terminal_ids.get(&notification.session_id) {
                *counts.entry(identity.clone()).or_default() += 1;
            }
            counts
        },
    );

    let mut ui_sessions = latest_by_terminal
        .into_iter()
        .map(|(identity, (session, terminal_state))| {
            let related_session_ids = session_ids_by_terminal
                .get(&identity)
                .cloned()
                .unwrap_or_else(|| vec![session.session_id.clone()]);
            FloatingStatusSession {
                unopened_notification_count: unopened_by_terminal
                    .get(&identity)
                    .copied()
                    .unwrap_or_default(),
                terminal_liveness: terminal_state.liveness,
                related_session_ids,
                project_name: session.project.name.clone(),
                session_id: session.session_id,
                tool: session.tool,
                status: session.status,
                last_event_type: session.last_event_type,
                last_message: session.last_message,
                window: session.window,
                updated_at: session.updated_at,
            }
        })
        .collect::<Vec<_>>();
    ui_sessions.sort_by(|left, right| {
        compare_updated_at(&right.updated_at, &left.updated_at)
            .then_with(|| left.session_id.cmp(&right.session_id))
    });

    let summary = FloatingStatusSummary {
        terminal_count: ui_sessions.len(),
        running_count: ui_sessions
            .iter()
            .filter(|session| session.status == SessionStatus::Running)
            .count(),
        completed_count: ui_sessions
            .iter()
            .filter(|session| session.status == SessionStatus::Completed)
            .count(),
        unopened_completed_notifications: visible_notifications.len(),
        counts_best_effort: ui_sessions
            .iter()
            .any(|session| session.terminal_liveness == TerminalLiveness::BestEffort),
    };

    let mut ui_notifications = visible_notifications
        .into_iter()
        .map(|notification| FloatingStatusNotification {
            notification_id: notification.notification_id,
            session_id: notification.session_id,
            event_type: notification.event_type,
            title: notification.title,
            body: notification.body,
            detail: notification.detail,
            created_at: notification.created_at,
            clicked_at: notification.clicked_at,
        })
        .collect::<Vec<_>>();
    ui_notifications.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.notification_id.cmp(&right.notification_id))
    });

    FloatingStatusSnapshot {
        summary,
        state: state.normalized(),
        sessions: ui_sessions,
        notifications: ui_notifications,
        generated_at: now.to_rfc3339(),
    }
}

pub fn mark_notification_clicked(
    notifications: &mut HashMap<String, FloatingNotificationRecord>,
    notification_id: &str,
    session_id: &str,
    clicked_at: impl Into<String>,
) -> Result<bool, NotificationClickError> {
    let Some(notification) = notifications.get_mut(notification_id) else {
        return Err(NotificationClickError::NotFound);
    };
    if notification.session_id != session_id {
        return Err(NotificationClickError::SessionMismatch);
    }
    if notification.clicked_at.is_some() {
        return Ok(false);
    }
    notification.clicked_at = Some(clicked_at.into());
    Ok(true)
}

fn terminal_state(session: &SessionInfo, now: DateTime<Utc>) -> TerminalState {
    if let Some(hwnd) = session
        .window
        .as_ref()
        .and_then(|window| window.hwnd)
        .filter(|hwnd| *hwnd != 0)
    {
        return TerminalState {
            countable: is_live_hwnd(hwnd),
            liveness: TerminalLiveness::Exact,
        };
    }

    TerminalState {
        countable: has_terminal_hint(session) && is_recent_session(session, now),
        liveness: TerminalLiveness::BestEffort,
    }
}

fn terminal_identity(session: &SessionInfo) -> TerminalIdentity {
    if let Some(hwnd) = session
        .window
        .as_ref()
        .and_then(|window| window.hwnd)
        .filter(|hwnd| *hwnd != 0)
    {
        return TerminalIdentity::Hwnd(hwnd);
    }

    if let Some(pid) = session
        .window
        .as_ref()
        .and_then(|window| window.pid)
        .filter(|pid| *pid != 0)
    {
        return TerminalIdentity::WindowPid(pid);
    }

    TerminalIdentity::Session(session.session_id.clone())
}

fn is_newer_session(candidate: &SessionInfo, current: &SessionInfo) -> bool {
    compare_updated_at(&candidate.updated_at, &current.updated_at)
        .then_with(|| candidate.session_id.cmp(&current.session_id))
        == Ordering::Greater
}

fn compare_updated_at(left: &str, right: &str) -> Ordering {
    match (
        DateTime::parse_from_rfc3339(left).ok(),
        DateTime::parse_from_rfc3339(right).ok(),
    ) {
        (Some(left), Some(right)) => left.with_timezone(&Utc).cmp(&right.with_timezone(&Utc)),
        _ => left.cmp(right),
    }
}

fn has_terminal_hint(session: &SessionInfo) -> bool {
    session
        .process
        .as_ref()
        .is_some_and(|process| process.pid.is_some() || process.parent_pid.is_some())
        || session.window.as_ref().is_some_and(|window| {
            window
                .title
                .as_deref()
                .is_some_and(|title| !title.trim().is_empty())
                || window
                    .terminal
                    .as_deref()
                    .is_some_and(|terminal| !terminal.trim().is_empty())
        })
}

fn is_recent_session(session: &SessionInfo, now: DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(&session.updated_at)
        .ok()
        .map(|updated_at| {
            now.signed_duration_since(updated_at.with_timezone(&Utc))
                .num_seconds()
                <= STALE_SESSION_SECONDS
        })
        .unwrap_or(false)
}

#[cfg(windows)]
fn is_live_hwnd(hwnd: isize) -> bool {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsWindow, IsWindowVisible};

    unsafe {
        let hwnd = hwnd as HWND;
        !hwnd.is_null() && IsWindow(hwnd) != 0 && IsWindowVisible(hwnd) != 0
    }
}

#[cfg(not(windows))]
fn is_live_hwnd(_: isize) -> bool {
    false
}

fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        bytes
    }
}

fn invalid_data(error: impl std::error::Error + Send + Sync + 'static) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_notify_core::{ProcessInfo, ProjectInfo, Severity};

    #[test]
    fn creates_record_only_for_notifiable_events() {
        let task_event = event("n1", "s1", EventType::TaskCompleted);
        let record = notification_record_from_event(&task_event, "2026-05-11T00:00:00Z").unwrap();

        assert_eq!(record.notification_id, "n1");
        assert_eq!(record.session_id, "s1");
        assert_eq!(record.event_type, EventType::TaskCompleted);

        let heartbeat = event("n2", "s1", EventType::Heartbeat);
        assert!(notification_record_from_event(&heartbeat, "2026-05-11T00:00:00Z").is_none());
    }

    #[test]
    fn snapshot_counts_only_live_completed_notifications() {
        let now = DateTime::parse_from_rfc3339("2026-05-11T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let running = session("s1", SessionStatus::Running, "2026-05-11T11:59:00Z");
        let completed = session("s2", SessionStatus::Completed, "2026-05-11T11:58:00Z");
        let failed = session("s3", SessionStatus::Failed, "2026-05-11T11:57:00Z");
        let stale = session("s4", SessionStatus::Completed, "2026-05-11T10:00:00Z");
        let clicked = FloatingNotificationRecord {
            clicked_at: Some("2026-05-11T12:01:00Z".to_string()),
            ..notification("n-clicked", "s2")
        };
        let snapshot = build_floating_status_snapshot(
            vec![running, completed, failed, stale],
            vec![
                notification("n1", "s2"),
                clicked,
                notification("n-failed", "s3"),
                notification("n-stale", "s4"),
            ],
            FloatingBarState::default(),
            now,
        );

        assert_eq!(snapshot.summary.terminal_count, 3);
        assert_eq!(snapshot.summary.running_count, 1);
        assert_eq!(snapshot.summary.completed_count, 1);
        assert_eq!(snapshot.summary.unopened_completed_notifications, 1);
        assert_eq!(snapshot.notifications[0].notification_id, "n1");
        assert_eq!(
            snapshot
                .sessions
                .iter()
                .find(|session| session.session_id == "s2")
                .unwrap()
                .unopened_notification_count,
            1
        );
    }

    #[test]
    fn snapshot_collapses_shared_terminal_to_latest_status() {
        let now = DateTime::parse_from_rfc3339("2026-05-11T12:01:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut older = session("s-old", SessionStatus::Running, "2026-05-11T11:59:00Z");
        older.window.as_mut().unwrap().pid = Some(900);
        let mut latest = session("s-latest", SessionStatus::Completed, "2026-05-11T12:00:00Z");
        latest.window.as_mut().unwrap().pid = Some(900);

        let snapshot = build_floating_status_snapshot(
            vec![older, latest],
            vec![notification("n1", "s-latest")],
            FloatingBarState::default(),
            now,
        );

        assert_eq!(snapshot.summary.terminal_count, 1);
        assert_eq!(snapshot.summary.running_count, 0);
        assert_eq!(snapshot.summary.completed_count, 1);
        assert_eq!(snapshot.summary.unopened_completed_notifications, 1);
        assert_eq!(snapshot.sessions[0].session_id, "s-latest");
        assert_eq!(
            snapshot.sessions[0].related_session_ids,
            vec!["s-latest".to_string(), "s-old".to_string()]
        );
        assert_eq!(snapshot.sessions[0].unopened_notification_count, 1);
    }

    #[test]
    fn snapshot_hides_old_completed_notification_when_terminal_is_running() {
        let now = DateTime::parse_from_rfc3339("2026-05-11T12:01:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut completed = session(
            "s-completed",
            SessionStatus::Completed,
            "2026-05-11T11:59:00Z",
        );
        completed.window.as_mut().unwrap().pid = Some(900);
        let mut running = session("s-running", SessionStatus::Running, "2026-05-11T12:00:00Z");
        running.window.as_mut().unwrap().pid = Some(900);

        let snapshot = build_floating_status_snapshot(
            vec![completed, running],
            vec![notification("n1", "s-completed")],
            FloatingBarState::default(),
            now,
        );

        assert_eq!(snapshot.summary.terminal_count, 1);
        assert_eq!(snapshot.summary.running_count, 1);
        assert_eq!(snapshot.summary.completed_count, 0);
        assert_eq!(snapshot.summary.unopened_completed_notifications, 0);
        assert!(snapshot.notifications.is_empty());
    }

    #[test]
    fn mark_notification_clicked_rejects_missing_or_wrong_session() {
        let mut notifications = HashMap::from([("n1".to_string(), notification("n1", "s1"))]);

        assert_eq!(
            mark_notification_clicked(&mut notifications, "missing", "s1", "now"),
            Err(NotificationClickError::NotFound)
        );
        assert_eq!(
            mark_notification_clicked(&mut notifications, "n1", "s2", "now"),
            Err(NotificationClickError::SessionMismatch)
        );
        assert_eq!(
            mark_notification_clicked(&mut notifications, "n1", "s1", "now"),
            Ok(true)
        );
        assert_eq!(
            mark_notification_clicked(&mut notifications, "n1", "s1", "later"),
            Ok(false)
        );
    }

    #[test]
    fn floating_bar_state_round_trips_with_default_hotkey() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("floating-status-bar.json");
        let state = FloatingBarState {
            hidden: true,
            expanded: true,
            position: Some(FloatingBarPosition { x: 12, y: 34 }),
            hotkey: " ".to_string(),
        };

        save_floating_bar_state(&path, &state).unwrap();
        let loaded = load_floating_bar_state(&path);

        assert!(loaded.hidden);
        assert!(loaded.expanded);
        assert_eq!(loaded.position, Some(FloatingBarPosition { x: 12, y: 34 }));
        assert_eq!(loaded.hotkey, DEFAULT_HOTKEY);
    }

    #[test]
    fn loads_partial_floating_bar_state_without_losing_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("floating-status-bar.json");
        fs::write(&path, r#"{"hidden":true}"#).unwrap();

        let loaded = load_floating_bar_state(&path);

        assert!(loaded.hidden);
        assert!(!loaded.expanded);
        assert_eq!(loaded.position, None);
        assert_eq!(loaded.hotkey, DEFAULT_HOTKEY);
    }

    #[test]
    fn hwnd_zero_uses_best_effort_terminal_hint() {
        let now = DateTime::parse_from_rfc3339("2026-05-11T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut item = session("s1", SessionStatus::Running, "2026-05-11T11:59:00Z");
        item.window.as_mut().unwrap().hwnd = Some(0);

        let snapshot = build_floating_status_snapshot(
            vec![item],
            Vec::new(),
            FloatingBarState::default(),
            now,
        );

        assert_eq!(snapshot.summary.terminal_count, 1);
        assert_eq!(
            snapshot.sessions[0].terminal_liveness,
            TerminalLiveness::BestEffort
        );
    }

    fn event(event_id: &str, session_id: &str, event_type: EventType) -> AgentEvent {
        AgentEvent {
            version: 1,
            event_id: event_id.to_string(),
            event_type,
            severity: Severity::Info,
            tool: "codex".to_string(),
            session_id: session_id.to_string(),
            project: ProjectInfo {
                cwd: r"D:\own\notify".to_string(),
                name: "notify".to_string(),
            },
            process: Some(ProcessInfo {
                pid: Some(42),
                parent_pid: None,
                started_at: None,
            }),
            window: None,
            message: MessageInfo {
                title: "Task completed".to_string(),
                body: "notify".to_string(),
                detail: None,
            },
        }
    }

    fn session(session_id: &str, status: SessionStatus, updated_at: &str) -> SessionInfo {
        SessionInfo {
            session_id: session_id.to_string(),
            tool: "codex".to_string(),
            project: ProjectInfo {
                cwd: r"D:\own\notify".to_string(),
                name: "notify".to_string(),
            },
            status,
            last_event_type: EventType::TaskCompleted,
            last_message: MessageInfo {
                title: "Task completed".to_string(),
                body: "notify".to_string(),
                detail: None,
            },
            process: Some(ProcessInfo {
                pid: Some(42),
                parent_pid: None,
                started_at: None,
            }),
            window: Some(WindowInfo {
                pid: None,
                title: Some("notify".to_string()),
                hwnd: None,
                terminal: Some("Windows Terminal".to_string()),
            }),
            updated_at: updated_at.to_string(),
        }
    }

    fn notification(notification_id: &str, session_id: &str) -> FloatingNotificationRecord {
        FloatingNotificationRecord {
            notification_id: notification_id.to_string(),
            session_id: session_id.to_string(),
            process: None,
            window: None,
            event_type: EventType::TaskCompleted,
            title: "Task completed".to_string(),
            body: "notify".to_string(),
            detail: None,
            created_at: "2026-05-11T11:59:00Z".to_string(),
            clicked_at: None,
        }
    }
}
