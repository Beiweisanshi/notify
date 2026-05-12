#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use agent_notify_core::{load_or_create_config, read_token};
use anyhow::{Context, Result};
use reqwest::{Client, Method, Response};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

const BACKEND_TIMEOUT: Duration = Duration::from_millis(1500);
const BACKEND_START_COOLDOWN: Duration = Duration::from_secs(10);

static BACKEND_START_STATE: OnceLock<Mutex<BackendStartState>> = OnceLock::new();

#[derive(Debug, Default)]
struct BackendStartState {
    last_attempt: Option<Instant>,
}

#[tauri::command]
async fn get_floating_status() -> Result<Value, String> {
    request_json(Method::GET, "/floating-status", None).await
}

#[tauri::command]
async fn put_floating_status_state(state: Value) -> Result<Value, String> {
    request_json(Method::PUT, "/floating-status/state", Some(state)).await
}

#[tauri::command]
async fn floating_ui_heartbeat() -> Result<(), String> {
    request_empty(Method::POST, "/floating-status/ui-heartbeat", None).await
}

#[tauri::command]
async fn focus_session(
    session_id: String,
    notification_id: Option<String>,
) -> Result<Value, String> {
    allow_backend_foreground_activation();
    let body = notification_id.map(|notification_id| json!({ "notificationId": notification_id }));
    request_json(
        Method::POST,
        &format!("/focus/{}", encode_path_segment(&session_id)),
        body,
    )
    .await
}

#[tauri::command]
async fn dismiss_notification(
    notification_id: String,
    session_id: String,
) -> Result<Value, String> {
    request_json(
        Method::POST,
        &format!(
            "/floating-status/notifications/{}/dismiss",
            encode_path_segment(&notification_id)
        ),
        Some(json!({ "sessionId": session_id })),
    )
    .await
}

#[tauri::command]
async fn dismiss_notifications(session_id: Option<String>) -> Result<Value, String> {
    request_json(
        Method::POST,
        "/floating-status/notifications/dismiss-all",
        Some(json!({ "sessionId": session_id })),
    )
    .await
}

async fn request_json(method: Method, path: &str, body: Option<Value>) -> Result<Value, String> {
    let response = request_backend(method, path, body).await?;
    response
        .json::<Value>()
        .await
        .map_err(|error| format!("invalid backend json: {error}"))
}

async fn request_empty(method: Method, path: &str, body: Option<Value>) -> Result<(), String> {
    request_backend(method, path, body).await?;
    Ok(())
}

async fn request_backend(
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Response, String> {
    let first = send_backend_request(method.clone(), path, body.clone()).await;
    let response = match first {
        Ok(response) => response,
        Err(first_error) => {
            start_backend_process().map_err(|error| {
                format!("backend unavailable: {first_error}; failed to start backend: {error:#}")
            })?;
            tokio::time::sleep(Duration::from_millis(900)).await;
            send_backend_request(method, path, body)
                .await
                .map_err(|error| format!("backend unavailable after start: {error}"))?
        }
    };
    let status = response.status();
    if status.is_success() {
        Ok(response)
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(format!("backend rejected {path}: {status} {text}"))
    }
}

async fn send_backend_request(method: Method, path: &str, body: Option<Value>) -> Result<Response> {
    let config = load_or_create_config().context("failed to load runtime config")?;
    let token = read_token(&config).context("failed to read auth token")?;
    let endpoint = config.endpoint(path);
    let client = Client::builder()
        .timeout(BACKEND_TIMEOUT)
        .build()
        .context("failed to create backend client")?;
    let mut request = client.request(method, endpoint).bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    request.send().await.context("failed to contact backend")
}

fn start_backend_process() -> Result<()> {
    {
        let mut state = BACKEND_START_STATE
            .get_or_init(|| Mutex::new(BackendStartState::default()))
            .lock()
            .expect("backend start state poisoned");
        let now = Instant::now();
        if state
            .last_attempt
            .is_some_and(|last_attempt| now.duration_since(last_attempt) < BACKEND_START_COOLDOWN)
        {
            return Ok(());
        }
        state.last_attempt = Some(now);
    }

    let repo_root = repo_root();
    let mut command = if let Some(exe) = backend_executable_candidate()? {
        let mut command = Command::new(exe);
        command.args(["serve"]);
        command
    } else {
        let mut command = Command::new("cargo");
        command.args([
            "run",
            "--ignore-rust-version",
            "-p",
            "agent-notify-tray",
            "--",
            "serve",
        ]);
        command.current_dir(&repo_root);
        command
    };
    command
        .env("AGENT_NOTIFY_DESKTOP_PID", std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_console_window(&mut command);
    command.spawn().context("failed to spawn backend")?;
    Ok(())
}

fn backend_executable_candidate() -> Result<Option<PathBuf>> {
    let exe_name = if cfg!(windows) {
        "agent-notify-tray.exe"
    } else {
        "agent-notify-tray"
    };
    let current_exe = std::env::current_exe().context("failed to resolve current executable")?;
    let sibling = current_exe.with_file_name(exe_name);
    if sibling.exists() {
        return Ok(Some(sibling));
    }
    let workspace_target = repo_root().join("target").join("debug").join(exe_name);
    if workspace_target.exists() {
        return Ok(Some(workspace_target));
    }
    Ok(None)
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(windows)]
fn hide_console_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console_window(_: &mut Command) {}

#[cfg(windows)]
fn allow_backend_foreground_activation() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};

    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }
}

#[cfg(not(windows))]
fn allow_backend_foreground_activation() {}

fn encode_path_segment(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => {
                let encoded = format!("%{byte:02X}");
                encoded.chars().collect()
            }
        })
        .collect()
}

fn main() {
    let shortcut = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space);
    let shortcut_for_handler = shortcut;
    let global_shortcut = tauri_plugin_global_shortcut::Builder::new()
        .with_handler(move |app, observed_shortcut, event| {
            if observed_shortcut == &shortcut_for_handler && event.state() == ShortcutState::Pressed
            {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                    let _ = window.emit("floating-hotkey", ());
                }
            }
        })
        .build();

    tauri::Builder::default()
        .plugin(global_shortcut)
        .setup(move |app| {
            app.global_shortcut().register(shortcut)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_floating_status,
            put_floating_status_state,
            floating_ui_heartbeat,
            focus_session,
            dismiss_notification,
            dismiss_notifications
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Agent Notify desktop");
}
