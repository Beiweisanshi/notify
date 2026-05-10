pub mod adapters;
pub mod config;
pub mod dedupe;
pub mod event;
pub mod notification;
pub mod redact;

pub use adapters::{HookContext, build_event_from_hook};
pub use config::{
    DEFAULT_HOST, DEFAULT_PORT, RuntimeConfig, agent_notify_dir, config_path,
    load_or_create_config, read_token, token_path,
};
pub use dedupe::DedupeCache;
pub use event::{
    AgentEvent, EventType, MessageInfo, ProcessInfo, ProjectInfo, SessionInfo, SessionStatus,
    Severity, WindowInfo,
};
pub use notification::{NotificationDecision, NotificationView, notification_view, should_notify};
