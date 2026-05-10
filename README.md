# Agent Notify

Windows 本地通知工具，用于 Claude Code、Codex CLI 和其他长时间运行的终端任务。Claude/Codex 通过官方 hooks 上报统一事件，本地后台负责鉴权、去重、Windows Toast 通知和 session 状态维护。

当前 MVP 以“后台服务先可交付”为边界：`agent-notify-tray` 目前是 Axum localhost 后台入口，先承担事件入口、通知触发、当前用户 Start Menu AppID 注册、hook 检查/修复和 HTTP focus 接口。完整 Tauri 托盘 UI、Toast 点击 deep link、activation nonce、PID/标题 fallback 和 `agentrun` 仍是后续计划。

## 当前 MVP 组件

当前源码结构：

```text
scripts/hooks/
  agent-notify-hook.ps1
  claude-hook.template.json
  codex-hook.template.json
src/
  agent-notify-core/
  hook-manager/
  agent-notify/
  agent-notify-tray/
```

- `scripts/hooks/agent-notify-hook.ps1`：Claude/Codex 共用 hook 入口，从 stdin 读取工具 payload，转换为统一事件，并写入 `agent-notify emit --stdin` 子进程。
- `scripts/hooks/claude-hook.template.json`：Claude 用户级 hook 配置合并模板。
- `scripts/hooks/codex-hook.template.json`：Codex 用户级 `hooks.json` 合并模板。
- `src/agent-notify-core/`：共享 Rust 库，承载统一事件模型、配置路径、脱敏、去重和通知策略。
- `src/hook-manager/`：检测、备份、安装、修复和校验 Claude/Codex hooks。
- `src/agent-notify/`：CLI，主要供 hook 调用 `agent-notify emit --stdin`。
- `src/agent-notify-tray/`：当前是本地 HTTP 后台入口，负责 session 状态、通知去重、Windows Toast 和 `/focus/{sessionId}`；完整 Tauri 托盘壳尚未接入。

运行时写入当前用户本地目录：

```text
%LOCALAPPDATA%\AgentNotify\
  config.json
  token
  hooks\
    agent-notify-hook.ps1
    manifest.json
  backups\
  logs\
    hook.log
```

设置 `AGENT_NOTIFY_HOME` 可以覆盖运行时根目录。`agent-notify emit` 还支持 `AGENT_NOTIFY_ENDPOINT`、`AGENT_NOTIFY_TOKEN` 和 `AGENT_NOTIFY_STRICT`；默认从 `config.json` 和 `token` 文件读取 endpoint/token，失败时静默退出 `0`。

Hook Manager 额外只允许修改这些用户级配置：

```text
%USERPROFILE%\.claude\settings.json
%USERPROFILE%\.codex\hooks.json
%USERPROFILE%\.codex\config.toml
```

Windows Toast 还会在当前用户开始菜单目录创建或更新快捷方式，用于让 Windows 为桌面程序分配可通知的 AppID：

```text
%APPDATA%\Microsoft\Windows\Start Menu\Programs\Agent Notify.lnk
```

## Build And Test

```powershell
git status --short --branch
git log --oneline --decorate -5
```

标准检查命令：

```powershell
cargo fmt --check
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo build --workspace
```

如果当前机器没有 MSVC `link.exe`，可临时使用 GNU toolchain 和 ASCII target 目录验证：

```powershell
$env:CARGO_TARGET_DIR = "D:\own\notify-target"
cargo +stable-x86_64-pc-windows-gnu test --workspace
```

后续 Tauri 前端包提供 `package.json` 后，再补充前端检查：

```powershell
npm install
npm test
npm run tauri dev
```

从源码启动后台的常见形式：

```powershell
cargo run --release -p agent-notify-tray
```

安装后直接启动：

```powershell
agent-notify-tray
```

## 启动后台与本地接口

启动 `agent-notify-tray` 后，后台监听 localhost HTTP：

```text
POST http://127.0.0.1:17891/events
GET  http://127.0.0.1:17891/sessions
POST http://127.0.0.1:17891/focus/{sessionId}
Authorization: Bearer <AGENT_NOTIFY_TOKEN>
```

Bearer token 存放在：

```text
%LOCALAPPDATA%\AgentNotify\token
```

token 由当前代码自动生成并写入本地文件，不能写入日志。目标要求是 token 文件设置为当前 Windows 用户私有 ACL；当前实现尚未显式加固 ACL。所有 localhost 路由都必须鉴权，不只是 `/events`。

监听关闭、后台离线、token 无效、HTTP 401/403、连接拒绝、超时或 5xx 时，事件静默丢弃，不写离线队列，不补发过期通知；`agent-notify emit` 默认退出 `0`，且不向 stdout/stderr 打印 payload、token 或完整错误堆栈。

## Windows/Tauri 状态

MVP 当前面向 Windows。Tauri 是预留的常驻托盘外壳和通知运行时，但当前代码先交付后端服务能力：本地 HTTP、token 鉴权、事件解析、通知策略、hook 检查/修复和内存 session 表。完整托盘 UI、session 面板、静音入口和“修复 hooks”按钮尚未实现。

当前 Windows Toast 只负责展示文本，没有接入点击回调。`agent-notify-tray serve` 会创建或更新当前用户的 `Agent Notify.lnk`，发送通知时通过 `Get-StartApps` 获取 `Agent Notify` 的 AppID；如果该 AppID 暂不可用，会降级尝试 Windows PowerShell AppID 并把失败写入后台 stderr 日志。`POST /focus/{sessionId}` 已存在并要求 Bearer token，但当前只在 session 携带 HWND 时尝试 `SetForegroundWindow`；PID、父进程、窗口标题 fallback 和 `agent-notify://focus?activationId=...` deep link 是后续 Tauri/原生 Toast 集成计划。Bearer token 不得放进 URI。

## 手动发送事件

后台运行后，可以用 `agent-notify emit --stdin` 手动发送一个脱敏事件：

```powershell
@'
{
  "version": 1,
  "eventId": "manual-demo-001",
  "eventType": "user.confirmation_required",
  "severity": "warning",
  "tool": "codex",
  "sessionId": "codex-demo-session",
  "project": {
    "cwd": "D:\\own\\project",
    "name": "project"
  },
  "message": {
    "title": "Codex 需要确认",
    "body": "project · 当前会话 · 点击返回终端",
    "detail": "请求执行 shell 命令，参数已隐藏"
  }
}
'@ | agent-notify emit --stdin
```

hook 脚本也只应调用同一入口：

```powershell
agent-notify emit --stdin
```

hook 不直接调用 Windows Toast API，也不得把统一事件 JSON 写到 hook 自身 stdout/stderr，避免影响 Claude/Codex 对 hook 控制输出的解析。

当前 CLI 只支持 `emit --stdin`；`--endpoint`、`--token`、`--timeout-ms` 用于调试或测试本地后台。

## Hook 自动安装与修复

MVP 要求用户可以从任意终端直接运行 `claude` 或 `codex`，不需要手动编辑 hook 配置。当前 `agent-notify-tray` 在启动 `serve` 时会按配置自动运行 Hook Manager，也可以显式执行：

```powershell
agent-notify-tray check-hooks
agent-notify-tray repair-hooks
```

用户开启监听时自动检查、托盘按钮“修复 hooks”和 hook 健康状态面板仍属于后续 UI 工作。

检查流程包括：

- 执行 `claude --version`、`codex --version` 和 `codex features list`。
- 确认 Codex 使用支持官方 lifecycle hooks 的版本；如果需要，写入 `%USERPROFILE%\.codex\config.toml` 启用 `hooks`。
- 将 `agent-notify-hook.ps1` 安装到 `%LOCALAPPDATA%\AgentNotify\hooks\`，并生成 `manifest.json` 记录版本、事件列表和 SHA-256。
- 修改 Claude/Codex 用户级配置前，备份到 `%LOCALAPPDATA%\AgentNotify\backups\<tool>\`。
- 只合并带 `managedBy: agent-notify` 和稳定 id 的配置块；保留用户已有 hooks。
- 遇到无标记同名块、同命令块或重复块时进入 `merge_conflict`，不自动覆盖。
- 写入临时文件并校验 JSON/TOML 后替换目标文件。

当前 Hook Manager 已做备份和幂等合并，但还没有完整 ACL 加固、备份保留策略、失败自动回滚恢复和真实 Claude/Codex 触发后校验。

Codex 首次发现新 hook 时可能提示 `hooks need review`。这是 Codex 的安全审核机制；安装器只负责写入可审核的用户级配置，不自动批准。看到提示后，在 Codex 中打开 `/hooks`，确认这 4 个 `agent-notify` hook 命令都指向 `%LOCALAPPDATA%\AgentNotify\hooks\agent-notify-hook.ps1` 的展开后绝对路径，再手动批准。

Claude/Codex hook 命令指向安装器展开后的运行时绝对路径，例如：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File "C:\Users\alice\AppData\Local\AgentNotify\hooks\agent-notify-hook.ps1" --tool claude --hook-event Stop
powershell -NoProfile -ExecutionPolicy Bypass -File "C:\Users\alice\AppData\Local\AgentNotify\hooks\agent-notify-hook.ps1" --tool codex --hook-event PermissionRequest
```

不要把 `%LOCALAPPDATA%` 原样写进 Claude/Codex hook 命令；hook 可能不由 PowerShell shell 解释，`powershell -File` 也不会展开这个占位符。Hook Manager 必须在写入用户级配置前替换为当前用户的绝对路径。

## 通知策略

必须弹通知的事件：

```text
task.completed
task.failed
user.confirmation_required
user.input_required
tool.blocked
```

不弹通知的事件：

```text
task.started
heartbeat
```

通知默认只显示工具名、项目名、事件类型和简短脱敏状态。详情最多 160 个字符，默认隐藏完整命令参数、完整终端输出、token、Authorization header、完整 cwd 和路径中的敏感片段。同一个 `sessionId`、`eventType`、`eventId` 或摘要 hash 在 30 秒内只弹一次；状态变化必须更新 session 状态并绕过去重。

当前 MVP 只承诺展示 Windows Toast。点击通知主体、`agent-notify://focus?activationId=...`、短期 activation nonce 和 Toast 按钮动作尚未实现；后续实现时 deep link 只能携带短期 nonce，不能携带 Bearer token。已实现的 `/focus/{sessionId}` 是 Bearer 鉴权 HTTP 接口，当前只支持 HWND best-effort 聚焦。

## 安全边界

本项目第一版不做这些事：

- 不自动批准 Claude/Codex 权限请求。
- 不在通知中执行命令或继续对话。
- 不上传 payload 到外部服务。
- 不读取完整终端屏幕内容。
- 不修改 Claude/Codex 安装目录。
- 默认不持久化 raw payload。

日志只允许记录时间、组件、事件类型、错误码和耗时，默认不记录 raw payload。离线、监听关闭或鉴权失败时事件静默丢弃。目标要求是 `hook.log`、`config.json`、token 文件和备份目录使用当前 Windows 用户私有 ACL；当前代码尚未显式加固 ACL。调试导出 raw payload 必须由用户显式开启、先脱敏、再确认导出。
