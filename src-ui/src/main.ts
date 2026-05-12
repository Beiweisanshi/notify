import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { LogicalSize, PhysicalPosition } from "@tauri-apps/api/dpi";
import {
  currentMonitor,
  getCurrentWindow,
  type Monitor,
  monitorFromPoint,
  primaryMonitor
} from "@tauri-apps/api/window";
import "./styles.css";

const WINDOW_WIDTH = 360;
const HEIGHT_HIDDEN = 18;
const HEIGHT_COMPACT = 54;
const HEIGHT_EXPANDED = 318;
const HEIGHT_TOAST = 82;
const GAP = 8;
const POLL_MS = 1000;
const HEARTBEAT_MS = 2000;
const TOAST_MS = 8000;

type SessionStatus = "running" | "waiting_user" | "completed" | "failed" | "unknown";

interface FloatingBarPosition {
  x: number;
  y: number;
}

interface FloatingBarState {
  hidden: boolean;
  expanded: boolean;
  position?: FloatingBarPosition | null;
  hotkey: string;
}

interface FloatingStatusSummary {
  terminalCount: number;
  runningCount: number;
  completedCount: number;
  unopenedCompletedNotifications: number;
  countsBestEffort: boolean;
}

interface FloatingStatusSession {
  sessionId: string;
  relatedSessionIds?: string[];
  tool: string;
  projectName: string;
  status: SessionStatus;
  lastMessage: {
    title: string;
    body: string;
    detail?: string | null;
  };
  unopenedNotificationCount: number;
}

interface FloatingStatusNotification {
  notificationId: string;
  sessionId: string;
  eventType: string;
  title: string;
  body: string;
  detail?: string | null;
  createdAt: string;
  clickedAt?: string | null;
}

interface FloatingStatusSnapshot {
  summary: FloatingStatusSummary;
  state: FloatingBarState;
  sessions: FloatingStatusSession[];
  notifications: FloatingStatusNotification[];
  generatedAt: string;
}

interface FocusResponse {
  focused: boolean;
  openedSessionDetail: boolean;
  sessionFound: boolean;
  focusPrecision?: "exact" | "shared_window" | null;
  clickedNotificationId?: string | null;
  error?: "session_not_found" | "focus_failed" | null;
}

interface DismissResponse {
  dismissedCount: number;
}

type ChooserScope =
  | { kind: "closed" }
  | { kind: "all" }
  | { kind: "session"; sessionId: string; sessionIds: string[] };
type ToastPlacement = "below" | "above";

const appWindow = getCurrentWindow();
const rootElement = document.querySelector<HTMLDivElement>("#app");

if (!rootElement) {
  throw new Error("missing app root");
}

const root: HTMLDivElement = rootElement;

let snapshot: FloatingStatusSnapshot | null = null;
let barState: FloatingBarState = {
  hidden: false,
  expanded: false,
  hotkey: "Ctrl+Shift+Space"
};
let chooser: ChooserScope = { kind: "closed" };
let activeToast: FloatingStatusNotification | null = null;
let toastPlacement: ToastPlacement = "below";
let toastTimer: number | undefined;
let firstSnapshot = true;
let focusError: string | null = null;
const seenNotifications = new Set<string>();

void boot();

async function boot(): Promise<void> {
  await listen("floating-hotkey", () => {
    void setBarState({ hidden: false, expanded: false });
  });
  await refreshSnapshot();
  await heartbeat();
  window.setInterval(() => void refreshSnapshot(), POLL_MS);
  window.setInterval(() => void heartbeat(), HEARTBEAT_MS);
}

async function heartbeat(): Promise<void> {
  try {
    await invoke("floating_ui_heartbeat");
  } catch {
    // The next data refresh will surface backend state.
  }
}

async function refreshSnapshot(): Promise<void> {
  try {
    const next = await invoke<FloatingStatusSnapshot>("get_floating_status");
    snapshot = next;
    barState = normalizeState(next.state);
    syncNotifications(next.notifications);
    pruneChooser();
    render();
    await applyWindowGeometry();
  } catch (error) {
    renderError(error);
  }
}

function syncNotifications(notifications: FloatingStatusNotification[]): void {
  for (const notification of notifications) {
    if (seenNotifications.has(notification.notificationId)) {
      continue;
    }
    seenNotifications.add(notification.notificationId);
    if (!firstSnapshot) {
      showToast(notification);
    }
  }
  firstSnapshot = false;
  if (
    activeToast &&
    !notifications.some((item) => item.notificationId === activeToast?.notificationId)
  ) {
    clearToast();
  }
}

function showToast(notification: FloatingStatusNotification): void {
  activeToast = notification;
  if (toastTimer !== undefined) {
    window.clearTimeout(toastTimer);
  }
  toastTimer = window.setTimeout(() => {
    clearToast();
    void applyWindowGeometry();
    render();
  }, TOAST_MS);
}

function clearToast(): void {
  activeToast = null;
  if (toastTimer !== undefined) {
    window.clearTimeout(toastTimer);
    toastTimer = undefined;
  }
}

function normalizeState(state: FloatingBarState): FloatingBarState {
  return {
    hidden: Boolean(state.hidden),
    expanded: Boolean(state.expanded),
    position: state.position ?? null,
    hotkey: state.hotkey?.trim() || "Ctrl+Shift+Space"
  };
}

async function setBarState(patch: Partial<FloatingBarState>): Promise<void> {
  const next = normalizeState({
    ...barState,
    ...patch
  });
  if (next.hidden) {
    next.expanded = false;
    chooser = { kind: "closed" };
  }
  barState = await persistBarState(next);
  render();
  await applyWindowGeometry();
}

async function persistBarState(next: FloatingBarState): Promise<FloatingBarState> {
  const saved = await invoke<FloatingBarState>("put_floating_status_state", { state: next });
  return normalizeState(saved);
}

async function saveCurrentPosition(): Promise<void> {
  try {
    const position = await appWindow.outerPosition();
    const barPosition = await barPositionFromWindowPosition({ x: position.x, y: position.y });
    const clamped = await clampBarPosition(barPosition, baseHeight());
    await setBarState({
      position: clamped
    });
  } catch {
    // Position persistence is best-effort; dragging itself still works.
  }
}

async function applyWindowGeometry(): Promise<void> {
  const height = windowHeight();
  await appWindow.setSize(new LogicalSize(WINDOW_WIDTH, height));
  if (barState.position) {
    const barPosition = await clampBarPosition(barState.position, baseHeight());
    if (barPosition.x !== barState.position.x || barPosition.y !== barState.position.y) {
      barState = await persistBarState({ ...barState, position: barPosition });
    }
    const nextPlacement = await chooseToastPlacement(barPosition, baseHeight());
    const placementChanged = nextPlacement !== toastPlacement;
    toastPlacement = nextPlacement;
    const windowPosition = await windowPositionFromBarPosition(barPosition, height);
    await appWindow.setPosition(new PhysicalPosition(windowPosition.x, windowPosition.y));
    if (placementChanged) {
      render();
    }
  }
  await appWindow.show();
}

async function monitorForPosition(position: FloatingBarPosition): Promise<Monitor | null> {
  return (
    (await monitorFromPoint(position.x, position.y)) ??
    (await currentMonitor()) ??
    (await primaryMonitor())
  );
}

async function clampBarPosition(
  position: FloatingBarPosition,
  height: number
): Promise<FloatingBarPosition> {
  const monitor = await monitorForPosition(position);
  if (!monitor) {
    return position;
  }
  const area = monitor.workArea;
  const widthPx = toPhysical(WINDOW_WIDTH, monitor.scaleFactor);
  const heightPx = toPhysical(height, monitor.scaleFactor);
  const minX = area.position.x;
  const minY = area.position.y;
  const maxX = minX + Math.max(0, area.size.width - widthPx);
  const maxY = minY + Math.max(0, area.size.height - heightPx);
  return {
    x: clamp(position.x, minX, maxX),
    y: clamp(position.y, minY, maxY)
  };
}

async function chooseToastPlacement(
  barPosition: FloatingBarPosition,
  height: number
): Promise<ToastPlacement> {
  if (!activeToast) {
    return "below";
  }
  const monitor = await monitorForPosition(barPosition);
  if (!monitor) {
    return "below";
  }
  const area = monitor.workArea;
  const basePx = toPhysical(height, monitor.scaleFactor);
  const toastPx = toPhysical(HEIGHT_TOAST, monitor.scaleFactor);
  const gapPx = toPhysical(GAP, monitor.scaleFactor);
  const belowBottom = barPosition.y + basePx + gapPx + toastPx;
  if (belowBottom <= area.position.y + area.size.height) {
    return "below";
  }
  const aboveTop = barPosition.y - gapPx - toastPx;
  return aboveTop >= area.position.y ? "above" : "below";
}

async function windowPositionFromBarPosition(
  barPosition: FloatingBarPosition,
  height: number
): Promise<FloatingBarPosition> {
  const monitor = await monitorForPosition(barPosition);
  if (!monitor) {
    return barPosition;
  }
  const area = monitor.workArea;
  const widthPx = toPhysical(WINDOW_WIDTH, monitor.scaleFactor);
  const heightPx = toPhysical(height, monitor.scaleFactor);
  const offsetY =
    activeToast && toastPlacement === "above"
      ? toPhysical(HEIGHT_TOAST + GAP, monitor.scaleFactor)
      : 0;
  return {
    x: clamp(barPosition.x, area.position.x, area.position.x + Math.max(0, area.size.width - widthPx)),
    y: clamp(
      barPosition.y - offsetY,
      area.position.y,
      area.position.y + Math.max(0, area.size.height - heightPx)
    )
  };
}

async function barPositionFromWindowPosition(
  windowPosition: FloatingBarPosition
): Promise<FloatingBarPosition> {
  const scaleFactor = await appWindow.scaleFactor();
  const offsetY =
    activeToast && toastPlacement === "above" ? toPhysical(HEIGHT_TOAST + GAP, scaleFactor) : 0;
  return {
    x: windowPosition.x,
    y: windowPosition.y + offsetY
  };
}

function toPhysical(value: number, scaleFactor: number): number {
  return Math.round(value * scaleFactor);
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}

function baseHeight(): number {
  return barState.hidden
    ? HEIGHT_HIDDEN
    : barState.expanded
      ? HEIGHT_EXPANDED
      : HEIGHT_COMPACT;
}

function windowHeight(): number {
  return activeToast ? baseHeight() + GAP + HEIGHT_TOAST : baseHeight();
}

function render(): void {
  if (barState.hidden) {
    root.innerHTML = hiddenTemplate();
    bindHiddenEvents();
    return;
  }

  root.innerHTML = `
    ${activeToast && toastPlacement === "above" ? toastTemplate(activeToast) : ""}
    <section class="surface ${barState.expanded ? "expanded" : "compact"}">
      ${headerTemplate()}
      ${barState.expanded ? bodyTemplate() : ""}
    </section>
    ${activeToast && toastPlacement === "below" ? toastTemplate(activeToast) : ""}
    ${focusErrorTemplate()}
  `;
  bindEvents();
}

function renderError(error: unknown): void {
  root.innerHTML = `
    <section class="surface compact">
      <div class="bar-header" data-drag>
        <div class="brand">AN</div>
        <div class="error-text">${escapeHtml(String(error))}</div>
      </div>
    </section>
  `;
  bindEvents();
}

function hiddenTemplate(): string {
  return `
    ${activeToast && toastPlacement === "above" ? toastTemplate(activeToast) : ""}
    <button class="hidden-tab" type="button" data-drag data-restore title="Show">
      Agent Notify
    </button>
    ${activeToast && toastPlacement === "below" ? toastTemplate(activeToast) : ""}
    ${focusErrorTemplate()}
  `;
}

function headerTemplate(): string {
  const summary = snapshot?.summary ?? {
    terminalCount: 0,
    runningCount: 0,
    completedCount: 0,
    unopenedCompletedNotifications: 0,
    countsBestEffort: false
  };
  return `
    <div class="bar-header" data-drag>
      <div class="brand">AN</div>
      ${chipTemplate("All", summary.terminalCount, "total")}
      ${chipTemplate("Run", summary.runningCount, "running")}
      <button class="chip completed" type="button" data-open-all title="Completed notifications">
        <span>Done</span>
        <strong>${summary.completedCount}</strong>
        ${
          summary.unopenedCompletedNotifications > 0
            ? `<em>${summary.unopenedCompletedNotifications}</em>`
            : ""
        }
      </button>
      <button class="icon-button" type="button" data-toggle title="Toggle">${barState.expanded ? "^" : "v"}</button>
      <button class="icon-button" type="button" data-hide title="Hide">x</button>
    </div>
  `;
}

function chipTemplate(label: string, value: number, tone: string): string {
  return `
    <div class="chip ${tone}">
      <span>${label}</span>
      <strong>${value}</strong>
    </div>
  `;
}

function bodyTemplate(): string {
  if (!snapshot) {
    return `<div class="empty">Waiting for events</div>`;
  }
  return chooser.kind === "closed" ? sessionListsTemplate(snapshot) : chooserTemplate(snapshot);
}

function sessionListsTemplate(data: FloatingStatusSnapshot): string {
  const running = data.sessions.filter((session) => session.status === "running");
  const completed = data.sessions.filter((session) => session.status === "completed");
  return `
    <div class="body">
      ${sectionTemplate("Running", running, false)}
      ${sectionTemplate("Completed", completed, true)}
    </div>
  `;
}

function sectionTemplate(
  title: string,
  sessions: FloatingStatusSession[],
  completed: boolean
): string {
  return `
    <div class="section-title">
      <span>${title}</span>
      <span>${sessions.length}</span>
    </div>
    <div class="rows">
      ${
        sessions.length === 0
          ? `<div class="muted-row">None</div>`
          : sessions.map((session) => sessionRowTemplate(session, completed)).join("")
      }
    </div>
  `;
}

function sessionRowTemplate(session: FloatingStatusSession, completed: boolean): string {
  const badge = completed
    ? `${session.unopenedNotificationCount} new`
    : session.status === "running"
      ? "live"
      : session.status;
  return `
    <button class="row" type="button" data-session="${escapeAttr(session.sessionId)}">
      <span class="dot ${completed ? "done" : "live"}"></span>
      <span class="row-main">
        <strong>${escapeHtml(session.tool)} / ${escapeHtml(session.projectName)}</strong>
        <small>${escapeHtml(session.lastMessage.title)}</small>
      </span>
      <span class="row-badge">${escapeHtml(badge)}</span>
    </button>
  `;
}

function chooserTemplate(data: FloatingStatusSnapshot): string {
  const rows = chooserNotifications(data);
  let title = "Unopened notifications";
  if (chooser.kind === "session") {
    const sessionId = chooser.sessionId;
    title =
      data.sessions.find((session) => session.sessionId === sessionId)?.projectName ??
      "Notifications";
  }
  return `
    <div class="body chooser">
      <div class="chooser-title">
        <span>${escapeHtml(title)}</span>
        <span class="chooser-actions">
          <button class="clear-button" type="button" data-dismiss-all title="Clear all" ${rows.length === 0 ? "disabled" : ""}>Clear</button>
          <button class="icon-button" type="button" data-close-chooser title="Close">x</button>
        </span>
      </div>
      ${
        rows.length === 0
          ? `<div class="empty">No unopened notifications</div>`
          : `<div class="notification-list">${rows.map(notificationRowTemplate).join("")}</div>`
      }
    </div>
  `;
}

function notificationRowTemplate(notification: FloatingStatusNotification): string {
  return `
    <div class="row notification-row">
      <button class="notification-open" type="button"
        data-focus-session="${escapeAttr(notification.sessionId)}"
        data-focus-notification="${escapeAttr(notification.notificationId)}">
        <span class="row-main">
          <strong>${escapeHtml(notification.title)}</strong>
          <small>${escapeHtml(notification.body)}</small>
        </span>
        <span class="row-badge action">open</span>
      </button>
      <button class="icon-button row-close" type="button"
        data-dismiss-notification="${escapeAttr(notification.notificationId)}"
        data-dismiss-session="${escapeAttr(notification.sessionId)}"
        title="Dismiss">x</button>
    </div>
  `;
}

function toastTemplate(notification: FloatingStatusNotification): string {
  return `
    <button class="toast ${toastPlacement}" type="button"
      data-focus-session="${escapeAttr(notification.sessionId)}"
      data-focus-notification="${escapeAttr(notification.notificationId)}">
      <strong>${escapeHtml(notification.title)}</strong>
      <span>${escapeHtml(notification.body)}</span>
      <small>click to focus terminal</small>
    </button>
  `;
}

function focusErrorTemplate(): string {
  return focusError ? `<div class="focus-error">${escapeHtml(focusError)}</div>` : "";
}

function chooserNotifications(data: FloatingStatusSnapshot): FloatingStatusNotification[] {
  const sessionIds =
    chooser.kind === "session"
      ? new Set(chooser.sessionIds.length > 0 ? chooser.sessionIds : [chooser.sessionId])
      : null;
  return data.notifications.filter((notification) => {
    if (sessionIds) {
      return sessionIds.has(notification.sessionId);
    }
    return true;
  });
}

function chooserSessionIds(session: FloatingStatusSession): string[] {
  return session.relatedSessionIds && session.relatedSessionIds.length > 0
    ? session.relatedSessionIds
    : [session.sessionId];
}

function pruneChooser(): void {
  if (!snapshot || chooser.kind === "closed") {
    return;
  }
  if (chooserNotifications(snapshot).length === 0) {
    chooser = { kind: "closed" };
  }
}

function bindHiddenEvents(): void {
  const tab = root.querySelector<HTMLElement>("[data-restore]");
  if (tab) {
    bindSurfaceGestures(tab, {
      onClick: () => {
        void setBarState({ hidden: false });
      }
    });
  }
  bindFocusEvents();
}

function bindEvents(): void {
  const header = root.querySelector<HTMLElement>("[data-drag]");
  if (header) {
    bindSurfaceGestures(header, {
      onDoubleClick: () => {
        void setBarState({ expanded: !barState.expanded });
      }
    });
  }
  root.querySelector("[data-toggle]")?.addEventListener("click", () => {
    void setBarState({ expanded: !barState.expanded });
  });
  root.querySelector("[data-hide]")?.addEventListener("click", () => {
    void setBarState({ hidden: true, expanded: false });
  });
  root.querySelector("[data-open-all]")?.addEventListener("click", () => {
    chooser = { kind: "all" };
    void setBarState({ expanded: true });
  });
  root.querySelector("[data-close-chooser]")?.addEventListener("click", () => {
    chooser = { kind: "closed" };
    render();
  });
  root.querySelector("[data-dismiss-all]")?.addEventListener("click", () => {
    void dismissChooserNotifications();
  });
  root.querySelectorAll<HTMLElement>("[data-dismiss-notification]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      const notificationId = button.dataset.dismissNotification;
      const sessionId = button.dataset.dismissSession;
      if (!notificationId || !sessionId) {
        return;
      }
      void dismissNotification(notificationId, sessionId);
    });
  });
  root.querySelectorAll<HTMLElement>("[data-session]").forEach((row) => {
    row.addEventListener("click", () => {
      const sessionId = row.dataset.session;
      if (!sessionId || !snapshot) {
        return;
      }
      const session = snapshot.sessions.find((item) => item.sessionId === sessionId);
      if (session && session.unopenedNotificationCount > 0) {
        chooser = {
          kind: "session",
          sessionId,
          sessionIds: chooserSessionIds(session)
        };
        void setBarState({ expanded: true });
      } else {
        void focusSession(sessionId);
      }
    });
  });
  bindFocusEvents();
}

function bindFocusEvents(): void {
  root.querySelectorAll<HTMLElement>("[data-focus-session]").forEach((row) => {
    row.addEventListener("click", () => {
      const sessionId = row.dataset.focusSession;
      if (!sessionId) {
        return;
      }
      void focusSession(sessionId, row.dataset.focusNotification);
    });
  });
}

interface SurfaceGestureOptions {
  onClick?: () => void;
  onDoubleClick?: () => void;
}

const CLICK_SLOP_PX = 4;
const DOUBLE_CLICK_MS = 350;
const GESTURE_IGNORE_SELECTOR = "button:not([data-drag]), .row, .chip";

type GestureState = "idle" | "pressed" | "dragging";

function bindSurfaceGestures(el: HTMLElement, options: SurfaceGestureOptions): void {
  let startX = 0;
  let startY = 0;
  let lastClickAt = 0;
  let state: GestureState = "idle";

  el.addEventListener("pointerdown", (event) => {
    const target = event.target as HTMLElement;
    if (target.closest(GESTURE_IGNORE_SELECTOR)) {
      return;
    }
    startX = event.screenX;
    startY = event.screenY;
    state = "pressed";
  });

  el.addEventListener("pointermove", (event) => {
    if (state !== "pressed") {
      return;
    }
    if (Math.abs(event.screenX - startX) > CLICK_SLOP_PX ||
        Math.abs(event.screenY - startY) > CLICK_SLOP_PX) {
      state = "dragging";
      void appWindow.startDragging();
      // startDragging() doesn't resolve when drag ends, so we poll twice to
      // capture short drags (300ms) and longer ones (900ms). Imperfect, but
      // there's no "drag ended" signal from Tauri on Windows.
      window.setTimeout(() => void saveCurrentPosition(), 300);
      window.setTimeout(() => void saveCurrentPosition(), 900);
    }
  });

  el.addEventListener("pointerup", () => {
    const prev = state;
    state = "idle";
    if (prev !== "pressed") {
      lastClickAt = 0;
      return;
    }
    const now = performance.now();
    if (options.onDoubleClick && now - lastClickAt < DOUBLE_CLICK_MS) {
      lastClickAt = 0;
      options.onDoubleClick();
      return;
    }
    lastClickAt = options.onDoubleClick ? now : 0;
    options.onClick?.();
  });

  el.addEventListener("pointercancel", () => {
    state = "idle";
  });
}

async function focusSession(sessionId: string, notificationId?: string): Promise<void> {
  try {
    focusError = null;
    const response = await invoke<FocusResponse>("focus_session", {
      sessionId,
      notificationId: notificationId || null
    });
    if (notificationId && response.clickedNotificationId === notificationId) {
      seenNotifications.delete(notificationId);
      focusError = null;
      clearToast();
      chooser = { kind: "closed" };
      await refreshSnapshot();
      return;
    }
    if (!response.focused) {
      focusError = focusErrorMessage(response);
      render();
      await applyWindowGeometry();
      return;
    }
    if (notificationId) {
      seenNotifications.delete(notificationId);
    }
    clearToast();
    chooser = { kind: "closed" };
    await refreshSnapshot();
  } catch (error) {
    focusError = String(error);
    render();
    await applyWindowGeometry();
  }
}

async function dismissNotification(notificationId: string, sessionId: string): Promise<void> {
  try {
    focusError = null;
    await invoke<DismissResponse>("dismiss_notification", {
      notificationId,
      sessionId
    });
    if (activeToast?.notificationId === notificationId) {
      clearToast();
    }
    await refreshSnapshot();
  } catch (error) {
    focusError = String(error);
    render();
    await applyWindowGeometry();
  }
}

async function dismissChooserNotifications(): Promise<void> {
  try {
    focusError = null;
    const rows = snapshot ? chooserNotifications(snapshot) : [];
    if (chooser.kind === "session") {
      await Promise.all(
        rows.map((notification) =>
          invoke<DismissResponse>("dismiss_notification", {
            notificationId: notification.notificationId,
            sessionId: notification.sessionId
          })
        )
      );
      if (
        activeToast &&
        rows.some((notification) => notification.notificationId === activeToast?.notificationId)
      ) {
        clearToast();
      }
    } else {
      await invoke<DismissResponse>("dismiss_notifications", { sessionId: null });
      if (activeToast) {
        clearToast();
      }
    }
    await refreshSnapshot();
  } catch (error) {
    focusError = String(error);
    render();
    await applyWindowGeometry();
  }
}

function focusErrorMessage(response: FocusResponse): string {
  if (!response.sessionFound || response.error === "session_not_found") {
    return "会话已不存在";
  }
  return "无法唤起终端，请先让终端窗口可见后重试";
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (character) => {
    switch (character) {
      case "&":
        return "&amp;";
      case "<":
        return "&lt;";
      case ">":
        return "&gt;";
      case "\"":
        return "&quot;";
      default:
        return "&#039;";
    }
  });
}

function escapeAttr(value: string): string {
  return escapeHtml(value);
}
