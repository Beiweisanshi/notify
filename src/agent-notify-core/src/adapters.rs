use crate::event::{
    AgentEvent, EventType, MessageInfo, ProcessInfo, ProjectInfo, Severity, WindowInfo,
};
use crate::redact::{safe_detail_for_tool, sanitize_summary, stable_event_id, summary_hash};
use serde_json::Value;
use std::env;

#[derive(Debug, Clone, Default)]
pub struct HookContext {
    pub tool: Option<String>,
    pub hook_event: Option<String>,
    pub explicit_event_type: Option<EventType>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub project: Option<String>,
    pub window_title: Option<String>,
    pub hwnd: Option<isize>,
    pub terminal: Option<String>,
    pub pid: Option<u32>,
    pub parent_pid: Option<u32>,
}

impl HookContext {
    pub fn with_env(mut self) -> Self {
        self.tool = self.tool.or_else(|| env::var("AGENT_NOTIFY_TOOL").ok());
        self.session_id = self
            .session_id
            .or_else(|| env::var("AGENT_NOTIFY_SESSION_ID").ok());
        self.cwd = self
            .cwd
            .or_else(|| env::var("AGENT_NOTIFY_CWD").ok())
            .or_else(|| {
                env::current_dir()
                    .ok()
                    .map(|path| path.display().to_string())
            });
        self.project = self
            .project
            .or_else(|| env::var("AGENT_NOTIFY_PROJECT").ok());
        self.window_title = self
            .window_title
            .or_else(|| env::var("AGENT_NOTIFY_WINDOW_TITLE").ok());
        self.terminal = self
            .terminal
            .or_else(|| env::var("AGENT_NOTIFY_TERMINAL").ok());
        self.pid = self.pid.or_else(|| {
            env::var("AGENT_NOTIFY_PID")
                .ok()
                .and_then(|v| v.parse().ok())
        });
        self.parent_pid = self.parent_pid.or_else(|| {
            env::var("AGENT_NOTIFY_PARENT_PID")
                .ok()
                .and_then(|v| v.parse().ok())
        });
        self
    }
}

pub fn build_event_from_hook(
    payload: &Value,
    context: HookContext,
) -> Result<AgentEvent, AdapterError> {
    let context = context.with_env();
    let tool = context
        .tool
        .clone()
        .or_else(|| text_field(payload, &["tool", "source", "app"]))
        .unwrap_or_else(|| "unknown".to_string())
        .to_ascii_lowercase();
    let hook_event = context
        .hook_event
        .clone()
        .or_else(|| {
            text_field(
                payload,
                &["hook_event_name", "hookEvent", "event", "eventName"],
            )
        })
        .unwrap_or_else(|| "Notification".to_string());
    let event_type = context
        .explicit_event_type
        .unwrap_or_else(|| map_hook_event(&tool, &hook_event, payload));
    let severity = severity_for(event_type);
    let cwd = context
        .cwd
        .clone()
        .or_else(|| {
            text_field(
                payload,
                &["cwd", "workspace", "project_path", "projectPath"],
            )
        })
        .unwrap_or_else(|| ".".to_string());
    let mut project = ProjectInfo::from_cwd(cwd);
    if let Some(name) = context
        .project
        .clone()
        .or_else(|| text_field(payload, &["project", "project_name", "projectName"]))
    {
        project.name = sanitize_project_name(&name);
    }
    let session_id = context
        .session_id
        .clone()
        .or_else(|| {
            text_field(
                payload,
                &[
                    "session_id",
                    "sessionId",
                    "conversation_id",
                    "conversationId",
                    "thread_id",
                    "threadId",
                ],
            )
        })
        .unwrap_or_else(|| format!("{}-{}", tool, &summary_hash(&[&project.cwd])[..12]));

    let tool_name = text_field(
        payload,
        &[
            "tool_name",
            "toolName",
            "requested_tool",
            "requestedTool",
            "name",
        ],
    );
    let raw_summary = text_field(
        payload,
        &[
            "message",
            "summary",
            "reason",
            "statusMessage",
            "error",
            "permission",
            "prompt",
        ],
    )
    .unwrap_or_else(|| hook_event.clone());
    let detail = safe_detail_for_tool(tool_name.as_deref(), &raw_summary);
    let title = title_for(&tool, event_type);
    let body = body_for(&project.name, &session_id, event_type);
    let stable_payload_id =
        text_field(payload, &["event_id", "eventId", "id", "call_id", "callId"])
            .unwrap_or_else(|| summary_hash(&[&sanitize_summary(payload.to_string())]));
    let event_id = stable_event_id(&[
        &tool,
        &session_id,
        &hook_event,
        &stable_payload_id,
        &summary_hash(&[&detail]),
    ]);

    Ok(AgentEvent {
        version: 1,
        event_id,
        event_type,
        severity,
        tool,
        session_id,
        project,
        process: Some(ProcessInfo {
            pid: context
                .pid
                .or_else(|| u32_field(payload, &["pid", "process_id", "processId"])),
            parent_pid: context.parent_pid.or_else(|| {
                u32_field(
                    payload,
                    &["parent_pid", "parentPid", "ppid", "parent_process_id"],
                )
            }),
            started_at: text_field(payload, &["process_started_at", "processStartedAt"]),
        }),
        window: Some(WindowInfo {
            title: context
                .window_title
                .or_else(|| text_field(payload, &["window_title", "windowTitle", "title"])),
            hwnd: context
                .hwnd
                .or_else(|| i64_field(payload, &["hwnd"]).map(|v| v as isize)),
            terminal: context
                .terminal
                .or_else(|| text_field(payload, &["terminal", "terminalName"])),
        }),
        message: MessageInfo {
            title,
            body,
            detail: Some(detail),
        },
    })
}

fn map_hook_event(tool: &str, hook_event: &str, payload: &Value) -> EventType {
    let normalized = hook_event.to_ascii_lowercase();
    match (tool, normalized.as_str()) {
        ("claude", "permissionrequest") => EventType::UserConfirmationRequired,
        ("claude", "notification") => {
            if contains_any(
                payload,
                &["permission", "approve", "confirm", "confirmation"],
            ) {
                EventType::UserConfirmationRequired
            } else {
                EventType::UserInputRequired
            }
        }
        ("claude", "stop") => {
            if contains_any(payload, &["input", "continue", "max_turns", "waiting"]) {
                EventType::UserInputRequired
            } else {
                EventType::TaskCompleted
            }
        }
        ("claude", "stopfailure") => EventType::TaskFailed,
        ("claude", "posttoolusefailure") => {
            if contains_any(payload, &["permission", "denied", "blocked", "sandbox"]) {
                EventType::ToolBlocked
            } else {
                EventType::TaskFailed
            }
        }
        ("claude", "sessionend") => {
            if exit_code(payload).is_some_and(|code| code != 0)
                || contains_any(payload, &["failed", "error"])
            {
                EventType::TaskFailed
            } else {
                EventType::TaskCompleted
            }
        }
        ("codex", "sessionstart") => EventType::TaskStarted,
        ("codex", "permissionrequest") => EventType::UserConfirmationRequired,
        ("codex", "stop") => {
            if contains_any(payload, &["input", "continue", "waiting"]) {
                EventType::UserInputRequired
            } else if exit_code(payload).is_some_and(|code| code != 0) {
                EventType::TaskFailed
            } else {
                EventType::TaskCompleted
            }
        }
        ("codex", "posttooluse") => {
            if exit_code(payload).is_some_and(|code| code != 0)
                || contains_any(payload, &["blocked", "denied", "sandbox"])
            {
                EventType::ToolBlocked
            } else {
                EventType::Heartbeat
            }
        }
        _ => EventType::UserInputRequired,
    }
}

fn severity_for(event_type: EventType) -> Severity {
    match event_type {
        EventType::TaskFailed | EventType::ToolBlocked => Severity::Error,
        EventType::UserConfirmationRequired | EventType::UserInputRequired => Severity::Warning,
        EventType::TaskStarted | EventType::TaskCompleted | EventType::Heartbeat => Severity::Info,
    }
}

fn title_for(tool: &str, event_type: EventType) -> String {
    let name = match tool {
        "claude" => "Claude",
        "codex" => "Codex",
        other => other,
    };
    let suffix = match event_type {
        EventType::TaskStarted => "任务开始",
        EventType::TaskCompleted => "任务完成",
        EventType::TaskFailed => "执行失败",
        EventType::UserConfirmationRequired => "需要确认",
        EventType::UserInputRequired => "等待输入",
        EventType::ToolBlocked => "执行受阻",
        EventType::Heartbeat => "状态更新",
    };
    format!("{name} {suffix}")
}

fn body_for(project_name: &str, session_id: &str, event_type: EventType) -> String {
    let action = match event_type {
        EventType::TaskCompleted => "点击查看结果",
        EventType::UserInputRequired => "需要你的下一步",
        EventType::TaskStarted | EventType::Heartbeat => "状态已更新",
        _ => "点击返回终端",
    };
    format!("{project_name} · {} · {action}", short_session(session_id))
}

fn short_session(session_id: &str) -> String {
    if session_id.len() <= 20 {
        session_id.to_string()
    } else {
        format!("{}…", &session_id[..19])
    }
}

fn sanitize_project_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        "project".to_string()
    } else {
        trimmed.replace(['\\', '/', ':'], "_")
    }
}

fn contains_any(payload: &Value, needles: &[&str]) -> bool {
    let haystack = payload.to_string().to_ascii_lowercase();
    needles.iter().any(|needle| haystack.contains(needle))
}

fn exit_code(payload: &Value) -> Option<i64> {
    i64_field(
        payload,
        &["exit_code", "exitCode", "status_code", "statusCode"],
    )
}

fn text_field(payload: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| payload.pointer(&format!("/{key}")).and_then(Value::as_str))
        .map(ToString::to_string)
        .or_else(|| find_nested_string(payload, keys))
}

fn find_nested_string(payload: &Value, keys: &[&str]) -> Option<String> {
    match payload {
        Value::Object(map) => {
            for (key, value) in map {
                if keys
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(key))
                    && let Some(value) = value.as_str()
                {
                    return Some(value.to_string());
                }
                if let Some(found) = find_nested_string(value, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(values) => values
            .iter()
            .find_map(|value| find_nested_string(value, keys)),
        _ => None,
    }
}

fn u32_field(payload: &Value, keys: &[&str]) -> Option<u32> {
    i64_field(payload, keys).and_then(|value| u32::try_from(value).ok())
}

fn i64_field(payload: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| payload.pointer(&format!("/{key}")).and_then(Value::as_i64))
        .or_else(|| find_nested_i64(payload, keys))
}

fn find_nested_i64(payload: &Value, keys: &[&str]) -> Option<i64> {
    match payload {
        Value::Object(map) => {
            for (key, value) in map {
                if keys
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(key))
                    && let Some(value) = value.as_i64()
                {
                    return Some(value);
                }
                if let Some(found) = find_nested_i64(value, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(values) => values.iter().find_map(|value| find_nested_i64(value, keys)),
        _ => None,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("failed to build event")]
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_claude_hook_payload_as_confirmation_required() {
        let event = build_event_from_hook(
            &json!({
                "session_id": "claude-backend",
                "cwd": "D:\\repo\\backend",
                "tool_name": "Bash",
                "message": "run npm test -- --token=secret"
            }),
            HookContext {
                tool: Some("claude".to_string()),
                hook_event: Some("PermissionRequest".to_string()),
                ..HookContext::default()
            },
        )
        .unwrap();

        assert_eq!(event.event_type, EventType::UserConfirmationRequired);
        assert_eq!(event.message.title, "Claude 需要确认");
        assert!(!event.message.detail.unwrap().contains("secret"));
    }

    #[test]
    fn parses_codex_stop_payload_as_completed() {
        let event = build_event_from_hook(
            &json!({
                "sessionId": "codex-project",
                "cwd": "D:\\repo\\project",
                "exitCode": 0
            }),
            HookContext {
                tool: Some("codex".to_string()),
                hook_event: Some("Stop".to_string()),
                ..HookContext::default()
            },
        )
        .unwrap();

        assert_eq!(event.event_type, EventType::TaskCompleted);
        assert_eq!(event.severity, Severity::Info);
    }

    #[test]
    fn uses_stable_event_id_for_same_payload() {
        let payload = json!({"sessionId": "s1", "cwd": "D:\\repo", "id": "abc"});
        let context = HookContext {
            tool: Some("codex".to_string()),
            hook_event: Some("PermissionRequest".to_string()),
            ..HookContext::default()
        };
        let first = build_event_from_hook(&payload, context.clone()).unwrap();
        let second = build_event_from_hook(&payload, context).unwrap();
        assert_eq!(first.event_id, second.event_id);
    }
}
