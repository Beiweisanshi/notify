use agent_notify_core::{
    AgentEvent, DedupeCache, RuntimeConfig, SessionInfo, load_or_create_config, notification_view,
    read_token,
};
use anyhow::{Context, Result};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use clap::{Parser, Subcommand};
use hook_manager::{HookInstallPaths, install_or_repair};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Debug, Parser)]
#[command(name = "agent-notify-tray")]
#[command(about = "Local AgentNotify backend. Tauri can host the same core flow later.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Serve,
    CheckHooks,
    RepairHooks,
}

#[derive(Clone)]
struct AppState {
    token: Arc<String>,
    listener_enabled: Arc<Mutex<bool>>,
    sessions: Arc<Mutex<HashMap<String, SessionInfo>>>,
    dedupe: Arc<Mutex<DedupeCache>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventResponse {
    accepted: bool,
    notified: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FocusResponse {
    focused: bool,
    opened_session_detail: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Commands::Serve) {
        Commands::Serve => serve().await,
        Commands::CheckHooks | Commands::RepairHooks => {
            let paths = HookInstallPaths::from_env();
            let report = install_or_repair(&paths)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
    }
}

async fn serve() -> Result<()> {
    let config = load_or_create_config().context("failed to load runtime config")?;
    if config.hooks.auto_check && config.hooks.auto_install {
        let paths = HookInstallPaths::from_env();
        if let Err(error) = install_or_repair(&paths) {
            eprintln!("hook manager warning: {error}");
        }
    }

    let token = read_token(&config).context("failed to read auth token")?;
    let state = build_state(&config, token);
    let app = Router::new()
        .route("/events", post(post_event))
        .route("/sessions", get(get_sessions))
        .route("/focus/{session_id}", post(focus_session))
        .with_state(state);
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port)
        .parse()
        .context("invalid listen address")?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("agent-notify-tray listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn build_state(config: &RuntimeConfig, token: String) -> AppState {
    AppState {
        token: Arc::new(token),
        listener_enabled: Arc::new(Mutex::new(config.notifications.listener_enabled)),
        sessions: Arc::new(Mutex::new(HashMap::new())),
        dedupe: Arc::new(Mutex::new(DedupeCache::new(Duration::from_secs(
            config.notifications.dedupe_seconds,
        )))),
    }
}

async fn post_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(event): Json<AgentEvent>,
) -> Result<Json<EventResponse>, StatusCode> {
    authorize(&headers, &state)?;
    event.validate().map_err(|_| StatusCode::BAD_REQUEST)?;
    if !*state.listener_enabled.lock().await {
        return Ok(Json(EventResponse {
            accepted: false,
            notified: false,
        }));
    }

    let now = Utc::now().to_rfc3339();
    state.sessions.lock().await.insert(
        event.session_id.clone(),
        SessionInfo::from_event(&event, now),
    );
    let should_emit = state
        .dedupe
        .lock()
        .await
        .should_emit(&event, Instant::now());
    let notified = should_emit
        && notification_view(&event)
            .map(|view| show_notification(&event, &view))
            .unwrap_or(false);

    Ok(Json(EventResponse {
        accepted: true,
        notified,
    }))
}

async fn get_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<SessionInfo>>, StatusCode> {
    authorize(&headers, &state)?;
    Ok(Json(
        state.sessions.lock().await.values().cloned().collect(),
    ))
}

async fn focus_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<FocusResponse>, StatusCode> {
    authorize(&headers, &state)?;
    let session = state.sessions.lock().await.get(&session_id).cloned();
    let focused = session.as_ref().is_some_and(focus_window);
    Ok(Json(FocusResponse {
        focused,
        opened_session_detail: !focused,
    }))
}

fn authorize(headers: &HeaderMap, state: &AppState) -> Result<(), StatusCode> {
    let expected = format!("Bearer {}", state.token.as_str());
    let authorized = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected);
    authorized.then_some(()).ok_or(StatusCode::UNAUTHORIZED)
}

fn show_notification(_event: &AgentEvent, view: &agent_notify_core::NotificationView) -> bool {
    #[cfg(windows)]
    {
        show_windows_toast(view);
    }
    #[cfg(not(windows))]
    {
        println!(
            "notification: {} | {} | {}",
            view.title,
            view.body,
            view.detail.as_deref().unwrap_or_default()
        );
    }
    true
}

#[cfg(windows)]
fn show_windows_toast(view: &agent_notify_core::NotificationView) {
    use std::process::Command;
    let title = ps_escape(&view.title);
    let body = ps_escape(&format!(
        "{}{}{}",
        view.body,
        if view.detail.is_some() { " - " } else { "" },
        view.detail.as_deref().unwrap_or_default()
    ));
    let script = format!(
        r#"
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] > $null
$template = @"
<toast><visual><binding template="ToastGeneric"><text>{title}</text><text>{body}</text></binding></visual></toast>
"@
$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
$xml.LoadXml($template)
$toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier("AgentNotify").Show($toast)
"#
    );
    let _ = Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .status();
}

#[cfg(windows)]
fn ps_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn focus_window(session: &SessionInfo) -> bool {
    #[cfg(windows)]
    {
        if let Some(hwnd) = session.window.as_ref().and_then(|window| window.hwnd)
            && focus_hwnd(hwnd)
        {
            return true;
        }
    }
    false
}

#[cfg(windows)]
fn focus_hwnd(hwnd: isize) -> bool {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsWindow, IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };
    unsafe {
        let hwnd = hwnd as HWND;
        if IsWindow(hwnd) == 0 || IsWindowVisible(hwnd) == 0 {
            return false;
        }
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd) != 0
    }
}
