use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use uuid::Uuid;

pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 17891;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeConfig {
    pub server: ServerConfig,
    pub auth: AuthConfig,
    pub notifications: NotificationConfig,
    pub hooks: HookConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthConfig {
    pub token_file: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationConfig {
    pub enabled: bool,
    pub listener_enabled: bool,
    pub dedupe_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookConfig {
    pub auto_check: bool,
    pub auto_install: bool,
    pub install_dir: String,
}

impl RuntimeConfig {
    pub fn default_for_dir(app_dir: PathBuf) -> Self {
        Self {
            server: ServerConfig {
                host: DEFAULT_HOST.to_string(),
                port: DEFAULT_PORT,
            },
            auth: AuthConfig {
                token_file: app_dir.join("token").display().to_string(),
            },
            notifications: NotificationConfig {
                enabled: true,
                listener_enabled: true,
                dedupe_seconds: 30,
            },
            hooks: HookConfig {
                auto_check: true,
                auto_install: true,
                install_dir: app_dir.join("hooks").display().to_string(),
            },
        }
    }

    pub fn endpoint(&self, path: &str) -> String {
        format!("http://{}:{}{}", self.server.host, self.server.port, path)
    }
}

pub fn agent_notify_dir() -> PathBuf {
    env::var_os("AGENT_NOTIFY_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("LOCALAPPDATA").map(|root| PathBuf::from(root).join("AgentNotify")))
        .unwrap_or_else(|| env::temp_dir().join("AgentNotify"))
}

pub fn config_path() -> PathBuf {
    agent_notify_dir().join("config.json")
}

pub fn token_path() -> PathBuf {
    agent_notify_dir().join("token")
}

pub fn load_or_create_config() -> io::Result<RuntimeConfig> {
    let path = config_path();
    load_or_create_config_at(path)
}

pub fn load_or_create_config_at(path: PathBuf) -> io::Result<RuntimeConfig> {
    if path.exists() {
        let bytes = strip_utf8_bom(fs::read(&path)?);
        return serde_json::from_slice(&bytes).map_err(invalid_data);
    }

    let app_dir = path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(agent_notify_dir);
    fs::create_dir_all(&app_dir)?;
    let config = RuntimeConfig::default_for_dir(app_dir);
    let text = serde_json::to_string_pretty(&config).map_err(invalid_data)?;
    fs::write(&path, text)?;
    ensure_token_file(PathBuf::from(&config.auth.token_file))?;
    Ok(config)
}

pub fn read_token(config: &RuntimeConfig) -> io::Result<String> {
    let path = PathBuf::from(&config.auth.token_file);
    if !path.exists() {
        ensure_token_file(path.clone())?;
    }
    fs::read_to_string(path).map(|value| value.trim().to_string())
}

pub fn ensure_token_file(path: PathBuf) -> io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let token = format!("agent-notify-{}", Uuid::new_v4().simple());
    fs::write(path, token)
}

fn invalid_data(error: impl std::error::Error + Send + Sync + 'static) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn strip_utf8_bom(bytes: Vec<u8>) -> Vec<u8> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        bytes[3..].to_vec()
    } else {
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_config_and_token() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let config = load_or_create_config_at(config_path.clone()).unwrap();

        assert!(config_path.exists());
        assert!(PathBuf::from(&config.auth.token_file).exists());
        assert_eq!(config.server.port, DEFAULT_PORT);
    }

    #[test]
    fn loads_config_with_utf8_bom() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let config = RuntimeConfig::default_for_dir(dir.path().to_path_buf());
        let text = serde_json::to_string_pretty(&config).unwrap();
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(text.as_bytes());
        fs::write(&config_path, bytes).unwrap();

        let loaded = load_or_create_config_at(config_path).unwrap();

        assert_eq!(loaded, config);
    }
}
