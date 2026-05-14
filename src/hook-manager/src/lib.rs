use agent_notify_core::agent_notify_dir;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use toml_edit::{DocumentMut, Item, Table, value};

pub const MANAGED_BY: &str = "agent-notify";
pub const HOOK_VERSION: &str = "0.1.0";
const EMBEDDED_HOOK_SCRIPT: &[u8] = include_bytes!("../../../scripts/hooks/agent-notify-hook.ps1");

const CLAUDE_EVENTS: &[&str] = &[
    "PermissionRequest",
    "Notification",
    "Stop",
    "StopFailure",
    "PostToolUseFailure",
    "SessionEnd",
];
const CODEX_EVENTS: &[&str] = &["SessionStart", "PermissionRequest", "Stop", "PostToolUse"];
const HOOK_COMMAND_TIMEOUT_SECONDS: u64 = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookStatus {
    Checking,
    Installing,
    Repairing,
    MissingCli,
    UnsupportedVersion,
    HookMissing,
    HookInstalled,
    HookOutdated,
    HookOk,
    InstallFailed,
    ConfigParseFailed,
    BackupFailed,
    MergeConflict,
    WriteFailed,
    VerifyFailed,
    RollbackAvailable,
    RollbackFailed,
    PermissionDenied,
    HookTampered,
}

#[derive(Debug, Clone)]
pub struct HookInstallPaths {
    pub app_dir: PathBuf,
    pub hook_install_dir: PathBuf,
    pub emitter_install_dir: PathBuf,
    pub installed_hook: PathBuf,
    pub installed_emitter: PathBuf,
    pub manifest: PathBuf,
    pub backups_dir: PathBuf,
    pub claude_settings: PathBuf,
    pub codex_hooks: PathBuf,
    pub codex_config: PathBuf,
    pub source_hook: PathBuf,
}

impl HookInstallPaths {
    pub fn from_env() -> Self {
        let app_dir = agent_notify_dir();
        let hook_install_dir = app_dir.join("hooks");
        let emitter_install_dir = app_dir.join("bin");
        let user_profile = env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        Self {
            installed_hook: hook_install_dir.join("agent-notify-hook.ps1"),
            installed_emitter: emitter_install_dir.join(agent_notify_exe_name()),
            manifest: hook_install_dir.join("manifest.json"),
            hook_install_dir,
            emitter_install_dir,
            backups_dir: app_dir.join("backups"),
            claude_settings: user_profile.join(".claude").join("settings.json"),
            codex_hooks: user_profile.join(".codex").join("hooks.json"),
            codex_config: user_profile.join(".codex").join("config.toml"),
            source_hook: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("scripts")
                .join("hooks")
                .join("agent-notify-hook.ps1"),
            app_dir,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookInstallReport {
    pub claude_cli: HookStatus,
    pub codex_cli: HookStatus,
    pub codex_hooks_feature: HookStatus,
    pub claude: HookStatus,
    pub codex: HookStatus,
    pub hook_script: HookStatus,
    pub installed_hook: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_emitter: Option<String>,
    pub manifest: String,
}

pub fn install_or_repair(paths: &HookInstallPaths) -> Result<HookInstallReport, HookManagerError> {
    let claude_cli = check_cli("claude", &["--version"]);
    let codex_cli = check_cli("codex", &["--version"]);
    let codex_hooks_feature = check_codex_hooks_feature();

    fs::create_dir_all(&paths.hook_install_dir)?;
    fs::create_dir_all(&paths.emitter_install_dir)?;
    fs::create_dir_all(&paths.backups_dir)?;
    let emitter_hash = install_emitter(paths)?;
    install_hook_script(paths, emitter_hash.as_deref())?;

    let command_template = hook_command_template();
    let installed_hook = paths.installed_hook.display().to_string();
    let command = command_template.replace("{hook}", &installed_hook);

    let claude = install_json_hooks(
        "claude",
        &paths.claude_settings,
        &paths.backups_dir,
        &claude_template(&command),
    )?;
    let codex = install_json_hooks(
        "codex",
        &paths.codex_hooks,
        &paths.backups_dir,
        &codex_template(&command),
    )?;
    enable_codex_hooks_feature(&paths.codex_config, &paths.backups_dir)?;

    Ok(HookInstallReport {
        claude_cli,
        codex_cli,
        codex_hooks_feature,
        claude,
        codex,
        hook_script: HookStatus::HookOk,
        installed_hook,
        installed_emitter: emitter_hash.map(|_| paths.installed_emitter.display().to_string()),
        manifest: paths.manifest.display().to_string(),
    })
}

pub fn check_cli(name: &str, args: &[&str]) -> HookStatus {
    match run_command(name, args) {
        Ok(output) if output.status.success() => HookStatus::HookOk,
        Ok(_) => HookStatus::UnsupportedVersion,
        Err(error) if error.kind() == io::ErrorKind::NotFound => HookStatus::MissingCli,
        Err(_) => HookStatus::InstallFailed,
    }
}

pub fn check_codex_hooks_feature() -> HookStatus {
    match run_command("codex", &["features", "list"]) {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
            if stdout.contains("hooks")
                && (stdout.contains("enabled")
                    || stdout.contains("stable")
                    || stdout.contains("true"))
            {
                HookStatus::HookOk
            } else {
                HookStatus::UnsupportedVersion
            }
        }
        Ok(_) => HookStatus::UnsupportedVersion,
        Err(error) if error.kind() == io::ErrorKind::NotFound => HookStatus::MissingCli,
        Err(_) => HookStatus::InstallFailed,
    }
}

fn run_command(name: &str, args: &[&str]) -> io::Result<Output> {
    match Command::new(name).args(args).output() {
        Ok(output) => Ok(output),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            run_resolved_windows_command(name, args).unwrap_or(Err(error))
        }
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn run_resolved_windows_command(name: &str, args: &[&str]) -> Option<io::Result<Output>> {
    let output = Command::new("where.exe").arg(name).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let path = PathBuf::from(line);
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let result = match extension.as_str() {
            "exe" => Command::new(&path).args(args).output(),
            "cmd" | "bat" => Command::new("cmd")
                .arg("/D")
                .arg("/C")
                .arg(&path)
                .args(args)
                .output(),
            "ps1" => Command::new("powershell")
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(&path)
                .args(args)
                .output(),
            _ => continue,
        };
        if result.is_ok() {
            return Some(result);
        }
    }
    None
}

#[cfg(not(windows))]
fn run_resolved_windows_command(_name: &str, _args: &[&str]) -> Option<io::Result<Output>> {
    None
}

fn install_hook_script(
    paths: &HookInstallPaths,
    emitter_hash: Option<&str>,
) -> Result<(), HookManagerError> {
    let source_bytes = source_hook_bytes(paths)?;
    let installed_bytes = with_utf8_bom(source_bytes);
    let script_hash = sha256_hex(&installed_bytes);
    let current_hash = fs::read(&paths.installed_hook)
        .ok()
        .map(|bytes| sha256_hex(&bytes));
    if current_hash.as_deref() != Some(&script_hash) {
        fs::write(&paths.installed_hook, &installed_bytes)?;
    }

    let manifest = json!({
        "managedBy": MANAGED_BY,
        "hookVersion": HOOK_VERSION,
        "installedAt": Utc::now().to_rfc3339(),
        "scriptSha256": script_hash,
        "emitterPath": paths.installed_emitter.display().to_string(),
        "emitterSha256": emitter_hash,
        "supportedEvents": {
            "claude": CLAUDE_EVENTS,
            "codex": CODEX_EVENTS
        }
    });
    write_json_atomic(&paths.manifest, &manifest)?;
    Ok(())
}

fn source_hook_bytes(paths: &HookInstallPaths) -> Result<Vec<u8>, HookManagerError> {
    match fs::read(&paths.source_hook) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(EMBEDDED_HOOK_SCRIPT.to_vec()),
        Err(error) => Err(HookManagerError::SourceHookRead(
            paths.source_hook.clone(),
            error,
        )),
    }
}

fn install_emitter(paths: &HookInstallPaths) -> Result<Option<String>, HookManagerError> {
    let Some(source) = resolve_source_emitter() else {
        return Ok(None);
    };
    install_emitter_from_path(&source, &paths.installed_emitter)
        .map(Some)
        .map_err(HookManagerError::Io)
}

fn resolve_source_emitter() -> Option<PathBuf> {
    if let Some(path) = env::var_os("AGENT_NOTIFY_EMITTER")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Some(path);
    }

    let sibling = env::current_exe()
        .ok()?
        .with_file_name(agent_notify_exe_name());
    sibling.is_file().then_some(sibling)
}

fn install_emitter_from_path(source: &Path, destination: &Path) -> io::Result<String> {
    let bytes = fs::read(source)?;
    let hash = sha256_hex(&bytes);
    if source.canonicalize().ok() == destination.canonicalize().ok() {
        return Ok(hash);
    }

    let current_hash = fs::read(destination).ok().map(|bytes| sha256_hex(&bytes));
    if current_hash.as_deref() != Some(&hash) {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(destination, &bytes)?;
    }
    Ok(hash)
}

fn agent_notify_exe_name() -> &'static str {
    if cfg!(windows) {
        "agent-notify.exe"
    } else {
        "agent-notify"
    }
}

fn with_utf8_bom(mut bytes: Vec<u8>) -> Vec<u8> {
    const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";
    if bytes.starts_with(UTF8_BOM) {
        return bytes;
    }

    let mut with_bom = Vec::with_capacity(UTF8_BOM.len() + bytes.len());
    with_bom.extend_from_slice(UTF8_BOM);
    with_bom.append(&mut bytes);
    with_bom
}

fn install_json_hooks(
    tool: &str,
    path: &Path,
    backups_dir: &Path,
    managed_template: &Value,
) -> Result<HookStatus, HookManagerError> {
    let existing = if path.exists() {
        let text = fs::read_to_string(path)?;
        serde_json::from_str(&text).map_err(|_| HookManagerError::ConfigParseFailed(path.into()))?
    } else {
        json!({})
    };
    let merged = merge_managed_hooks(existing.clone(), managed_template.clone())?;
    if merged == existing {
        return Ok(HookStatus::HookOk);
    }
    backup_if_exists(tool, path, backups_dir).map_err(|_| HookManagerError::BackupFailed)?;
    write_json_atomic(path, &merged)?;
    Ok(HookStatus::HookInstalled)
}

pub fn merge_managed_hooks(
    mut existing: Value,
    managed_template: Value,
) -> Result<Value, HookManagerError> {
    let existing_object = existing
        .as_object_mut()
        .ok_or(HookManagerError::ConfigParse)?;
    let hooks_value = existing_object.entry("hooks").or_insert_with(|| json!({}));
    let hooks_object = hooks_value
        .as_object_mut()
        .ok_or(HookManagerError::ConfigParse)?;
    let template_hooks = managed_template
        .get("hooks")
        .and_then(Value::as_object)
        .ok_or(HookManagerError::ConfigParse)?;

    for (event_name, template_groups) in template_hooks {
        let groups = hooks_object
            .entry(event_name.clone())
            .or_insert_with(|| json!([]));
        let group_array = groups.as_array_mut().ok_or(HookManagerError::ConfigParse)?;
        remove_managed_hooks(group_array)?;
        reject_unmanaged_same_command(group_array, template_groups)?;
        if let Some(template_array) = template_groups.as_array() {
            group_array.extend(template_array.iter().cloned());
        } else {
            return Err(HookManagerError::ConfigParse);
        }
    }

    Ok(existing)
}

fn remove_managed_hooks(groups: &mut Vec<Value>) -> Result<(), HookManagerError> {
    for group in groups.iter_mut() {
        let hooks = group
            .get_mut("hooks")
            .and_then(Value::as_array_mut)
            .ok_or(HookManagerError::ConfigParse)?;
        hooks.retain(|hook| !is_agent_notify_hook(hook));
    }
    groups.retain(|group| {
        group
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|hooks| !hooks.is_empty())
    });
    Ok(())
}

fn reject_unmanaged_same_command(
    groups: &[Value],
    template_groups: &Value,
) -> Result<(), HookManagerError> {
    let managed_commands = template_groups
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|group| {
            group
                .get("hooks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|hook| hook.get("command").and_then(Value::as_str))
        .collect::<Vec<_>>();

    for group in groups {
        if let Some(hooks) = group.get("hooks").and_then(Value::as_array) {
            for hook in hooks {
                let Some(command) = hook.get("command").and_then(Value::as_str) else {
                    continue;
                };
                let same_command = managed_commands.contains(&command)
                    || command.contains("agent-notify-hook.ps1");
                if same_command && !is_agent_notify_hook(hook) {
                    return Err(HookManagerError::MergeConflict);
                }
            }
        }
    }
    Ok(())
}

fn is_agent_notify_hook(hook: &Value) -> bool {
    hook.get("managedBy").and_then(Value::as_str) == Some(MANAGED_BY)
        || hook
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id.starts_with("agent-notify-"))
        || is_legacy_agent_notify_hook(hook)
}

fn is_legacy_agent_notify_hook(hook: &Value) -> bool {
    hook.get("statusMessage").and_then(Value::as_str) == Some("Agent Notify")
        && hook
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| command.contains("agent-notify-hook.ps1"))
}

fn enable_codex_hooks_feature(path: &Path, backups_dir: &Path) -> Result<(), HookManagerError> {
    let document = if path.exists() {
        fs::read_to_string(path)?
            .parse::<DocumentMut>()
            .map_err(|_| HookManagerError::ConfigParseFailed(path.into()))?
    } else {
        DocumentMut::new()
    };
    let mut document = document;
    if !document.as_table().contains_key("features") {
        document["features"] = Item::Table(Table::new());
    }
    if !document["features"].is_table() {
        return Err(HookManagerError::ConfigParseFailed(path.into()));
    }
    let features = document["features"]
        .as_table_mut()
        .ok_or_else(|| HookManagerError::ConfigParseFailed(path.into()))?;
    let hooks_enabled = features.get("hooks").and_then(Item::as_bool) == Some(true);
    let has_deprecated_key = features.contains_key("codex_hooks");
    if hooks_enabled && !has_deprecated_key {
        return Ok(());
    }
    backup_if_exists("codex", path, backups_dir).map_err(|_| HookManagerError::BackupFailed)?;
    features.insert("hooks", value(true));
    features.remove("codex_hooks");
    write_text_atomic(path, &document.to_string())?;
    Ok(())
}

fn claude_template(command: &str) -> Value {
    let events = CLAUDE_EVENTS.iter().map(|event| {
        let matcher = matches!(*event, "PermissionRequest" | "PostToolUseFailure").then_some("*");
        (
            (*event).to_string(),
            json!([managed_group("claude", event, matcher, command)]),
        )
    });
    json!({ "hooks": events.collect::<serde_json::Map<_, _>>() })
}

fn codex_template(command: &str) -> Value {
    let events = CODEX_EVENTS.iter().map(|event| {
        let matcher =
            matches!(*event, "SessionStart" | "PermissionRequest" | "PostToolUse").then_some("*");
        (
            (*event).to_string(),
            json!([managed_group("codex", event, matcher, command)]),
        )
    });
    json!({ "hooks": events.collect::<serde_json::Map<_, _>>() })
}

fn managed_group(tool: &str, event: &str, matcher: Option<&str>, command: &str) -> Value {
    let command = command.replace("{tool}", tool).replace("{event}", event);
    let mut group = json!({
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": HOOK_COMMAND_TIMEOUT_SECONDS,
            "statusMessage": "Agent Notify",
            "managedBy": MANAGED_BY,
            "id": format!("agent-notify-{tool}-{event}")
        }]
    });
    if let Some(matcher) = matcher {
        group["matcher"] = json!(matcher);
    }
    group
}

fn hook_command_template() -> String {
    "powershell -NoProfile -ExecutionPolicy Bypass -File \"{hook}\" --tool {tool} --hook-event {event}".to_string()
}

fn backup_if_exists(tool: &str, path: &Path, backups_dir: &Path) -> io::Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    let hash = sha256_hex(&bytes);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    let backup_dir = backups_dir.join(tool);
    fs::create_dir_all(&backup_dir)?;
    let backup = backup_dir.join(format!(
        "{}-{}-{}.bak",
        Utc::now().format("%Y%m%d-%H%M%S"),
        file_name,
        &hash[..12]
    ));
    fs::write(&backup, bytes)?;
    Ok(Some(backup))
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<(), HookManagerError> {
    let text = serde_json::to_string_pretty(value)?;
    serde_json::from_str::<Value>(&text)?;
    write_text_atomic(path, &text)
}

fn write_text_atomic(path: &Path, text: &str) -> Result<(), HookManagerError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[derive(Debug, thiserror::Error)]
pub enum HookManagerError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("source hook {0} could not be read: {1}")]
    SourceHookRead(PathBuf, io::Error),
    #[error("configuration parse failed")]
    ConfigParse,
    #[error("configuration parse failed for {0}")]
    ConfigParseFailed(PathBuf),
    #[error("backup failed")]
    BackupFailed,
    #[error("merge conflict")]
    MergeConflict,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_user_hook_and_installs_managed_hook() {
        let existing = json!({
            "hooks": {
                "Stop": [{
                    "hooks": [{
                        "type": "command",
                        "command": "powershell -File user.ps1"
                    }]
                }]
            }
        });
        let merged = merge_managed_hooks(
            existing,
            claude_template("powershell -File hook.ps1 --tool {tool} --hook-event {event}"),
        )
        .unwrap();
        let hooks = merged["hooks"]["Stop"].as_array().unwrap();

        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0]["hooks"][0]["command"], "powershell -File user.ps1");
        assert_eq!(hooks[1]["hooks"][0]["managedBy"], MANAGED_BY);
    }

    #[test]
    fn replaces_existing_managed_hook_idempotently() {
        let template =
            claude_template("powershell -File new.ps1 --tool {tool} --hook-event {event}");
        let once = merge_managed_hooks(json!({}), template.clone()).unwrap();
        let twice = merge_managed_hooks(once.clone(), template).unwrap();

        assert_eq!(once, twice);
    }

    #[test]
    fn detects_unmanaged_same_command_conflict() {
        let existing = json!({
            "hooks": {
                "Stop": [{
                    "hooks": [{
                        "type": "command",
                        "command": "powershell -File agent-notify-hook.ps1 --tool claude --hook-event Stop"
                    }]
                }]
            }
        });
        let result = merge_managed_hooks(
            existing,
            claude_template(
                "powershell -File agent-notify-hook.ps1 --tool {tool} --hook-event {event}",
            ),
        );

        assert!(matches!(result, Err(HookManagerError::MergeConflict)));
    }

    #[test]
    fn adopts_legacy_agent_notify_hook_without_managed_marker() {
        let existing = json!({
            "hooks": {
                "Stop": [{
                    "hooks": [{
                        "type": "command",
                        "command": "powershell -File agent-notify-hook.ps1 --tool claude --hook-event Stop",
                        "statusMessage": "Agent Notify"
                    }]
                }]
            }
        });
        let merged = merge_managed_hooks(
            existing,
            claude_template(
                "powershell -File agent-notify-hook.ps1 --tool {tool} --hook-event {event}",
            ),
        )
        .unwrap();
        let hooks = merged["hooks"]["Stop"].as_array().unwrap();

        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["hooks"][0]["managedBy"], MANAGED_BY);
    }

    #[test]
    fn enables_codex_hooks_feature_without_removing_existing_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[model]\nname = \"gpt\"\n").unwrap();

        enable_codex_hooks_feature(&path, dir.path()).unwrap();
        let text = fs::read_to_string(path).unwrap();

        assert!(text.contains("[model]"));
        assert!(text.contains("hooks = true"));
        assert!(!text.contains("codex_hooks"));
    }

    #[test]
    fn migrates_deprecated_codex_hooks_feature_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[features]\ncodex_hooks = true\n").unwrap();

        enable_codex_hooks_feature(&path, dir.path()).unwrap();
        let text = fs::read_to_string(path).unwrap();

        assert!(text.contains("hooks = true"));
        assert!(!text.contains("codex_hooks"));
    }

    #[test]
    fn installs_emitter_binary_when_source_available() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join(agent_notify_exe_name());
        let destination = dir
            .path()
            .join("runtime")
            .join("bin")
            .join(agent_notify_exe_name());
        fs::write(&source, b"agent-notify test emitter").unwrap();

        let hash = install_emitter_from_path(&source, &destination).unwrap();

        assert_eq!(
            fs::read(&destination).unwrap(),
            b"agent-notify test emitter"
        );
        assert_eq!(hash, sha256_hex(b"agent-notify test emitter"));
    }

    #[test]
    fn managed_hooks_use_runtime_safe_timeout() {
        let template =
            codex_template("powershell -File hook.ps1 --tool {tool} --hook-event {event}");
        let timeout = template["hooks"]["Stop"][0]["hooks"][0]["timeout"]
            .as_u64()
            .unwrap();

        assert_eq!(timeout, HOOK_COMMAND_TIMEOUT_SECONDS);
        assert!(timeout > 2);
    }

    #[test]
    fn source_hook_falls_back_to_embedded_script() {
        let dir = tempfile::tempdir().unwrap();
        let app_dir = dir.path().join("app");
        let paths = HookInstallPaths {
            installed_hook: app_dir.join("hooks").join("agent-notify-hook.ps1"),
            installed_emitter: app_dir.join("bin").join(agent_notify_exe_name()),
            manifest: app_dir.join("hooks").join("manifest.json"),
            hook_install_dir: app_dir.join("hooks"),
            emitter_install_dir: app_dir.join("bin"),
            backups_dir: app_dir.join("backups"),
            claude_settings: dir.path().join(".claude").join("settings.json"),
            codex_hooks: dir.path().join(".codex").join("hooks.json"),
            codex_config: dir.path().join(".codex").join("config.toml"),
            source_hook: dir.path().join("missing-agent-notify-hook.ps1"),
            app_dir,
        };

        let bytes = source_hook_bytes(&paths).unwrap();

        assert!(String::from_utf8_lossy(&bytes).contains("New-AgentNotifyEvent"));
    }

    #[test]
    fn installed_hook_bytes_include_utf8_bom() {
        let bytes = with_utf8_bom("中文通知".as_bytes().to_vec());

        assert!(bytes.starts_with(b"\xEF\xBB\xBF"));
        assert_eq!(&bytes[3..], "中文通知".as_bytes());
    }
}
