use agent_notify_core::{AgentEvent, load_or_create_config, read_token};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::time::Duration;
use tokio::io::{self, AsyncReadExt};

#[derive(Debug, Parser)]
#[command(name = "agent-notify")]
#[command(about = "Send AgentNotify hook events to the local notification backend.")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Emit(EmitArgs),
}

#[derive(Debug, Parser)]
struct EmitArgs {
    #[arg(long)]
    stdin: bool,
    #[arg(long, env = "AGENT_NOTIFY_ENDPOINT")]
    endpoint: Option<String>,
    #[arg(long, env = "AGENT_NOTIFY_TOKEN")]
    token: Option<String>,
    #[arg(long, default_value_t = 1500)]
    timeout_ms: u64,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await
        && std::env::var_os("AGENT_NOTIFY_STRICT").is_some()
    {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Emit(args) => emit(args).await,
    }
}

async fn emit(args: EmitArgs) -> Result<()> {
    if !args.stdin {
        anyhow::bail!("only `agent-notify emit --stdin` is supported in the MVP");
    }
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .await
        .context("failed to read stdin")?;
    if input.trim().is_empty() {
        return Ok(());
    }
    let input = input.trim_start_matches('\u{feff}');
    let event: AgentEvent = serde_json::from_str(input).context("invalid event json")?;
    event.validate().context("invalid event")?;

    let config = load_or_create_config().context("failed to load config")?;
    let endpoint = args.endpoint.unwrap_or_else(|| config.endpoint("/events"));
    let token = match args.token {
        Some(token) => token,
        None => read_token(&config).context("failed to read token")?,
    };
    let timeout = Duration::from_millis(args.timeout_ms);
    let client = reqwest::Client::builder().timeout(timeout).build()?;
    let result = client
        .post(endpoint)
        .bearer_auth(token)
        .json(&event)
        .send()
        .await;

    match result {
        Ok(response) if response.status().is_success() => Ok(()),
        Ok(_) | Err(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_utf8_bom_from_stdin_json() {
        let input = "\u{feff}{\"version\":1,\"eventId\":\"e1\",\"eventType\":\"heartbeat\",\"severity\":\"info\",\"tool\":\"codex\",\"sessionId\":\"s1\",\"project\":{\"cwd\":\"D:\\\\repo\",\"name\":\"repo\"},\"message\":{\"title\":\"Codex status\",\"body\":\"repo s1 status\"}}";
        let parsed: AgentEvent =
            serde_json::from_str(input.trim_start_matches('\u{feff}')).unwrap();

        assert_eq!(parsed.event_id, "e1");
    }
}
