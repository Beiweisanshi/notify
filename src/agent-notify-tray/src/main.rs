mod floating_status;
mod focus;

use agent_notify_core::{
    AgentEvent, DedupeCache, RuntimeConfig, SessionInfo, WindowInfo, load_or_create_config,
    notification_view, read_token,
};
use anyhow::{Context, Result};
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::Utc;
use clap::{Parser, Subcommand};
use floating_status::{
    FloatingBarState, FloatingNotificationRecord, FloatingStatusSnapshot, NotificationClickError,
    build_floating_status_snapshot, floating_bar_state_path, load_floating_bar_state,
    mark_notification_clicked, notification_record_from_event, save_floating_bar_state,
};
use hook_manager::{HookInstallPaths, install_or_repair};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

#[cfg(windows)]
const WINDOWS_APP_NAME: &str = "智能任务通知";
#[cfg(windows)]
const WINDOWS_LEGACY_APP_NAME: &str = "Agent Notify";
#[cfg(windows)]
const WINDOWS_URI_SCHEME: &str = "agent-notify";
#[cfg(windows)]
const WINDOWS_ICON_PNG: &[u8] = include_bytes!("../../../assets/agent-notify-icon.png");
#[cfg(windows)]
const WINDOWS_ICON_ICO: &[u8] = include_bytes!("../../../assets/agent-notify-icon.ico");
#[cfg(windows)]
const WINDOWS_TRAY_ICON_ID: u32 = 1;
#[cfg(windows)]
const WINDOWS_TRAY_MESSAGE: u32 = 0x8000 + 1;
#[cfg(windows)]
const WINDOWS_TRAY_TOGGLE_BAR_ID: usize = 1001;
#[cfg(windows)]
const WINDOWS_TRAY_OPEN_LOGS_ID: usize = 1002;
#[cfg(windows)]
const WINDOWS_TRAY_EXIT_ID: usize = 1003;
const ACTIVATION_TTL: Duration = Duration::from_secs(600);
const FLOATING_UI_ACTIVE_TTL: Duration = Duration::from_secs(6);

#[cfg(windows)]
#[derive(Debug, Clone)]
struct WindowsIconAssets {
    png: String,
    ico: String,
}

#[cfg(windows)]
static WINDOWS_ICON_ASSETS: std::sync::OnceLock<Option<WindowsIconAssets>> =
    std::sync::OnceLock::new();
#[cfg(windows)]
static WINDOWS_TRAY_SHUTDOWN: std::sync::OnceLock<Arc<Notify>> = std::sync::OnceLock::new();
#[cfg(windows)]
static WINDOWS_TRAY_BAR_STATE: std::sync::OnceLock<(
    Arc<Mutex<FloatingBarState>>,
    Arc<PathBuf>,
)> = std::sync::OnceLock::new();

#[derive(Debug, Parser)]
#[command(name = "agent-notify-tray")]
#[command(about = "Local AgentNotify backend. Tauri can host the same core flow later.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Serve(ServeArgs),
    Activate(ActivateArgs),
    CheckHooks,
    RepairHooks,
}

#[derive(Debug, Default, Parser)]
struct ServeArgs {
    /// Run only the localhost backend; Tauri owns the visible desktop UI.
    #[arg(long)]
    no_native_tray: bool,
}

#[derive(Debug, Parser)]
struct ActivateArgs {
    #[arg(long)]
    home: Option<PathBuf>,
    uri: String,
}

#[derive(Clone)]
struct AppState {
    token: Arc<String>,
    listener_enabled: Arc<Mutex<bool>>,
    sessions: Arc<Mutex<HashMap<String, SessionInfo>>>,
    notifications: Arc<Mutex<HashMap<String, FloatingNotificationRecord>>>,
    floating_bar_state: Arc<Mutex<FloatingBarState>>,
    floating_bar_state_path: Arc<PathBuf>,
    floating_ui_heartbeat: Arc<Mutex<Option<Instant>>>,
    dedupe: Arc<Mutex<DedupeCache>>,
    activations: Arc<Mutex<HashMap<String, ActivationTarget>>>,
}

#[derive(Debug, Clone)]
struct ActivationTarget {
    session_id: String,
    notification_id: String,
    session: SessionInfo,
    expires_at: Instant,
}

#[derive(Debug, Clone)]
struct ActivationRegistration {
    id: String,
    uri: String,
}

#[derive(Debug, Clone)]
struct FocusTarget {
    exact: SessionInfo,
    shared_window: Option<SessionInfo>,
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
    session_found: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    focus_precision: Option<FocusPrecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    clicked_notification_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<FocusError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FocusPrecision {
    Exact,
    SharedWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FocusError {
    SessionNotFound,
    FocusFailed,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivationResponse {
    session: SessionInfo,
    #[serde(flatten)]
    focus: FocusResponse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FocusRequest {
    #[serde(default)]
    notification_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DismissNotificationRequest {
    session_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DismissNotificationsRequest {
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DismissNotificationsResponse {
    dismissed_count: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or_default() {
        Commands::Serve(args) => serve(args).await,
        Commands::Activate(args) => activate(args).await,
        Commands::CheckHooks | Commands::RepairHooks => {
            let paths = HookInstallPaths::from_env();
            let report = install_or_repair(&paths)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
    }
}

impl Default for Commands {
    fn default() -> Self {
        Self::Serve(ServeArgs::default())
    }
}

async fn serve(args: ServeArgs) -> Result<()> {
    let config = load_or_create_config().context("failed to load runtime config")?;
    if config.hooks.auto_check && config.hooks.auto_install {
        let paths = HookInstallPaths::from_env();
        if let Err(error) = install_or_repair(&paths) {
            eprintln!("hook manager warning: {error}");
        }
    }
    #[cfg(windows)]
    if !ensure_windows_toast_registration(
        windows_icon_assets().map(|assets| assets.ico.as_str()),
        active_runtime_home().as_str(),
    ) {
        eprintln!("toast registration warning: failed to register Windows Start Menu shortcut");
    }
    let shutdown = Arc::new(Notify::new());
    let token = read_token(&config).context("failed to read auth token")?;
    let state = build_state(&config, token);
    #[cfg(windows)]
    if !args.no_native_tray {
        start_windows_tray(
            shutdown.clone(),
            windows_icon_assets().map(|assets| assets.ico.clone()),
            state.floating_bar_state.clone(),
            state.floating_bar_state_path.clone(),
        );
    }
    let app = Router::new()
        .route("/events", post(post_event))
        .route("/sessions", get(get_sessions))
        .route("/floating-status", get(get_floating_status))
        .route("/floating-status/state", put(put_floating_status_state))
        .route(
            "/floating-status/notifications/dismiss-all",
            post(dismiss_floating_notifications),
        )
        .route(
            "/floating-status/notifications/{notification_id}/dismiss",
            post(dismiss_floating_notification),
        )
        .route(
            "/floating-status/ui-heartbeat",
            post(post_floating_ui_heartbeat),
        )
        .route("/focus/{session_id}", post(focus_session))
        .route("/activate/{activation_id}", post(activate_session))
        .with_state(state);
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port)
        .parse()
        .context("invalid listen address")?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("agent-notify-tray listening on http://{addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(wait_for_shutdown(shutdown))
        .await?;
    Ok(())
}

async fn wait_for_shutdown(shutdown: Arc<Notify>) {
    tokio::select! {
        _ = shutdown.notified() => {}
        result = tokio::signal::ctrl_c() => {
            if let Err(error) = result {
                eprintln!("shutdown signal warning: {error}");
            }
        }
    }
}

fn build_state(config: &RuntimeConfig, token: String) -> AppState {
    let floating_bar_state_path = floating_bar_state_path(config);
    let floating_bar_state = load_floating_bar_state(&floating_bar_state_path);
    AppState {
        token: Arc::new(token),
        listener_enabled: Arc::new(Mutex::new(config.notifications.listener_enabled)),
        sessions: Arc::new(Mutex::new(HashMap::new())),
        notifications: Arc::new(Mutex::new(HashMap::new())),
        floating_bar_state: Arc::new(Mutex::new(floating_bar_state)),
        floating_bar_state_path: Arc::new(floating_bar_state_path),
        floating_ui_heartbeat: Arc::new(Mutex::new(None)),
        dedupe: Arc::new(Mutex::new(DedupeCache::new(Duration::from_secs(
            config.notifications.dedupe_seconds,
        )))),
        activations: Arc::new(Mutex::new(HashMap::new())),
    }
}

async fn activate(args: ActivateArgs) -> Result<()> {
    let activation_id =
        activation_id_from_uri(&args.uri).context("invalid agent-notify activation uri")?;
    let config = load_or_create_runtime_config(args.home.as_deref())
        .context("failed to load runtime config")?;
    let token = read_token(&config).context("failed to read auth token")?;
    let endpoint = config.endpoint(&format!("/activate/{activation_id}"));
    allow_any_foreground_activation();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(1500))
        .build()
        .context("failed to create activation client")?;
    let response = client
        .post(endpoint)
        .bearer_auth(token)
        .send()
        .await
        .context("failed to contact agent-notify backend")?;
    if !response.status().is_success() {
        anyhow::bail!(
            "agent-notify backend rejected activation: {}",
            response.status()
        );
    }
    let activation: ActivationResponse = response
        .json()
        .await
        .context("invalid activation response")?;
    if !activation.focus.focused {
        focus::focus_window(&activation.session);
    }
    Ok(())
}

fn load_or_create_runtime_config(home: Option<&FsPath>) -> std::io::Result<RuntimeConfig> {
    match home {
        Some(home) => agent_notify_core::config::load_or_create_config_at(home.join("config.json")),
        None => load_or_create_config(),
    }
}

#[cfg(windows)]
fn allow_any_foreground_activation() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }
}

#[cfg(not(windows))]
fn allow_any_foreground_activation() {}

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
        SessionInfo::from_event(&event, now.clone()),
    );
    let should_emit = state
        .dedupe
        .lock()
        .await
        .should_emit(&event, Instant::now());
    let floating_notification = if should_emit {
        record_floating_notification(&state, &event, &now).await
    } else {
        None
    };
    let notified =
        should_emit && notify_event(&state, &event, floating_notification.as_ref()).await;

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

async fn get_floating_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<FloatingStatusSnapshot>, StatusCode> {
    authorize(&headers, &state)?;
    Ok(Json(floating_status_snapshot(&state).await))
}

async fn put_floating_status_state(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(next_state): Json<FloatingBarState>,
) -> Result<Json<FloatingBarState>, StatusCode> {
    authorize(&headers, &state)?;
    let next_state = next_state.normalized();
    save_floating_bar_state(&state.floating_bar_state_path, &next_state)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    *state.floating_bar_state.lock().await = next_state.clone();
    Ok(Json(next_state))
}

async fn post_floating_ui_heartbeat(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    authorize(&headers, &state)?;
    *state.floating_ui_heartbeat.lock().await = Some(Instant::now());
    Ok(StatusCode::NO_CONTENT)
}

async fn dismiss_floating_notification(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(notification_id): Path<String>,
    Json(request): Json<DismissNotificationRequest>,
) -> Result<Json<DismissNotificationsResponse>, StatusCode> {
    authorize(&headers, &state)?;
    let dismissed =
        mark_notification_clicked_for_session(&state, &notification_id, &request.session_id)
            .await?;
    Ok(Json(DismissNotificationsResponse {
        dismissed_count: usize::from(dismissed),
    }))
}

async fn dismiss_floating_notifications(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<DismissNotificationsRequest>,
) -> Result<Json<DismissNotificationsResponse>, StatusCode> {
    authorize(&headers, &state)?;
    let targets = dismissible_notification_targets(&state, request.session_id.as_deref()).await;
    let clicked_at = Utc::now().to_rfc3339();
    let mut notifications = state.notifications.lock().await;
    let dismissed_count = targets
        .into_iter()
        .filter(|(notification_id, session_id)| {
            mark_notification_clicked(
                &mut notifications,
                notification_id,
                session_id,
                clicked_at.clone(),
            )
            .unwrap_or(false)
        })
        .count();
    Ok(Json(DismissNotificationsResponse { dismissed_count }))
}

async fn focus_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    body: Bytes,
) -> Result<Json<FocusResponse>, StatusCode> {
    authorize(&headers, &state)?;
    let request = parse_focus_request(&body)?;
    let notification_id = request.and_then(|request| request.notification_id);
    if let Some(notification_id) = notification_id.as_deref() {
        ensure_notification_for_session(&state, notification_id, &session_id).await?;
    }
    let mut response = focus_session_id(&state, &session_id, notification_id.as_deref()).await;
    if let Some(notification_id) = notification_id
        && mark_notification_clicked_for_session(&state, &notification_id, &session_id).await?
    {
        response.clicked_notification_id = Some(notification_id);
    }
    Ok(Json(response))
}

async fn activate_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(activation_id): Path<String>,
) -> Result<Json<ActivationResponse>, StatusCode> {
    authorize(&headers, &state)?;
    let Some(activation_id) = canonical_activation_id(&activation_id) else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let Some(target) = take_activation_target(&state, &activation_id).await else {
        return Err(StatusCode::NOT_FOUND);
    };
    let sessions = session_snapshot(&state).await;
    let focus_target = focus_target(target.session.clone(), &sessions);
    let mut response = focus_session_target(Some(focus_target), true);
    if mark_notification_clicked_for_session(&state, &target.notification_id, &target.session_id)
        .await?
    {
        response.clicked_notification_id = Some(target.notification_id.clone());
    }
    Ok(Json(ActivationResponse {
        session: target.session,
        focus: response,
    }))
}

async fn focus_session_id(
    state: &AppState,
    session_id: &str,
    notification_id: Option<&str>,
) -> FocusResponse {
    let sessions = session_snapshot(state).await;
    let session = sessions
        .iter()
        .find(|session| session.session_id == session_id)
        .cloned();
    let session_found = session.is_some();
    let notification_target = if let Some(notification_id) = notification_id {
        state
            .notifications
            .lock()
            .await
            .get(notification_id)
            .cloned()
    } else {
        None
    };
    let session = notification_target
        .and_then(|notification| {
            session
                .clone()
                .map(|session| session_with_notification_target(session, &notification))
        })
        .or(session);
    let session = session.map(|session| focus_target(session, &sessions));
    focus_session_target(session, session_found)
}

fn focus_session_target(target: Option<FocusTarget>, session_found: bool) -> FocusResponse {
    let mut buffer: Vec<String> = Vec::new();
    let mut log = |fields: &str| {
        buffer.push(format!(
            "{} component=tray-focus {}\r\n",
            Utc::now().to_rfc3339(),
            fields
        ));
    };
    let (focused, focus_precision) = match target.as_ref() {
        Some(target) if focus::focus_window_with_logger(&target.exact, &mut log) => {
            (true, Some(FocusPrecision::Exact))
        }
        Some(target) => target
            .shared_window
            .as_ref()
            .filter(|session| focus::focus_window_handle_with_logger(session, &mut log))
            .map_or((false, None), |_| {
                (true, Some(FocusPrecision::SharedWindow))
            }),
        None => (false, None),
    };
    flush_focus_log(&buffer);
    FocusResponse {
        focused,
        opened_session_detail: !focused,
        session_found,
        focus_precision,
        clicked_notification_id: None,
        error: if !session_found {
            Some(FocusError::SessionNotFound)
        } else if !focused {
            Some(FocusError::FocusFailed)
        } else {
            None
        },
    }
}

fn session_with_notification_target(
    mut session: SessionInfo,
    notification: &FloatingNotificationRecord,
) -> SessionInfo {
    if notification.process.is_some() {
        session.process = notification.process.clone();
    }
    if notification.window.is_some() {
        session.window = notification.window.clone();
    }
    session
}

fn focus_target(session: SessionInfo, sessions: &[SessionInfo]) -> FocusTarget {
    let shared_window = shared_window_focus_target(&session, sessions);
    let exact = disambiguate_focus_target(session, sessions);
    FocusTarget {
        exact,
        shared_window,
    }
}

fn shared_window_focus_target(
    session: &SessionInfo,
    sessions: &[SessionInfo],
) -> Option<SessionInfo> {
    let window = session.window.as_ref()?;
    let hwnd = window.hwnd.filter(|hwnd| *hwnd != 0)?;
    if !other_session_has_hwnd(&session.session_id, sessions, hwnd) {
        return None;
    }
    Some(SessionInfo {
        process: None,
        window: Some(WindowInfo {
            pid: window.pid,
            title: window.title.clone(),
            hwnd: Some(hwnd),
            terminal: window.terminal.clone(),
        }),
        ..session.clone()
    })
}

async fn session_snapshot(state: &AppState) -> Vec<SessionInfo> {
    state.sessions.lock().await.values().cloned().collect()
}

fn disambiguate_focus_target(mut session: SessionInfo, sessions: &[SessionInfo]) -> SessionInfo {
    let session_id = session.session_id.clone();
    let Some(window) = session.window.as_mut() else {
        return session;
    };
    let shares_hwnd = window
        .hwnd
        .filter(|hwnd| *hwnd != 0)
        .is_some_and(|hwnd| other_session_has_hwnd(&session_id, sessions, hwnd));
    let shares_window_pid = window
        .pid
        .filter(|pid| *pid != 0)
        .is_some_and(|pid| other_session_has_window_pid(&session_id, sessions, pid));

    if shares_hwnd {
        window.hwnd = None;
        window.title = None;
    }
    if shares_hwnd || shares_window_pid {
        window.pid = None;
    }
    session
}

fn other_session_has_hwnd(session_id: &str, sessions: &[SessionInfo], hwnd: isize) -> bool {
    sessions.iter().any(|session| {
        session.session_id != session_id
            && session
                .window
                .as_ref()
                .and_then(|window| window.hwnd)
                .is_some_and(|candidate| candidate == hwnd)
    })
}

fn other_session_has_window_pid(session_id: &str, sessions: &[SessionInfo], pid: u32) -> bool {
    sessions.iter().any(|session| {
        session.session_id != session_id
            && session
                .window
                .as_ref()
                .and_then(|window| window.pid)
                .is_some_and(|candidate| candidate == pid)
    })
}

async fn floating_status_snapshot(state: &AppState) -> FloatingStatusSnapshot {
    let sessions = session_snapshot(state).await;
    let notifications = state
        .notifications
        .lock()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let bar_state = state.floating_bar_state.lock().await.clone();
    build_floating_status_snapshot(sessions, notifications, bar_state, Utc::now())
}

async fn dismissible_notification_targets(
    state: &AppState,
    session_id: Option<&str>,
) -> Vec<(String, String)> {
    floating_status_snapshot(state)
        .await
        .notifications
        .into_iter()
        .filter(|notification| match session_id {
            Some(session_id) => notification.session_id == session_id,
            None => true,
        })
        .map(|notification| (notification.notification_id, notification.session_id))
        .collect()
}

async fn record_floating_notification(
    state: &AppState,
    event: &AgentEvent,
    created_at: &str,
) -> Option<FloatingNotificationRecord> {
    if let Some(record) = notification_record_from_event(event, created_at.to_string()) {
        let notification = record.clone();
        state
            .notifications
            .lock()
            .await
            .insert(record.notification_id.clone(), record);
        Some(notification)
    } else {
        None
    }
}

fn parse_focus_request(body: &Bytes) -> Result<Option<FocusRequest>, StatusCode> {
    if body.is_empty() {
        return Ok(None);
    }
    serde_json::from_slice(body)
        .map(Some)
        .map_err(|_| StatusCode::BAD_REQUEST)
}

async fn ensure_notification_for_session(
    state: &AppState,
    notification_id: &str,
    session_id: &str,
) -> Result<(), StatusCode> {
    let notifications = state.notifications.lock().await;
    let Some(notification) = notifications.get(notification_id) else {
        return Err(StatusCode::NOT_FOUND);
    };
    if notification.session_id != session_id {
        return Err(StatusCode::CONFLICT);
    }
    Ok(())
}

async fn mark_notification_clicked_for_session(
    state: &AppState,
    notification_id: &str,
    session_id: &str,
) -> Result<bool, StatusCode> {
    let clicked_at = Utc::now().to_rfc3339();
    let mut notifications = state.notifications.lock().await;
    mark_notification_clicked(&mut notifications, notification_id, session_id, clicked_at)
        .map_err(notification_click_status)
}

fn notification_click_status(error: NotificationClickError) -> StatusCode {
    match error {
        NotificationClickError::NotFound => StatusCode::NOT_FOUND,
        NotificationClickError::SessionMismatch => StatusCode::CONFLICT,
    }
}

fn authorize(headers: &HeaderMap, state: &AppState) -> Result<(), StatusCode> {
    let expected = format!("Bearer {}", state.token.as_str());
    let authorized = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected);
    authorized.then_some(()).ok_or(StatusCode::UNAUTHORIZED)
}

async fn notify_event(
    state: &AppState,
    event: &AgentEvent,
    floating_notification: Option<&FloatingNotificationRecord>,
) -> bool {
    let Some(view) = notification_view(event) else {
        return false;
    };
    if floating_notification.is_some() && floating_ui_active(state).await {
        return true;
    }
    let activation = if cfg!(windows) {
        Some(register_activation(state, event).await)
    } else {
        None
    };
    let notified = show_notification(&view, activation.as_ref().map(|entry| entry.uri.as_str()));
    if !notified && let Some(entry) = activation {
        remove_activation(state, &entry.id).await;
    }
    notified
}

async fn floating_ui_active(state: &AppState) -> bool {
    state
        .floating_ui_heartbeat
        .lock()
        .await
        .is_some_and(|last_seen| last_seen.elapsed() <= FLOATING_UI_ACTIVE_TTL)
}

async fn register_activation(state: &AppState, event: &AgentEvent) -> ActivationRegistration {
    let id = Uuid::new_v4().to_string();
    let now = Instant::now();
    let mut activations = state.activations.lock().await;
    prune_activations(&mut activations, now);
    activations.insert(
        id.clone(),
        ActivationTarget {
            session_id: event.session_id.clone(),
            notification_id: event.event_id.clone(),
            session: SessionInfo::from_event(event, Utc::now().to_rfc3339()),
            expires_at: now + ACTIVATION_TTL,
        },
    );
    ActivationRegistration {
        uri: activation_uri(&id),
        id,
    }
}

async fn take_activation_target(state: &AppState, activation_id: &str) -> Option<ActivationTarget> {
    let now = Instant::now();
    let mut activations = state.activations.lock().await;
    prune_activations(&mut activations, now);
    activations
        .remove(activation_id)
        .filter(|target| target.expires_at > now)
}

async fn remove_activation(state: &AppState, activation_id: &str) {
    state.activations.lock().await.remove(activation_id);
}

fn prune_activations(activations: &mut HashMap<String, ActivationTarget>, now: Instant) {
    activations.retain(|_, target| target.expires_at > now);
}

fn activation_uri(activation_id: &str) -> String {
    format!("agent-notify://focus?activationId={activation_id}")
}

fn activation_id_from_uri(uri: &str) -> Option<String> {
    let uri = uri.trim();
    let (target, query) = uri.split_once('?')?;
    if !is_activation_uri_target(target) {
        return None;
    }
    let mut activation_id = None;
    for part in query.split('&') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if key.eq_ignore_ascii_case("activationId") {
            if activation_id.is_some() {
                return None;
            }
            activation_id = Some(canonical_activation_id(value)?);
        }
    }
    activation_id
}

fn is_activation_uri_target(target: &str) -> bool {
    target.eq_ignore_ascii_case("agent-notify://focus")
        || target.eq_ignore_ascii_case("agent-notify://focus/")
}

fn canonical_activation_id(value: &str) -> Option<String> {
    Uuid::parse_str(value.trim())
        .ok()
        .map(|activation_id| activation_id.to_string())
}

fn show_notification(view: &agent_notify_core::NotificationView, launch_uri: Option<&str>) -> bool {
    #[cfg(windows)]
    {
        show_windows_toast(view, launch_uri)
    }
    #[cfg(not(windows))]
    {
        let _ = launch_uri;
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
fn show_windows_toast(
    view: &agent_notify_core::NotificationView,
    launch_uri: Option<&str>,
) -> bool {
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
$launch = Escape-ToastText $env:AGENT_NOTIFY_TOAST_LAUNCH
$iconXml = ""
$iconPath = $env:AGENT_NOTIFY_TOAST_ICON
if (-not [string]::IsNullOrWhiteSpace($iconPath) -and (Test-Path -LiteralPath $iconPath)) {
    $iconUri = Escape-ToastText ([Uri]::new($iconPath).AbsoluteUri)
    $iconXml = "<image placement=""appLogoOverride"" hint-crop=""circle"" src=""$iconUri""/>"
}
$activationXml = ""
if (-not [string]::IsNullOrWhiteSpace($launch)) {
    $activationXml = " activationType=""protocol"" launch=""$launch"""
}
$template = "<toast$activationXml><visual><binding template=""ToastGeneric"">$iconXml<text>$title</text><text>$body</text></binding></visual></toast>"
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
            ("AGENT_NOTIFY_TOAST_LAUNCH", launch_uri.unwrap_or_default()),
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
fn active_runtime_home() -> String {
    let path = agent_notify_core::agent_notify_dir();
    windows_normalize_path(&path.canonicalize().unwrap_or(path).display().to_string())
}

#[cfg(windows)]
fn ensure_windows_toast_registration(icon_path: Option<&str>, runtime_home: &str) -> bool {
    let target_path = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("toast registration warning: failed to resolve current executable: {error}");
            return false;
        }
    };
    let target = windows_normalize_path(&target_path.display().to_string());
    let activation_target_path = target_path.with_file_name(windows_activation_helper_name());
    let activation_target = if activation_target_path.is_file() {
        Some(windows_normalize_path(
            &activation_target_path.display().to_string(),
        ))
    } else {
        eprintln!(
            "toast registration warning: activation helper was not found; falling back to tray activation subcommand: {}",
            activation_target_path.display()
        );
        None
    };
    let protocol_command =
        windows_protocol_command(&target, activation_target.as_deref(), runtime_home);
    let script = r#"
$ErrorActionPreference = "Stop"
$target = $env:AGENT_NOTIFY_EXE_PATH
$appName = $env:AGENT_NOTIFY_WINDOWS_APP_NAME
$legacyAppName = $env:AGENT_NOTIFY_WINDOWS_LEGACY_APP_NAME
$iconPath = $env:AGENT_NOTIFY_SHORTCUT_ICON
$uriScheme = $env:AGENT_NOTIFY_URI_SCHEME
$protocolCommand = $env:AGENT_NOTIFY_PROTOCOL_COMMAND
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

if (-not [string]::IsNullOrWhiteSpace($uriScheme) -and -not [string]::IsNullOrWhiteSpace($protocolCommand)) {
    $protocolKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Software\Classes\$uriScheme")
    $protocolKey.SetValue("", "URL:$uriScheme")
    $protocolKey.SetValue("URL Protocol", "")
    $commandKey = $protocolKey.CreateSubKey("shell\open\command")
    $commandKey.SetValue("", $protocolCommand)
    $commandKey.Close()
    $protocolKey.Close()
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
            ("AGENT_NOTIFY_URI_SCHEME", WINDOWS_URI_SCHEME),
            ("AGENT_NOTIFY_PROTOCOL_COMMAND", protocol_command.as_str()),
        ],
    )
}

#[cfg(windows)]
fn windows_protocol_command(
    tray_target: &str,
    activation_target: Option<&str>,
    runtime_home: &str,
) -> String {
    match activation_target {
        Some(activation_target) => format!(
            "{} --home {} {}",
            windows_command_arg(activation_target),
            windows_command_arg(runtime_home),
            windows_command_arg("%1")
        ),
        None => format!(
            "{} activate --home {} {}",
            windows_command_arg(tray_target),
            windows_command_arg(runtime_home),
            windows_command_arg("%1")
        ),
    }
}

#[cfg(windows)]
fn windows_activation_helper_name() -> &'static str {
    "agent-notify-activate.exe"
}

#[cfg(windows)]
fn windows_normalize_path(value: &str) -> String {
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = value.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        value.to_string()
    }
}

#[cfg(windows)]
fn windows_command_arg(value: &str) -> String {
    let mut quoted = String::from("\"");
    let mut backslashes = 0usize;
    for ch in value.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                quoted.push(ch);
            }
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

#[cfg(windows)]
fn start_windows_tray(
    shutdown: Arc<Notify>,
    icon_path: Option<String>,
    bar_state: Arc<Mutex<FloatingBarState>>,
    bar_state_path: Arc<PathBuf>,
) {
    if WINDOWS_TRAY_SHUTDOWN.set(shutdown).is_err() {
        eprintln!("tray warning: shutdown signal was already registered");
        return;
    }
    let _ = WINDOWS_TRAY_BAR_STATE.set((bar_state, bar_state_path));
    let spawn_result = std::thread::Builder::new()
        .name("agent-notify-tray-icon".to_string())
        .spawn(move || {
            if let Err(error) = run_windows_tray(icon_path.as_deref()) {
                eprintln!("tray warning: {error:#}");
            }
        });
    if let Err(error) = spawn_result {
        eprintln!("tray warning: failed to start tray thread: {error}");
    }
}

#[cfg(windows)]
fn run_windows_tray(icon_path: Option<&str>) -> Result<()> {
    use std::ptr;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyIcon, DispatchMessageW, GetMessageW, MSG, RegisterClassW,
        TranslateMessage, WNDCLASSW,
    };

    let class_name = wide_null("AgentNotifyTrayWindow");
    let window_name = wide_null("Agent Notify Tray");
    let hinstance = unsafe { GetModuleHandleW(ptr::null()) };
    let window_class = WNDCLASSW {
        lpfnWndProc: Some(windows_tray_wnd_proc),
        hInstance: hinstance,
        lpszClassName: class_name.as_ptr(),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&window_class) } == 0 {
        anyhow::bail!("failed to register tray window class");
    }

    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            window_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            hinstance,
            ptr::null(),
        )
    };
    if hwnd.is_null() {
        anyhow::bail!("failed to create tray window");
    }

    let (icon, should_destroy_icon) = load_windows_tray_icon(icon_path);
    add_windows_tray_icon(hwnd, icon)?;

    let mut message: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    delete_windows_tray_icon(hwnd);
    if should_destroy_icon && !icon.is_null() {
        unsafe {
            DestroyIcon(icon);
        }
    }
    Ok(())
}

#[cfg(windows)]
unsafe extern "system" fn windows_tray_wnd_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    message: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, DestroyWindow, PostQuitMessage, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY,
        WM_LBUTTONUP, WM_RBUTTONUP,
    };

    match message {
        WINDOWS_TRAY_MESSAGE => {
            let event = lparam as u32;
            if matches!(event, WM_CONTEXTMENU | WM_LBUTTONUP | WM_RBUTTONUP) {
                show_windows_tray_menu(hwnd);
                return 0;
            }
        }
        WM_COMMAND => {
            match wparam & 0xffff {
                id if id == WINDOWS_TRAY_TOGGLE_BAR_ID => {
                    toggle_floating_bar_visibility();
                    return 0;
                }
                id if id == WINDOWS_TRAY_OPEN_LOGS_ID => {
                    open_logs_folder();
                    return 0;
                }
                id if id == WINDOWS_TRAY_EXIT_ID => {
                    if let Some(shutdown) = WINDOWS_TRAY_SHUTDOWN.get() {
                        shutdown.notify_waiters();
                    }
                    terminate_parent_desktop();
                    unsafe {
                        DestroyWindow(hwnd);
                    }
                    return 0;
                }
                _ => {}
            }
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            return 0;
        }
        _ => {}
    }

    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

#[cfg(windows)]
fn show_windows_tray_menu(hwnd: windows_sys::Win32::Foundation::HWND) {
    use std::ptr;
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, MF_SEPARATOR, MF_STRING,
        SetForegroundWindow, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RIGHTBUTTON, TrackPopupMenu,
    };

    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return;
    }
    let hidden = WINDOWS_TRAY_BAR_STATE
        .get()
        .map(|(state, _)| state.blocking_lock().hidden)
        .unwrap_or(false);
    let toggle_label = wide_null(if hidden { "显示浮窗" } else { "隐藏浮窗" });
    let logs_label = wide_null("打开日志目录");
    let exit_label = wide_null("退出");
    unsafe {
        AppendMenuW(menu, MF_STRING, WINDOWS_TRAY_TOGGLE_BAR_ID, toggle_label.as_ptr());
        AppendMenuW(menu, MF_STRING, WINDOWS_TRAY_OPEN_LOGS_ID, logs_label.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
        AppendMenuW(menu, MF_STRING, WINDOWS_TRAY_EXIT_ID, exit_label.as_ptr());
    }
    let mut point = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut point);
        SetForegroundWindow(hwnd);
        TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_LEFTALIGN,
            point.x,
            point.y,
            0,
            hwnd,
            ptr::null(),
        );
        DestroyMenu(menu);
    }
}

#[cfg(windows)]
fn toggle_floating_bar_visibility() {
    let Some((state, path)) = WINDOWS_TRAY_BAR_STATE.get() else {
        return;
    };
    let next_state = {
        let mut guard = state.blocking_lock();
        guard.hidden = !guard.hidden;
        guard.clone()
    };
    if let Err(error) = save_floating_bar_state(path.as_ref(), &next_state) {
        eprintln!("tray warning: failed to persist floating bar state: {error}");
    }
}

#[cfg(windows)]
fn open_logs_folder() {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;

    let path = agent_notify_core::agent_notify_dir().join("logs");
    if std::fs::create_dir_all(&path).is_err() {
        return;
    }
    let verb = wide_null("open");
    let path_str = wide_null(&path.display().to_string());
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            path_str.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        );
    }
}

#[cfg(windows)]
fn terminate_parent_desktop() {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, QueryFullProcessImageNameW,
        TerminateProcess,
    };

    let Some(parent_pid) = std::env::var("AGENT_NOTIFY_DESKTOP_PID")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|pid| *pid != 0)
    else {
        return;
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, parent_pid);
        if handle.is_null() {
            return;
        }
        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        if ok == 0 {
            return;
        }
        let image = String::from_utf16_lossy(&buffer[..len as usize]);
        let basename = std::path::Path::new(&image)
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        if basename != "agent-notify-desktop.exe" {
            return;
        }
        let kill_handle = OpenProcess(PROCESS_TERMINATE, 0, parent_pid);
        if !kill_handle.is_null() {
            TerminateProcess(kill_handle, 0);
            CloseHandle(kill_handle);
        }
    }
}

#[cfg(windows)]
fn add_windows_tray_icon(
    hwnd: windows_sys::Win32::Foundation::HWND,
    icon: windows_sys::Win32::UI::WindowsAndMessaging::HICON,
) -> Result<()> {
    use windows_sys::Win32::UI::Shell::{
        NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, Shell_NotifyIconW,
    };

    let mut data = notify_icon_data(hwnd);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WINDOWS_TRAY_MESSAGE;
    data.hIcon = icon;
    write_wide_buffer(&mut data.szTip, "智能任务通知");

    if unsafe { Shell_NotifyIconW(NIM_ADD, &data) } == 0 {
        anyhow::bail!("failed to add tray icon");
    }
    Ok(())
}

#[cfg(windows)]
fn delete_windows_tray_icon(hwnd: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::UI::Shell::{NIM_DELETE, Shell_NotifyIconW};

    let data = notify_icon_data(hwnd);
    unsafe {
        Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

#[cfg(windows)]
fn notify_icon_data(
    hwnd: windows_sys::Win32::Foundation::HWND,
) -> windows_sys::Win32::UI::Shell::NOTIFYICONDATAW {
    let mut data: windows_sys::Win32::UI::Shell::NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = std::mem::size_of::<windows_sys::Win32::UI::Shell::NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = WINDOWS_TRAY_ICON_ID;
    data
}

#[cfg(windows)]
fn load_windows_tray_icon(
    icon_path: Option<&str>,
) -> (windows_sys::Win32::UI::WindowsAndMessaging::HICON, bool) {
    use std::ptr;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTSIZE, LR_LOADFROMFILE, LoadIconW, LoadImageW,
    };

    if let Some(icon_path) = icon_path.filter(|path| !path.trim().is_empty()) {
        let wide_path = wide_null(icon_path);
        let icon = unsafe {
            LoadImageW(
                ptr::null_mut(),
                wide_path.as_ptr(),
                IMAGE_ICON,
                0,
                0,
                LR_LOADFROMFILE | LR_DEFAULTSIZE,
            )
        } as windows_sys::Win32::UI::WindowsAndMessaging::HICON;
        if !icon.is_null() {
            return (icon, true);
        }
    }

    (
        unsafe { LoadIconW(ptr::null_mut(), IDI_APPLICATION) },
        false,
    )
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn write_wide_buffer(buffer: &mut [u16], value: &str) {
    if buffer.is_empty() {
        return;
    }
    let max_len = buffer.len() - 1;
    let mut written = 0;
    for code_unit in value.encode_utf16().take(max_len) {
        buffer[written] = code_unit;
        written += 1;
    }
    buffer[written] = 0;
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

fn flush_focus_log(lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    let dir = agent_notify_core::agent_notify_dir().join("logs");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("activation.log"))
        .and_then(|mut file| {
            use std::io::Write;
            for line in lines {
                file.write_all(line.as_bytes())?;
            }
            Ok(())
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_notify_core::{
        EventType, MessageInfo, ProcessInfo, ProjectInfo, SessionStatus, WindowInfo,
    };

    #[test]
    fn activation_uri_round_trips_id() {
        let id = "f3b8a91d-2c02-49f1-a42a-3b58ed5bda10";

        assert_eq!(
            activation_id_from_uri(&activation_uri(id)).as_deref(),
            Some(id)
        );
    }

    #[test]
    fn activation_uri_allows_extra_query_parameters() {
        let id = "f3b8a91d-2c02-49f1-a42a-3b58ed5bda10";

        assert_eq!(
            activation_id_from_uri(&format!(
                "agent-notify://focus?source=toast&activationId={id}"
            ))
            .as_deref(),
            Some(id)
        );
    }

    #[test]
    fn activation_uri_accepts_windows_normalized_focus_target() {
        let id = "f3b8a91d-2c02-49f1-a42a-3b58ed5bda10";

        assert_eq!(
            activation_id_from_uri(&format!("agent-notify://focus/?activationId={id}")).as_deref(),
            Some(id)
        );
    }

    #[test]
    fn activation_uri_rejects_session_id_only_links() {
        assert!(activation_id_from_uri("agent-notify://focus?sessionId=s1").is_none());
        assert!(activation_id_from_uri("https://example.test/focus?activationId=s1").is_none());
    }

    #[test]
    fn activation_uri_rejects_path_traversal_ids() {
        assert!(
            activation_id_from_uri("agent-notify://focus?activationId=../focus/known-session")
                .is_none()
        );
        assert!(
            activation_id_from_uri("agent-notify://focus?activationId=%2e%2e/focus/session")
                .is_none()
        );
    }

    #[test]
    fn activation_uri_rejects_duplicate_activation_ids() {
        assert!(
            activation_id_from_uri("agent-notify://focus?activationId=f3b8a91d-2c02-49f1-a42a-3b58ed5bda10&activationId=f3b8a91d-2c02-49f1-a42a-3b58ed5bda10")
                .is_none()
        );
    }

    #[test]
    fn activation_uri_canonicalizes_uuid_case() {
        assert_eq!(
            activation_id_from_uri(
                "agent-notify://focus?activationId=F3B8A91D-2C02-49F1-A42A-3B58ED5BDA10"
            )
            .as_deref(),
            Some("f3b8a91d-2c02-49f1-a42a-3b58ed5bda10")
        );
    }

    #[cfg(windows)]
    #[test]
    fn protocol_command_uses_gui_activation_helper() {
        let command = windows_protocol_command(
            r"D:\Agent Notify Home\agent-notify-tray.exe",
            Some(r"D:\Agent Notify Home\agent-notify-activate.exe"),
            r"C:\Users\alice\AppData\Local\AgentNotify",
        );

        assert!(command.starts_with(r#""D:\Agent Notify Home\agent-notify-activate.exe""#));
        assert!(command.contains(" --home "));
        assert!(command.contains(r#""C:\Users\alice\AppData\Local\AgentNotify""#));
        assert!(command.ends_with(r#""%1""#));
    }

    #[cfg(windows)]
    #[test]
    fn protocol_command_falls_back_to_tray_activate() {
        let command = windows_protocol_command(
            r"D:\Agent Notify Home\agent-notify-tray.exe",
            None,
            r"C:\Users\alice\AppData\Local\AgentNotify",
        );

        assert!(command.starts_with(r#""D:\Agent Notify Home\agent-notify-tray.exe" activate"#));
        assert!(command.contains(" --home "));
        assert!(command.contains(r#""C:\Users\alice\AppData\Local\AgentNotify""#));
        assert!(command.ends_with(r#""%1""#));
    }

    #[cfg(windows)]
    #[test]
    fn activation_helper_name_is_executable() {
        assert_eq!(
            windows_activation_helper_name(),
            "agent-notify-activate.exe"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_normalize_path_removes_extended_prefix() {
        assert_eq!(
            windows_normalize_path(r"\\?\C:\Users\alice\AppData\Local\AgentNotify"),
            r"C:\Users\alice\AppData\Local\AgentNotify"
        );
        assert_eq!(
            windows_normalize_path(r"\\?\UNC\server\share\AgentNotify"),
            r"\\server\share\AgentNotify"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_command_arg_preserves_trailing_backslash() {
        assert_eq!(
            windows_command_arg(r"D:\AgentNotify\"),
            "\"D:\\AgentNotify\\\\\""
        );
    }

    #[tokio::test]
    async fn focus_request_marks_notification_clicked_even_when_focus_fails() {
        let state = test_state();
        state
            .sessions
            .lock()
            .await
            .insert("s1".to_string(), test_session("s1"));
        state
            .notifications
            .lock()
            .await
            .insert("n1".to_string(), test_notification("n1", "s1"));
        let response = focus_session(
            State(state.clone()),
            auth_headers(),
            Path("s1".to_string()),
            Bytes::from_static(br#"{"notificationId":"n1"}"#),
        )
        .await
        .unwrap()
        .0;

        assert!(!response.focused);
        assert_eq!(response.clicked_notification_id.as_deref(), Some("n1"));
        assert!(
            state
                .notifications
                .lock()
                .await
                .get("n1")
                .unwrap()
                .clicked_at
                .is_some()
        );
    }

    #[tokio::test]
    async fn dismiss_all_marks_visible_notifications_in_requested_scope() {
        let state = test_state();
        state
            .sessions
            .lock()
            .await
            .insert("s1".to_string(), test_visible_session("s1"));
        state
            .sessions
            .lock()
            .await
            .insert("s2".to_string(), test_visible_session("s2"));
        state
            .notifications
            .lock()
            .await
            .insert("n1".to_string(), test_notification("n1", "s1"));
        state
            .notifications
            .lock()
            .await
            .insert("n2".to_string(), test_notification("n2", "s2"));

        let response = dismiss_floating_notifications(
            State(state.clone()),
            auth_headers(),
            Json(DismissNotificationsRequest {
                session_id: Some("s1".to_string()),
            }),
        )
        .await
        .unwrap()
        .0;

        let notifications = state.notifications.lock().await;
        assert_eq!(response.dismissed_count, 1);
        assert!(notifications.get("n1").unwrap().clicked_at.is_some());
        assert!(notifications.get("n2").unwrap().clicked_at.is_none());
    }

    #[test]
    fn disambiguate_focus_target_removes_shared_window_identity() {
        let target = test_windowed_session("s1", Some(100), Some(200), Some("notify"));
        let other = test_windowed_session("s2", Some(100), Some(200), Some("notify"));

        let result = disambiguate_focus_target(target, &[other]);
        let window = result.window.unwrap();

        assert_eq!(window.hwnd, None);
        assert_eq!(window.pid, None);
        assert_eq!(window.title, None);
        assert_eq!(result.process.unwrap().pid, Some(42));
    }

    #[test]
    fn disambiguate_focus_target_keeps_unique_window_identity() {
        let target = test_windowed_session("s1", Some(100), Some(200), Some("notify"));
        let other = test_windowed_session("s2", Some(101), Some(201), Some("other"));

        let result = disambiguate_focus_target(target, &[other]);
        let window = result.window.unwrap();

        assert_eq!(window.hwnd, Some(100));
        assert_eq!(window.pid, Some(200));
        assert_eq!(window.title.as_deref(), Some("notify"));
    }

    #[test]
    fn focus_target_keeps_shared_hwnd_only_as_fallback() {
        let target = test_windowed_session("s1", Some(100), Some(200), Some("notify"));
        let other = test_windowed_session("s2", Some(100), Some(200), Some("notify"));

        let result = focus_target(target, &[other]);
        let exact_window = result.exact.window.unwrap();
        let shared = result.shared_window.unwrap();
        let shared_window = shared.window.unwrap();

        assert_eq!(exact_window.hwnd, None);
        assert_eq!(exact_window.pid, None);
        assert_eq!(shared.process, None);
        assert_eq!(shared_window.hwnd, Some(100));
        assert_eq!(shared_window.pid, Some(200));
    }

    fn test_state() -> AppState {
        AppState {
            token: Arc::new("test-token".to_string()),
            listener_enabled: Arc::new(Mutex::new(true)),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            notifications: Arc::new(Mutex::new(HashMap::new())),
            floating_bar_state: Arc::new(Mutex::new(FloatingBarState::default())),
            floating_bar_state_path: Arc::new(PathBuf::from("floating-status-bar-test.json")),
            floating_ui_heartbeat: Arc::new(Mutex::new(None)),
            dedupe: Arc::new(Mutex::new(DedupeCache::new(Duration::from_secs(1)))),
            activations: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn auth_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            "Bearer test-token".parse().unwrap(),
        );
        headers
    }

    fn test_session(session_id: &str) -> SessionInfo {
        SessionInfo {
            session_id: session_id.to_string(),
            tool: "codex".to_string(),
            project: ProjectInfo {
                cwd: r"D:\own\notify".to_string(),
                name: "notify".to_string(),
            },
            status: SessionStatus::Completed,
            last_event_type: EventType::TaskCompleted,
            last_message: MessageInfo {
                title: "done".to_string(),
                body: "notify".to_string(),
                detail: None,
            },
            process: None,
            window: None,
            updated_at: "2026-05-11T11:59:00Z".to_string(),
        }
    }

    fn test_visible_session(session_id: &str) -> SessionInfo {
        SessionInfo {
            process: Some(ProcessInfo {
                pid: Some(42),
                parent_pid: None,
                started_at: None,
            }),
            updated_at: Utc::now().to_rfc3339(),
            ..test_session(session_id)
        }
    }

    fn test_windowed_session(
        session_id: &str,
        hwnd: Option<isize>,
        window_pid: Option<u32>,
        title: Option<&str>,
    ) -> SessionInfo {
        SessionInfo {
            process: Some(ProcessInfo {
                pid: Some(42),
                parent_pid: Some(24),
                started_at: None,
            }),
            window: Some(WindowInfo {
                pid: window_pid,
                title: title.map(str::to_string),
                hwnd,
                terminal: Some("WindowsTerminal".to_string()),
            }),
            ..test_session(session_id)
        }
    }

    fn test_notification(notification_id: &str, session_id: &str) -> FloatingNotificationRecord {
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
