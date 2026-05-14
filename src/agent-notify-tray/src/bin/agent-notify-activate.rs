#![cfg_attr(windows, windows_subsystem = "windows")]

use agent_notify_core::{RuntimeConfig, SessionInfo, read_token};
use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

#[path = "../focus.rs"]
mod focus;

#[derive(Debug, Parser)]
#[command(name = "agent-notify-activate")]
struct Args {
    #[arg(long)]
    home: Option<PathBuf>,
    uri: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivationResponse {
    session: SessionInfo,
    focused: bool,
}

#[tokio::main]
async fn main() {
    if let Err(error) = activate(Args::parse()).await {
        log_activation_error(&error);
    }
}

async fn activate(args: Args) -> Result<()> {
    log_activation_event(&format!(
        "code=start home={} {}",
        args.home
            .as_ref()
            .map(|path| sanitize_log_text(&path.display().to_string()))
            .unwrap_or_else(|| "<default>".to_string()),
        activation_uri_log_shape(&args.uri)
    ));
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
    let status = response.status();
    log_activation_event(&format!("code=backend_response status={status}"));
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
    let focused = activation.focused
        || focus::focus_window_with_logger(&activation.session, log_activation_event);
    log_activation_event(&format!("code=done focused={focused}"));
    Ok(())
}

fn load_or_create_runtime_config(home: Option<&Path>) -> std::io::Result<RuntimeConfig> {
    match home {
        Some(home) => agent_notify_core::config::load_or_create_config_at(home.join("config.json")),
        None => agent_notify_core::load_or_create_config(),
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

fn log_activation_error(error: &anyhow::Error) {
    log_activation_event(&format!("code=failed error={}", error));
}

fn log_activation_event(fields: &str) {
    let dir = agent_notify_core::agent_notify_dir().join("logs");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let line = format!(
        "{} component=activation {}\r\n",
        chrono::Utc::now().to_rfc3339(),
        sanitize_log_text(fields)
    );
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("activation.log"))
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(line.as_bytes())
        });
}

fn activation_uri_log_shape(uri: &str) -> String {
    let uri = uri.trim();
    let (target, query) = uri.split_once('?').unwrap_or((uri, ""));
    let keys = query
        .split('&')
        .filter_map(|part| part.split_once('=').map(|(key, _)| sanitize_log_text(key)))
        .take(8)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "uri_len={} uri_target={} query_keys={}",
        uri.len(),
        sanitize_log_text(target),
        keys
    )
}

fn sanitize_log_text(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\r' | '\n' | '\t' => ' ',
            _ => ch,
        })
        .take(480)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_uri_accepts_exact_focus_target() {
        let id = "f3b8a91d-2c02-49f1-a42a-3b58ed5bda10";

        assert_eq!(
            activation_id_from_uri(&format!("agent-notify://focus?activationId={id}")).as_deref(),
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
    fn activation_uri_rejects_path_traversal_ids() {
        assert!(
            activation_id_from_uri("agent-notify://focus/?activationId=../focus/known-session")
                .is_none()
        );
        assert!(
            activation_id_from_uri("agent-notify://focus/?activationId=%2e%2e/focus/session")
                .is_none()
        );
    }
}
