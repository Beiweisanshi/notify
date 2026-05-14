# Floating Status Bar Design

## Update 2026-05-11 - Initial module design

### Purpose

Provide a small persistent desktop bar that summarizes terminal activity and gives a direct path back into completed sessions.

### Boundary

This module covers:

- Desktop floating bar window.
- Drag-to-position behavior.
- Hide/show behavior.
- Hotkey summon behavior.
- Compact session counters.
- Completed-session chooser flow.
- Click-through activation to the existing backend/session state.

This module does not cover:

- Hook ingestion.
- Toast generation.
- Token storage.
- Terminal process discovery.
- Backend focus heuristics.

### Proposed UX structure

- Compact bar:
  - App name or icon.
  - Total sessions.
  - Running count.
  - Completed count.
  - Hide button.
  - Menu or icon to expand.
- Expanded panel:
  - Compact counters remain pinned at the top.
  - Running sessions list.
  - Completed sessions list.
  - “New/unopened notifications” indicator on completed rows.
  - Click on a completed row opens a chooser of unclicked notifications for that session.
- Notification chooser:
  - Shows the unclicked notifications only.
  - Lets the user pick one activation target.
  - Opens the backend activation path for that target.

### Recommended architecture

Implement this as a future frameless always-on-top Tauri window in the same app family as `agent-notify-tray`, not as a separate process that owns its own backend state.

Recommended first version:

- One small frameless window for the bar.
- Optional expanded panel in the same window.
- Global shortcut registration inside the Tauri shell.
- Position and hidden state persisted under the existing AgentNotify runtime home.
- Backend remains the source of truth for sessions, notification state, auth token, and focus.

The current Rust backend can keep serving localhost HTTP during the transition. The UI should call the backend through an authenticated local channel; do not put the Bearer token or activation IDs in visible UI text.

### Count definitions

Use explicit count rules so the bar does not drift from user expectation:

- `terminalCount`: sessions with a currently valid terminal window or live process.
- `runningCount`: valid terminal sessions whose status is `running`.
- `completedCount`: valid terminal sessions whose latest status is `completed`.
- `unopenedCompletedNotifications`: unclicked notification records attached to completed sessions.

Do not count stale sessions whose HWND no longer exists. If exact liveness is unavailable in the first implementation, mark counts as best-effort and expire stale sessions after a short idle window.

Failed, blocked, and waiting-user sessions should not be silently folded into completed in the first pass. They can keep using Toast and later get their own badge if needed.

### Data and interaction model

The bar needs a read-only snapshot from the backend, likely derived from the existing session list plus an added local UI state for click tracking.

Required UI-facing state:

- `totalSessions`
- `runningSessions`
- `completedSessions`
- `unreadCompletedNotifications`
- `hidden`
- `position`
- `expanded`
- `hotkey`

The completed-session chooser needs a per-notification state, at minimum:

- notification id
- session id
- event type
- time
- clicked/unread flag
- activation target

Suggested UI DTO:

```json
{
  "summary": {
    "terminalCount": 5,
    "runningCount": 2,
    "completedCount": 3,
    "unopenedCompletedNotifications": 4
  },
  "sessions": [
    {
      "sessionId": "codex-notify-ui",
      "tool": "codex",
      "projectName": "notify-ui",
      "status": "running",
      "window": {
        "hwnd": 6294472,
        "title": "notify-ui"
      },
      "unopenedNotificationCount": 0
    }
  ],
  "notifications": [
    {
      "notificationId": "event-20260511-001",
      "sessionId": "codex-docs",
      "eventType": "task.completed",
      "title": "Codex 任务完成",
      "body": "docs · 点击返回终端",
      "createdAt": "2026-05-11T14:30:00+08:00",
      "clickedAt": null
    }
  ]
}
```

### State machine

Bar state:

- `visible.compact`: default small bar, shows counts only.
- `visible.expanded`: counts plus running/completed lists.
- `hidden`: no full bar, only hotkey can restore; an optional small edge tab is acceptable.

Chooser state:

- `closed`
- `open.allCompleted`: opened from the completed count chip.
- `open.sessionCompleted`: opened from one completed session row.
- `activating`: user selected a notification; UI marks it as clicked and asks backend to focus.
- `failed`: backend could not focus; UI keeps the notification visible or shows retry depending on returned error.

### Click flow

1. User clicks the completed count chip or a completed session row.
2. UI requests unclicked completed notifications from the local backend snapshot.
3. UI shows only notifications whose `clickedAt` is null.
4. User selects one notification.
5. UI marks the notification as clicked optimistically only after the backend accepts the focus request.
6. Backend focuses the target terminal by `sessionId` using the existing HWND/PID/title order.
7. If focus fails, the UI keeps the chooser open and shows the failed row as retryable.

Preferred backend call from this UI:

```text
POST /focus/{sessionId}
Authorization: Bearer <token from local runtime config>
```

The existing `/activate/{activationId}` route should remain for Windows Toast protocol activation. The floating bar is an authenticated local UI and should not depend on short-lived protocol activation IDs for older completed notifications.

### Window behavior

- Default compact size target: about `320-380px` wide and `40-56px` tall.
- Expanded size target: same width, about `260-340px` tall.
- Border radius should stay at or below 8px.
- Dragging should clamp the bar inside the current monitor work area.
- Position should survive app restart.
- Hide should not terminate the backend.
- Global hotkey should restore the bar and move focus to the bar window.

Suggested first hotkey:

```text
Ctrl + Shift + Space
```

The hotkey should become configurable once a settings surface exists.

### Dependencies

- Existing authenticated session data from `agent-notify-tray`.
- A stable way to mark notifications as clicked or opened.
- A future settings surface for hotkey and hide/show preferences.

### Expected change locations later

- Future Tauri UI window module.
- Backend session response, if unread/click tracking needs to be persisted.
- Notification activation state storage.
- Settings/config storage for hotkey and bar position.

Expected backend changes later:

- Add notification click/open state instead of inferring from Toast activation only.
- Add a UI summary endpoint or extend `/sessions` with counts and unopened notification records.
- Add focus result details so the UI can tell success from best-effort failure.
- Add stale window pruning so `terminalCount` means currently open terminal count.

### Risks

- Windows Terminal multi-tab limits may prevent exact tab selection.
- If click tracking is stored only in memory, unread state will reset after restart.
- If the bar is too tall or too wide, it will stop feeling like a status bar.

### Verification plan

- Confirm the compact view stays small at common desktop scaling values.
- Confirm hide/show does not terminate the backend.
- Confirm the chooser only exposes unclicked notifications.
- Confirm selecting a notification returns to the intended session.
- Confirm counts update when sessions start, complete, and close.
- Confirm clicked notifications disappear from the chooser after successful focus.
- Confirm stale closed terminal windows do not inflate `terminalCount`.
- Confirm the hotkey restores the bar after it is hidden.

### Sync log

- 2026-05-11: Added first-pass design for a persistent floating desktop status bar and notification chooser flow.
- 2026-05-11: Added count definitions, data contract, state machine, click flow, and first backend integration recommendation.

## Update 2026-05-11 - Bar-anchored notification display

### Decision

Notifications should be displayed from the current floating status bar, not from the screen corner.

### Behavior

- If the compact or expanded bar is visible, the notification panel is anchored to the bar.
- If the bar is hidden as a narrow strip, the notification panel is anchored to the hidden strip.
- The notification panel should avoid leaving the current monitor work area. Prefer below the bar; if there is not enough room, place it above the bar.
- The notification panel should use the same visual system as the floating bar: same dark panel, border, radius, and shadow.
- The notification panel should not obscure the counter chips when there is enough surrounding space.

### Interaction

- Clicking a bar-anchored notification should focus the corresponding terminal through the same authenticated focus flow used by the completed-session chooser.
- After successful focus, mark the notification as clicked/opened so it leaves the completed notification chooser.
- If focus fails, keep the notification available and show it as retryable.

### Fallback

Windows Toast can remain a temporary fallback while the floating UI process is not running, but it should no longer be treated as the primary target experience once the floating status bar exists.

### Implementation implication

The future UI needs an internal notification presenter owned by the floating window. Backend notification events should update UI state and request a bar-anchored panel rather than always delegating visual display to Windows Toast.

### Sync log

- 2026-05-11: User requested notifications to pop from the current floating box instead of the old bottom-right/corner notification behavior.

## Update 2026-05-11 - Execution Sync

### Implemented

- Added an authenticated `GET /floating-status` backend snapshot for floating-bar summary counts, countable sessions, unopened completed notifications, persisted UI state, and generation time.
- Added an authenticated `PUT /floating-status/state` backend path that persists hidden/expanded/position/hotkey state under the AgentNotify runtime home as `floating-status-bar.json`.
- Added in-memory notification click/open tracking keyed by event id, recorded only for dedupe-accepted notifiable events.
- Extended `POST /focus/{sessionId}` with an optional JSON `notificationId`; when focus succeeds, the matching notification is marked clicked.
- Preserved `/activate/{activationId}` for Toast fallback and marks the related notification opened when an activation is accepted.
- Added best-effort terminal liveness counting: valid HWNDs are checked exactly; sessions without a usable HWND count only while they have terminal/process hints and are fresh.

### Changed Files

- `src/agent-notify-tray/src/floating_status.rs`
- `src/agent-notify-tray/src/main.rs`
- `src/agent-notify-tray/Cargo.toml`

### Verification

- `cargo fmt --check`
- `cargo test --workspace --ignore-rust-version`
- `cargo clippy --workspace --ignore-rust-version -- -D warnings`
- `cargo build --workspace --ignore-rust-version`
- `git diff --check`

The local default Rust toolchain is rustc 1.94.0 while the workspace declares `rust-version = "1.95"`, so standard cargo commands without `--ignore-rust-version` stop at the version gate on this machine.

### Review

- `/review` was not available through this execution surface.
- Performed a local final review in the required order: special cases, complexity, data structure, and breakage risk.
- Fixed review findings for partial state-file defaults and `hwnd: 0` fallback handling.
- Fixed clippy findings for needless borrow and needless return.

### Drift From Original Design

- The full frameless always-on-top Tauri window, drag behavior, global hotkey registration, and bar-anchored visual notification presenter are not implemented because the repository still has no Tauri UI shell.
- The backend contract needed by that future UI is implemented without replacing the existing Toast fallback.

### Follow-up

- Add the Tauri floating window that consumes `/floating-status`, saves `/floating-status/state`, registers `Ctrl + Shift + Space`, and renders bar-anchored notifications.
- Persist notification click/open state beyond backend process lifetime if completed-session history must survive restart.

## Update 2026-05-11 - Desktop UI Execution Sync

### Implemented

- Added a native Windows floating status window inside `agent-notify-tray` as the first desktop implementation because this repository still has no Tauri frontend package, `src-tauri`, or frontend entrypoint.
- Implemented a frameless always-on-top tool window with compact, expanded, and hidden-strip states.
- Implemented drag-to-position with work-area clamping and persisted position/state reuse through the existing floating status state file.
- Registered `Ctrl + Shift + Space` as the first global restore hotkey.
- Rendered compact counters for total, running, completed, and unopened completed notifications.
- Rendered expanded running/completed session lists and a completed-notification chooser.
- Rendered bar-anchored notification panels for new notifiable events while keeping Windows Toast as fallback when no floating UI is available.
- Wired notification and chooser clicks to the authenticated focus flow with `notificationId`, so successful focus marks the notification clicked and removes it from the unopened chooser.

### Changed Files

- `src/agent-notify-tray/src/floating_window.rs`
- `src/agent-notify-tray/src/main.rs`

### Runtime Verification

- Started `D:\own\notify-target-ui\debug\agent-notify-tray.exe serve`.
- Confirmed `GET /floating-status` returns `200 OK`.
- Confirmed the desktop window class `AgentNotifyFloatingStatusBar` exists and is visible.
- Confirmed compact state is `360x54`, expanded state is `360x318`, and hidden strip is `360x18`.
- Confirmed a new completed event expands the bar-anchored notification surface to `360x144`.
- Confirmed `POST /focus/{sessionId}` with `notificationId` focuses the terminal and removes the notification from the unopened list.
- Confirmed `Ctrl + Shift + Space` restores the hidden strip back to compact state.

### Review Notes

- Fixed a runtime deadlock in geometry calculation caused by re-locking the floating model while computing the default position.
- Fixed HTTP hangs by keeping Win32 geometry changes on the window thread and making backend-to-UI model updates non-blocking.
- Reduced paint-time lock scope by cloning a render snapshot before drawing with GDI.

### Drift From Original Design

- This is not a Tauri UI. It is a native Win32 desktop surface hosted in the existing Rust tray/backend process. That is a deliberate scope reduction to deliver the requested visible desktop behavior without introducing a new frontend stack that does not exist in this repository.

### Remaining Risk

- Notification click/open records are still in memory and reset when the backend process restarts.
- The UI is intentionally minimal GDI drawing; a future Tauri shell can replace it if the project later adds a real frontend workspace.

## Update 2026-05-11 - Tauri UI Execution Sync

### Implemented

- Replaced the rejected native Win32/GDI floating-bar path with a real Tauri v2 desktop shell and Vite/TypeScript frontend.
- Added `src-tauri` and `src-ui` as the production desktop UI surface for the floating status bar.
- Configured the Tauri window as frameless, always-on-top, transparent, non-resizable, and skipped from the taskbar.
- Implemented compact, expanded, and hidden-strip UI states in the Tauri frontend.
- Implemented drag-to-position using Tauri window APIs, with persisted position reuse through `PUT /floating-status/state`.
- Implemented `Ctrl + Shift + Space` as the global restore hotkey in the Tauri shell.
- Implemented count chips for total terminals, running sessions, completed sessions, and unopened completed notifications.
- Implemented the completed notification chooser and bar-anchored in-app notification panel.
- Routed frontend backend access through Tauri commands so the Bearer token stays in Rust, not visible frontend state.
- Added a Tauri heartbeat endpoint so the backend can suppress Windows Toast only while the Tauri UI is active.
- Changed Tauri backend startup to `agent-notify-tray serve --no-native-tray`, so the visible UI path is Tauri-owned.
- Changed `/activate/{activationId}` to use the same focus-first click marking rule: notification clicked state is recorded only after focus succeeds.
- Granted foreground activation from the Tauri process before calling `/focus/{sessionId}`, improving direct return to the target terminal after a notification click.
- Changed the frontend focus flow to close chooser/toast only when the backend returns `focused: true`; failed focus keeps the notification available.
- Added monitor work-area clamping and above/below notification placement so the panel stays anchored to the floating bar without running off-screen.
- Added backend-start throttling in the Tauri shell to avoid duplicate backend processes while the backend is still initializing.

### Changed Files

- `Cargo.toml`
- `Cargo.lock`
- `package.json`
- `package-lock.json`
- `tsconfig.json`
- `vite.config.ts`
- `src-tauri/Cargo.toml`
- `src-tauri/build.rs`
- `src-tauri/tauri.conf.json`
- `src-tauri/capabilities/default.json`
- `src-tauri/src/main.rs`
- `src-ui/index.html`
- `src-ui/src/main.ts`
- `src-ui/src/styles.css`
- `src/agent-notify-tray/Cargo.toml`
- `src/agent-notify-tray/src/main.rs`
- `src/agent-notify-tray/src/bin/agent-notify-activate.rs`

### Verification

- `npm install`: passed.
- `npm run typecheck`: passed.
- `npm run build:ui`: passed.
- `cargo fmt --check`: passed.
- `cargo test --workspace --ignore-rust-version`: passed.
- `cargo clippy --workspace --ignore-rust-version -- -D warnings`: passed.
- `cargo build --workspace --ignore-rust-version`: passed.
- `npm run build`: passed and produced `target/release/agent-notify-desktop.exe`, MSI, and NSIS bundles.
- `git diff --check`: passed with only Git CRLF normalization warnings.
- Runtime: started `target/release/agent-notify-desktop.exe`; confirmed one visible `Tauri Window` and backend command line `agent-notify-tray.exe serve --no-native-tray`.
- Runtime: confirmed `GET /floating-status` returns `200 OK`.
- Runtime: confirmed no `AgentNotifyFloatingStatusBar` or old native floating UI window exists.
- Runtime: confirmed hidden state restores via `Ctrl + Shift + Space`.
- Runtime: confirmed a new completed event expands the Tauri window to show the bar-anchored notification panel.
- Runtime: confirmed `POST /focus/{sessionId}` with `notificationId` returns `focused: true` and removes that notification from unopened state.
- Runtime: killed the backend while Tauri remained open and confirmed the Tauri shell restarted exactly one `serve --no-native-tray` backend process.

### Review

- `/review` was unavailable in this execution surface.
- Ran two parallel read-only explorer reviews.
- Fixed the reported focus-result bug where frontend HTTP success was incorrectly treated as focus success.
- Fixed the reported Win32 tray contamination for the Tauri startup path by adding `--no-native-tray`.
- Fixed the reported high-DPI/window geometry risk by switching Tauri sizing to logical units and clamping persisted physical positions to the monitor work area.
- Fixed the reported notification anchoring risk by adding above/below placement.
- Fixed the reported duplicate backend spawn risk with startup throttling.
- Fixed the reported Toast activation click-state drift by moving click marking behind successful backend focus.

### Drift From Original Design

- The Tauri shell starts the existing Axum backend as a separate headless process instead of embedding the backend in the Tauri process. This keeps the existing hook-facing HTTP contract stable while making Tauri the visible desktop UI owner.
- Windows Toast remains as a fallback when the Tauri UI heartbeat is absent.

### Follow-up

- Notification click/open records are still in memory and reset when the backend process restarts.
- Hook-triggered events still require the backend/Tauri app to be running; login auto-start for the Tauri app is not implemented in this module.

## Update 2026-05-11 - Focus Target Accuracy Sync

### Implemented

- Fixed notification activation to keep a process/window snapshot from the original event, so older notifications are not redirected by later updates to the same `sessionId`.
- Added `window.pid` to the event/session model and hook output so HWND targets can be checked against the owning terminal process.
- Changed backend and Toast activation focus logic to validate stored HWNDs against `window.pid`, `process.pid`, `parentPid`, or exact title before focusing; mismatched HWNDs are skipped and PID/title fallbacks are attempted.
- Improved hook fallback session identity to include process start time, transcript path, HWND, and terminal window PID instead of collapsing missing-session events to only `tool|cwd`.
- Improved hook HWND parsing to accept pointer-sized decimal and `0x...` values.

### Verification

- `cargo fmt --check`
- `cargo test -p agent-notify-core --ignore-rust-version`
- `cargo test -p agent-notify-tray --ignore-rust-version`
- `cargo test --workspace --ignore-rust-version`
- `cargo clippy -p agent-notify-core --ignore-rust-version -- -D warnings`
- `cargo clippy -p agent-notify-tray --ignore-rust-version -- -D warnings`
- `cargo clippy --workspace --ignore-rust-version -- -D warnings`
- `cargo build --workspace --ignore-rust-version`
- `cargo build -p agent-notify-tray -p agent-notify --release --ignore-rust-version`
- `npm run typecheck`
- `npm run build`
- `git diff --check`
- PowerShell parser check for `scripts/hooks/agent-notify-hook.ps1`
- Local hook event construction smoke check confirmed distinct fallback `sessionId` and emitted `window.pid`.
- Runtime hook repair copied the updated hook and emitter to `%LOCALAPPDATA%\AgentNotify`; the release Tauri app is running with `agent-notify-tray.exe serve --no-native-tray`.
