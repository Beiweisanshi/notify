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

#[cfg(windows)]
const WINDOWS_APP_NAME: &str = "智能任务通知";
#[cfg(windows)]
const WINDOWS_LEGACY_APP_NAME: &str = "Agent Notify";
#[cfg(windows)]
const WINDOWS_ICON_PNG: &[u8] = include_bytes!("../../../assets/agent-notify-icon.png");
#[cfg(windows)]
const WINDOWS_ICON_ICO: &[u8] = include_bytes!("../../../assets/agent-notify-icon.ico");

#[cfg(windows)]
#[derive(Debug, Clone)]
struct WindowsIconAssets {
    png: String,
    ico: String,
}

#[cfg(windows)]
static WINDOWS_ICON_ASSETS: std::sync::OnceLock<Option<WindowsIconAssets>> =
    std::sync::OnceLock::new();

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
    if !ensure_windows_toast_registration(windows_icon_assets().map(|assets| assets.ico.as_str())) {
        eprintln!("toast registration warning: failed to register Windows Start Menu shortcut");
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
        if view.detail.is_some() { " · " } else { "" },
        view.detail.as_deref().unwrap_or_default()
    );
    let icon_path = windows_icon_assets()
        .map(|assets| assets.png.as_str())
        .unwrap_or_default();
    let script = r#"
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] > $null

$appName = $env:AGENT_NOTIFY_WINDOWS_APP_NAME
$legacyAppName = $env:AGENT_NOTIFY_WINDOWS_LEGACY_APP_NAME
$app = Get-StartApps | Where-Object { $_.Name -eq $appName } | Select-Object -First 1
if ($null -eq $app -and -not [string]::IsNullOrWhiteSpace($legacyAppName)) {
    $app = Get-StartApps | Where-Object { $_.Name -eq $legacyAppName } | Select-Object -First 1
}
if ($null -eq $app) {
    $powerShellApp = Get-StartApps | Where-Object { $_.Name -eq "Windows PowerShell" } | Select-Object -First 1
    if ($null -eq $powerShellApp) {
        throw "$appName is not registered in Start Menu and Windows PowerShell fallback was not found"
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
$iconXml = ""
$iconPath = $env:AGENT_NOTIFY_TOAST_ICON
if (-not [string]::IsNullOrWhiteSpace($iconPath) -and (Test-Path -LiteralPath $iconPath)) {
    $iconUri = Escape-ToastText ([Uri]::new($iconPath).AbsoluteUri)
    $iconXml = "<image placement=""appLogoOverride"" hint-crop=""circle"" src=""$iconUri""/>"
}
$template = "<toast><visual><binding template=""ToastGeneric"">$iconXml<text>$title</text><text>$body</text></binding></visual></toast>"
$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
$xml.LoadXml($template)
$toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($appId).Show($toast)
"#;
    run_windows_powershell(
        script,
        &[
            ("AGENT_NOTIFY_WINDOWS_APP_NAME", WINDOWS_APP_NAME),
            (
                "AGENT_NOTIFY_WINDOWS_LEGACY_APP_NAME",
                WINDOWS_LEGACY_APP_NAME,
            ),
            ("AGENT_NOTIFY_TOAST_TITLE", view.title.as_str()),
            ("AGENT_NOTIFY_TOAST_BODY", body.as_str()),
            ("AGENT_NOTIFY_TOAST_ICON", icon_path),
        ],
    )
}

#[cfg(windows)]
fn windows_icon_assets() -> Option<&'static WindowsIconAssets> {
    WINDOWS_ICON_ASSETS
        .get_or_init(ensure_windows_icon_assets)
        .as_ref()
}

#[cfg(windows)]
fn ensure_windows_icon_assets() -> Option<WindowsIconAssets> {
    use std::fs;

    let assets_dir = agent_notify_core::agent_notify_dir().join("assets");
    if let Err(error) = fs::create_dir_all(&assets_dir) {
        eprintln!("toast icon warning: failed to create icon assets directory: {error}");
        return None;
    }

    let png_path = assets_dir.join("agent-notify-icon.png");
    let ico_path = assets_dir.join("agent-notify-icon.ico");
    if let Err(error) = write_embedded_asset(&png_path, WINDOWS_ICON_PNG) {
        eprintln!("toast icon warning: failed to write png icon asset: {error}");
        return None;
    }
    if let Err(error) = write_embedded_asset(&ico_path, WINDOWS_ICON_ICO) {
        eprintln!("toast icon warning: failed to write ico icon asset: {error}");
        return None;
    }

    Some(WindowsIconAssets {
        png: png_path.display().to_string(),
        ico: ico_path.display().to_string(),
    })
}

#[cfg(windows)]
fn write_embedded_asset(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::fs;

    let is_current = fs::read(path).is_ok_and(|current| current == bytes);
    if !is_current {
        fs::write(path, bytes)?;
    }
    Ok(())
}

#[cfg(windows)]
fn ensure_windows_toast_registration(icon_path: Option<&str>) -> bool {
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
$appName = $env:AGENT_NOTIFY_WINDOWS_APP_NAME
$legacyAppName = $env:AGENT_NOTIFY_WINDOWS_LEGACY_APP_NAME
$iconPath = $env:AGENT_NOTIFY_SHORTCUT_ICON
if ([string]::IsNullOrWhiteSpace($target) -or -not (Test-Path -LiteralPath $target)) {
    throw "$appName executable was not found: $target"
}

$shortcutDir = [Environment]::GetFolderPath("Programs")
if ([string]::IsNullOrWhiteSpace($shortcutDir)) {
    throw "Current user Start Menu Programs folder was not found"
}

$shortcutPath = Join-Path $shortcutDir "$appName.lnk"
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = $target
$shortcut.Arguments = "serve"
$shortcut.WorkingDirectory = Split-Path -Parent $target
if (-not [string]::IsNullOrWhiteSpace($iconPath) -and (Test-Path -LiteralPath $iconPath)) {
    $shortcut.IconLocation = $iconPath
} else {
    $shortcut.IconLocation = "$target,0"
}
$shortcut.Description = "本地 AI 任务通知后台"
$shortcut.Save()

if (-not [string]::IsNullOrWhiteSpace($legacyAppName) -and $legacyAppName -ne $appName) {
    $legacyShortcutPath = Join-Path $shortcutDir "$legacyAppName.lnk"
    if (Test-Path -LiteralPath $legacyShortcutPath) {
        Remove-Item -LiteralPath $legacyShortcutPath -Force -ErrorAction SilentlyContinue
    }
}

for ($i = 0; $i -lt 10; $i++) {
    $app = Get-StartApps | Where-Object { $_.Name -eq $appName } | Select-Object -First 1
    if ($null -ne $app) {
        return
    }
    Start-Sleep -Milliseconds 200
}

throw "$appName Start Menu shortcut was created, but Windows did not return an AppID"
"#;
    run_windows_powershell(
        script,
        &[
            ("AGENT_NOTIFY_EXE_PATH", target.as_str()),
            ("AGENT_NOTIFY_WINDOWS_APP_NAME", WINDOWS_APP_NAME),
            (
                "AGENT_NOTIFY_WINDOWS_LEGACY_APP_NAME",
                WINDOWS_LEGACY_APP_NAME,
            ),
            ("AGENT_NOTIFY_SHORTCUT_ICON", icon_path.unwrap_or_default()),
        ],
    )
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
