# Floating Status Bar Requirement

## Update 2026-05-11 - Desktop Floating Status Bar

### Confirmed requirement

Create a small desktop floating status bar that stays visible on Windows, can be dragged anywhere, can be hidden, and can be summoned with a hotkey. It must show:

1. Current total number of open terminals/sessions.
2. Number of running sessions.
3. Number of completed sessions.
4. A completed-session action that opens a chooser of notifications that have not yet been clicked, then uses the selected notification to jump back into that terminal.

### User experience goals

- The bar should feel lightweight and always-available, not like a full window.
- It should support both compact and expanded states.
- It should let the user hide it without killing the backend.
- It should support a global hotkey to bring it back.
- It should make completed sessions actionable instead of passive.

### Non-goals

- No full reimplementation of the backend notification system.
- No redesign of hook capture, activation nonce, or toast delivery in this planning pass.
- No cross-platform design.
- No assumption that Windows Terminal can always jump to an exact tab; the design only needs to define the selection and activation flow.

### Constraints

- Must work alongside the current `agent-notify-tray` backend.
- Must not break the current toast click activation path.
- Must preserve the current security boundary: the notification UI should not expose tokens or raw command output.
- Must keep the bar small enough to sit on the desktop without dominating the screen.

### Unresolved design questions

- Whether the bar is implemented inside the future Tauri shell or as a separate window in the same app.
- Whether the completed-session chooser shows one row per session only, or also shows per-notification history.
- Whether the hotkey should be globally configurable in settings or fixed in the first version.

## Update 2026-05-11 - Notifications Anchor To Floating Bar

### Confirmed requirement change

System notifications should appear from the current floating status bar position, not from the old screen-corner notification area.

### User experience rule

- When the floating bar is visible, new task notifications pop out adjacent to the bar.
- When the floating bar is hidden as the narrow strip, notifications pop out adjacent to that narrow strip.
- The notification should visually belong to the bar, so the user can connect counts, unread state, and click-to-return behavior in one place.
- Screen-corner Toast-style notifications are no longer the target UI for the floating status bar design.

### Compatibility note

The existing Windows Toast path may remain as an implementation fallback while the floating UI is not yet available, but the product design target is bar-anchored notification display.
