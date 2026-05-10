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
    #[cfg(windows)]
    if !ensure_windows_toast_registration() {
        eprintln!(
            "toast registration warning: failed to register Agent Notify Start Menu shortcut"
        );
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
        return show_windows_toast(view);
    }
    #[cfg(not(windows))]
    {
        println!(
            "notification: {} | {} | {}",
            view.title,
            view.body,
            view.detail.as_deref().unwrap_or_default()
        );
        true
    }
}

#[cfg(windows)]
fn show_windows_toast(view: &agent_notify_core::NotificationView) -> bool {
    let body = format!(
        "{}{}{}",
        view.body,
        if view.detail.is_some() { " - " } else { "" },
        view.detail.as_deref().unwrap_or_default()
    );
    let script = r#"
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] > $null

$appName = "Agent Notify"
$app = Get-StartApps | Where-Object { $_.Name -eq $appName } | Select-Object -First 1
if ($null -eq $app) {
    $powerShellApp = Get-StartApps | Where-Object { $_.Name -eq "Windows PowerShell" } | Select-Object -First 1
    if ($null -eq $powerShellApp) {
        throw "Agent Notify is not registered in Start Menu and Windows PowerShell fallback was not found"
    }
    $appId = $powerShellApp.AppID
} else {
    $appId = $app.AppID
}

function Escape-ToastText([string]$value) {
    if ($null -eq $value) {
        return ""
    }
    return [System.Security.SecurityElement]::Escape($value)
}

$title = Escape-ToastText $env:AGENT_NOTIFY_TOAST_TITLE
$body = Escape-ToastText $env:AGENT_NOTIFY_TOAST_BODY
$template = "<toast><visual><binding template=""ToastGeneric""><text>$title</text><text>$body</text></binding></visual></toast>"
$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
$xml.LoadXml($template)
$toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($appId).Show($toast)
"#;
    run_windows_powershell(
        script,
        &[
            ("AGENT_NOTIFY_TOAST_TITLE", view.title.as_str()),
            ("AGENT_NOTIFY_TOAST_BODY", body.as_str()),
        ],
    )
}

#[cfg(windows)]
fn ensure_windows_toast_registration() -> bool {
    let target = match std::env::current_exe() {
        Ok(path) => path.display().to_string(),
        Err(error) => {
            eprintln!("toast registration warning: failed to resolve current executable: {error}");
            return false;
        }
    };
    let script = r#"
$ErrorActionPreference = "Stop"
$target = $env:AGENT_NOTIFY_EXE_PATH
if ([string]::IsNullOrWhiteSpace($target) -or -not (Test-Path -LiteralPath $target)) {
    throw "Agent Notify executable was not found: $target"
}

$shortcutDir = [Environment]::GetFolderPath("Programs")
if ([string]::IsNullOrWhiteSpace($shortcutDir)) {
    throw "Current user Start Menu Programs folder was not found"
}

$shortcutPath = Join-Path $shortcutDir "Agent Notify.lnk"
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = $target
$shortcut.Arguments = "serve"
$shortcut.WorkingDirectory = Split-Path -Parent $target
$shortcut.IconLocation = "$target,0"
$shortcut.Description = "Agent Notify notification backend"
$shortcut.Save()

for ($i = 0; $i -lt 10; $i++) {
    $app = Get-StartApps | Where-Object { $_.Name -eq "Agent Notify" } | Select-Object -First 1
    if ($null -ne $app) {
        return
    }
    Start-Sleep -Milliseconds 200
}

throw "Agent Notify Start Menu shortcut was created, but Windows did not return an AppID"
"#;
    run_windows_powershell(script, &[("AGENT_NOTIFY_EXE_PATH", target.as_str())])
}

#[cfg(windows)]
fn run_windows_powershell(script: &str, envs: &[(&str, &str)]) -> bool {
    use std::process::Command;

    let mut command = Command::new("powershell");
    command.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ]);
    for (key, value) in envs {
        command.env(key, value);
    }

    match command.output() {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            eprintln!(
                "toast powershell warning: exit={} stderr={} stdout={}",
                output
                    .status
                    .code()
                    .map_or_else(|| "terminated".to_string(), |code| code.to_string()),
                stderr.trim(),
                stdout.trim()
            );
            false
        }
        Err(error) => {
            eprintln!("toast powershell warning: failed to start PowerShell: {error}");
            false
        }
    }
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
