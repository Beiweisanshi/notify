# Repository Guidelines

## Project Structure & Module Organization

This repository contains a Rust MVP for a Windows notification tool for Claude Code, Codex CLI, and other long-running terminal tasks.

- `PLAN.md` contains the product design, event model, adapter strategy, and phased implementation plan.
- `src/agent-notify-core/` contains the shared event model, config, adapter, redaction, dedupe, and notification policy code.
- `src/agent-notify/` contains the hook-facing CLI. Current MVP supports `agent-notify emit --stdin`.
- `src/agent-notify-tray/` currently contains the Axum localhost backend, not a completed Tauri tray UI.
- `src/hook-manager/` installs and repairs Claude/Codex user-level hooks.
- `scripts/hooks/` contains the PowerShell hook and JSON reference templates.
- Current tests are inline Rust unit tests. Add integration tests under `tests/` when cross-process or end-to-end coverage is introduced.
- Keep generated binaries, logs, local settings, and build output out of Git.

## Build, Test, and Development Commands

```powershell
git status --short --branch
git log --oneline --decorate -5
```

Standard Rust checks:

```powershell
cargo fmt --check
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo build --workspace
```

On machines without MSVC `link.exe`, use the GNU Windows toolchain with an ASCII target directory:

```powershell
$env:CARGO_TARGET_DIR = "D:\own\notify-target"
cargo +stable-x86_64-pc-windows-gnu test --workspace
```

Run the backend locally with:

```powershell
cargo run -p agent-notify-tray -- serve
cargo run -p agent-notify-tray -- check-hooks
cargo run -p agent-notify-tray -- repair-hooks
```

When the planned Tauri frontend is added, document its package manager scripts in `README.md`.

## Coding Style & Naming Conventions

For Markdown, use concise headings, fenced code blocks with language tags, and short actionable paragraphs. Keep architecture terms consistent with `PLAN.md`, especially `agent-notify-tray`, `agent-notify`, `sessionId`, and `eventType`. Treat `agentrun`, deep-link click handling, and full Tauri tray UI as planned features unless the code implements them.

For Rust code, follow `rustfmt` defaults and use snake_case for functions/modules. For future TypeScript/Tauri UI code, use 2-space indentation and PascalCase component names. For PowerShell, use approved verbs, PascalCase function names, and clear parameter names.

## Testing Guidelines

Unit tests should cover event parsing, event-to-notification mapping, deduplication, config loading, hook config merging, and CLI failure behavior. Integration tests should verify localhost event submission and window-focus behavior where feasible.

Use descriptive test names such as `parses_claude_hook_payload_as_confirmation_required` or `suppresses_duplicate_events_within_window`.

## Commit & Pull Request Guidelines

Current history uses Conventional Commit style, for example:

```text
docs: add notification tool plan
```

Continue using short, scoped messages such as `docs: update event model`, `feat: add notify daemon`, or `test: cover event dedupe`.

Pull requests should include a clear summary, verification steps, linked issues if any, and screenshots or screen recordings for notification UI changes. Call out Windows-specific behavior and any Claude/Codex version assumptions.

## Security & Configuration Tips

Do not auto-approve Claude or Codex permission prompts. Do not include secrets, full terminal transcripts, or sensitive command arguments in notifications by default. Keep local ports, startup registration, and notification display settings configurable.
