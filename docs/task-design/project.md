# Task Design Project Notes

## Update 2026-05-11 - Floating Status Bar Planning

Current project state:

- `AGENTS.md` exists and matches the current Rust MVP structure: `agent-notify-core`, `agent-notify`, `agent-notify-tray`, and `hook-manager`.
- `agent-notify-tray` is currently a Rust/Axum localhost backend with Windows Toast, protocol activation, session state, hook repair, and a native tray exit menu.
- Full Tauri UI, session detail UI, runtime listener toggle UI, and a persistent desktop status surface are not implemented.
- Existing HTTP routes include authenticated `GET /sessions`, `POST /focus/{sessionId}`, and `POST /activate/{activationId}`.
- Current session state is in memory and does not track whether a completed notification has been clicked or dismissed.

Planning conventions:

- Planning documents live under `docs/task-design`.
- Requirement files describe user-visible behavior and boundaries.
- Module files describe implementation-facing design without modifying implementation code.
- HTML prototypes under `docs/task-design/prototypes` are design artifacts, not production code.
- Current prototype: `docs/task-design/prototypes/floating-status-bar.html`.

AGENTS.md consistency result:

- Consistent enough for this planning task.
- The repo currently has no `docs/task-design` directory, so this update initializes the planning tree.

## Update 2026-05-11 - Integration Result

### Modules Completed

- `floating-status-bar`: implemented the backend-facing contract for floating status bar snapshots, persisted bar state, unopened completed notification tracking, and focus-time click marking.

### Whole-Project Verification

- `cargo fmt --check`: passed.
- `cargo test --workspace --ignore-rust-version`: passed.
- `cargo clippy --workspace --ignore-rust-version -- -D warnings`: passed.
- `cargo build --workspace --ignore-rust-version`: passed.
- `git diff --check`: passed, with only Git CRLF normalization warnings.

The machine's default Rust toolchain is rustc 1.94.0, below the workspace `rust-version = "1.95"`, so verification used `--ignore-rust-version`.

### Final Review

- `/review` was unavailable in this execution surface.
- Local final review completed; fixed partial state-file default handling, `hwnd: 0` best-effort fallback, and clippy findings.

### Remaining Risk

- The actual desktop floating window, drag/hide/hotkey behavior, and bar-anchored notification UI still require a Tauri shell.
- Notification click/open state is in memory and resets with the backend process.

## Update 2026-05-11 - Desktop Floating UI Integration Result

### Modules Completed

- `floating-status-bar`: completed the first visible desktop implementation as a native Win32 always-on-top floating window in `agent-notify-tray`.

### Integration Result

- The desktop surface now starts with `agent-notify-tray serve`.
- The floating bar supports compact, expanded, and hidden-strip states.
- The bar can be restored with `Ctrl + Shift + Space`.
- New task notifications render adjacent to the floating bar instead of relying only on screen-corner Toast.
- Completed notification chooser and bar notification clicks use the existing authenticated focus path and mark notifications clicked only after focus succeeds.

### Verification

- `cargo fmt --check`: passed.
- `cargo build -p agent-notify-tray --ignore-rust-version`: passed.
- `cargo test -p agent-notify-tray --ignore-rust-version`: passed.
- `cargo clippy -p agent-notify-tray --ignore-rust-version -- -D warnings`: passed.
- Runtime smoke checks passed for `GET /floating-status`, `PUT /floating-status/state`, visible Win32 window creation, compact/expanded/hidden dimensions, bar-anchored notification display, focus-time click marking, and `Ctrl + Shift + Space` restore.

### Review

- `/review` was unavailable in this execution surface.
- Local review found and fixed a model-lock deadlock, cross-thread Win32 geometry update risk, paint-time lock contention, and the clippy `too_many_arguments` finding in text drawing.

### Remaining Risk

- This remains a native Win32 implementation, not Tauri, because the repository still lacks a Tauri/frontend workspace.
- Notification click/open state remains process-memory only.

## Update 2026-05-11 - Tauri Floating UI Integration Result

### Modules Completed

- `floating-status-bar`: superseded the rejected native Win32 floating-bar implementation with a Tauri v2 desktop UI and headless backend startup path.

### Integration Result

- The visible desktop floating bar now runs as `agent-notify-desktop.exe` from the Tauri shell.
- Tauri starts the existing backend as `agent-notify-tray.exe serve --no-native-tray`, so the old native tray/floating UI path is not used by the desktop UI.
- The Tauri bar implements compact, expanded, hidden-strip, drag position persistence, `Ctrl + Shift + Space` restore, count chips, completed notification chooser, and bar-anchored notifications.
- Notification clicks call the authenticated `/focus/{sessionId}` path with `notificationId`; notifications are marked clicked only after focus succeeds.
- Windows Toast remains only as a fallback when the Tauri floating UI heartbeat is absent.

### Whole-Project Verification

- `npm install`: passed.
- `npm run typecheck`: passed.
- `npm run build:ui`: passed.
- `cargo fmt --check`: passed.
- `cargo test --workspace --ignore-rust-version`: passed.
- `cargo clippy --workspace --ignore-rust-version -- -D warnings`: passed.
- `cargo build --workspace --ignore-rust-version`: passed.
- `npm run build`: passed.
- `git diff --check`: passed with only Git CRLF normalization warnings.

The machine's default Rust toolchain is rustc 1.94.0, below the workspace `rust-version = "1.95"`, so cargo verification used `--ignore-rust-version`.

### Runtime Verification

- Started `D:\own\notify\target\release\agent-notify-desktop.exe`.
- Confirmed one visible `Tauri Window` for `Agent Notify`.
- Confirmed the backend command line is `agent-notify-tray.exe serve --no-native-tray`.
- Confirmed no old `AgentNotifyFloatingStatusBar` window is present.
- Confirmed `GET /floating-status` returns a valid authenticated snapshot.
- Confirmed `Ctrl + Shift + Space` restores the hidden bar.
- Confirmed completed event notification panel appears from the Tauri floating bar.
- Confirmed focus with `notificationId` returns `focused: true` and marks the notification clicked.
- Confirmed backend restart from Tauri does not create duplicate backend processes after startup throttling.

### Final Review

- `/review` was unavailable in this execution surface.
- Two parallel read-only explorer reviews found issues in focus result handling, Win32 tray contamination, DPI/window geometry, notification anchoring, Toast activation marking, and backend duplicate spawn behavior.
- Fixed all in-scope findings listed above.

### Remaining Risk

- Notification click/open state remains process-memory only.
- Tauri login auto-start is not implemented, so hook events still require the Tauri app or backend to be running first.

## Update 2026-05-11 - Focus Target Accuracy Integration

### Integration Result

- Notification clicks now prefer the process/window snapshot captured when the notification was created.
- Stored HWNDs are validated against the expected terminal/window identity before focus, avoiding stale or unrelated fixed-window activation.
- Hook-generated fallback sessions now include process and terminal-window identity, reducing Claude/Codex session merging when official payloads do not provide a session id.

### Verification

- `cargo fmt --check`: passed.
- `cargo test -p agent-notify-core --ignore-rust-version`: passed.
- `cargo test -p agent-notify-tray --ignore-rust-version`: passed.
- `cargo test --workspace --ignore-rust-version`: passed.
- `cargo clippy -p agent-notify-core --ignore-rust-version -- -D warnings`: passed.
- `cargo clippy -p agent-notify-tray --ignore-rust-version -- -D warnings`: passed.
- `cargo clippy --workspace --ignore-rust-version -- -D warnings`: passed.
- `cargo build --workspace --ignore-rust-version`: passed after stopping the old locked backend process.
- `cargo build -p agent-notify-tray -p agent-notify --release --ignore-rust-version`: passed.
- `npm run typecheck`: passed.
- `npm run build`: passed.
- `git diff --check`: passed with only Git CRLF normalization warnings.
- PowerShell parser and hook event construction smoke checks passed.
- Runtime hook repair and release Tauri startup passed.
