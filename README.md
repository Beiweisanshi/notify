# Agent Notify

Windows 本地通知工具，用于 Claude Code、Codex CLI 和其他长时间运行的终端任务。Claude/Codex 通过官方 hooks 上报统一事件，本地后台负责鉴权、去重、Windows Toast 通知和点击后 best-effort 唤起原终端窗口。

当前 MVP 以“后台服务先可交付”为边界：`agent-notify-tray` 先承担本地后台、事件入口、通知触发和 hook 健康检查能力。完整 Tauri 托盘 UI 后续可接入同一套 Rust 库；如果当前构建尚未提供完整托盘界面，不应把它当作已完成的 UI 产品。

## 当前 MVP 组件

目标源码结构：

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
tests/
```

- `scripts/hooks/agent-notify-hook.ps1`：Claude/Codex 共用 hook 入口，从 stdin 读取工具 payload，转换为统一事件，并写入 `agent-notify emit --stdin` 子进程。
- `scripts/hooks/claude-hook.template.json`：Claude 用户级 hook 配置合并模板。
- `scripts/hooks/codex-hook.template.json`：Codex 用户级 `hooks.json` 合并模板。
- `src/agent-notify-core/`：共享 Rust 库，承载统一事件模型、配置路径、脱敏、去重和通知策略。
- `src/hook-manager/`：检测、备份、安装、修复和校验 Claude/Codex hooks。
- `src/agent-notify/`：CLI，主要供 hook 调用 `agent-notify emit --stdin`。
- `src/agent-notify-tray/`：Windows/Tauri 后台入口，负责本地 HTTP、session 状态、通知去重、Toast 和唤窗。

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

Hook Manager 额外只允许修改这些用户级配置：

```text
%USERPROFILE%\.claude\settings.json
%USERPROFILE%\.codex\hooks.json
%USERPROFILE%\.codex\config.toml
```

## Build And Test

如果当前检出缺少某些 workspace 成员目录、`Cargo.toml` 或 `scripts/hooks/`，说明并行实现源码还未完整合入，此时完整 workspace 构建可能失败；可先用下面命令检查仓库状态：

```powershell
git status --short --branch
git log --oneline --decorate -5
```

完整 MVP 源码合入后的标准命令应保持为：

```powershell
cargo fmt --check
cargo test --workspace
cargo build --release --workspace
```

如果后续 Tauri 前端包提供 `package.json`，再运行对应前端检查：

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

token 必须是随机高熵值，文件必须设置为当前 Windows 用户私有 ACL，不能写入日志。所有 localhost 路由都必须鉴权，不只是 `/events`。

监听关闭、后台离线、token 无效、HTTP 401/403、连接拒绝、超时或 5xx 时，事件静默丢弃，不写离线队列，不补发过期通知；`agent-notify emit` 默认退出 `0`，且不向 stdout/stderr 打印 payload、token 或完整错误堆栈。

## Windows/Tauri 状态

MVP 当前面向 Windows。Tauri 是预留的常驻托盘外壳和通知运行时，但后台服务能力先交付：本地 HTTP、token 鉴权、事件解析、通知策略和 hook 检查应先可用。完整托盘 UI、session 面板、静音入口和“修复 hooks”按钮可以后续接入同一套 Rust 库；在这些 UI 尚未提供时，不应声称已经有完整托盘产品。

Windows Toast 交互只承诺点击通知主体。点击 payload 使用 `agent-notify://focus?activationId=...`，其中 `activationId` 是后台生成、短期有效、单次使用的本机 nonce；它绑定到 session/event，但不是 Bearer token。Windows Terminal 多 tab 只能保证尽量唤起整个窗口，不保证切到具体 tab。

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

## Hook 自动安装与修复

MVP 要求用户可以从任意终端直接运行 `claude` 或 `codex`，不需要手动编辑 hook 配置。`agent-notify-tray` 在这些时机执行同一套幂等检查流程：

- 后台启动时。
- 用户开启监听时。
- 用户触发“修复 hooks”动作时。

后台服务先交付阶段如果还没有完整托盘 UI，启动或重启 `agent-notify-tray` 就是触发 hook 检查和修复的可用入口。

检查流程包括：

- 执行 `claude --version`、`codex --version` 和 `codex features list`。
- 确认 Codex 使用支持官方 lifecycle hooks 的版本；如果需要，写入 `%USERPROFILE%\.codex\config.toml` 启用 `codex_hooks`。
- 将 `agent-notify-hook.ps1` 安装到 `%LOCALAPPDATA%\AgentNotify\hooks\`，并生成 `manifest.json` 记录版本、事件列表和 SHA-256。
- 修改 Claude/Codex 用户级配置前，备份到 `%LOCALAPPDATA%\AgentNotify\backups\<tool>\`。
- 只合并带 `managedBy: agent-notify` 和稳定 id 的配置块；保留用户已有 hooks。
- 遇到无标记同名块、同命令块或重复块时进入 `merge_conflict`，不自动覆盖。
- 写入临时文件并校验 JSON/TOML 后再原子替换；失败时尝试回滚最近备份。

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

Windows MVP 只承诺点击通知主体，通过 `agent-notify://focus?activationId=...` 尽量唤起原终端窗口。后台收到 deep link 后用内存中的短期 nonce 找回绑定的 `sessionId` 和 `eventId`；不得把 Bearer token 放进 URI。Windows Terminal 多 tab 场景只承诺唤起整个 Terminal 窗口，不保证切到具体 tab。Toast 按钮动作不属于当前 MVP。

## 安全边界

本项目第一版不做这些事：

- 不自动批准 Claude/Codex 权限请求。
- 不在通知中执行命令或继续对话。
- 不上传 payload 到外部服务。
- 不读取完整终端屏幕内容。
- 不修改 Claude/Codex 安装目录。
- 默认不持久化 raw payload。

日志只允许记录时间、组件、事件类型、错误码和耗时，默认不记录 raw payload。离线、监听关闭或鉴权失败时事件静默丢弃。`hook.log`、`config.json`、token 文件和备份目录必须使用当前 Windows 用户私有 ACL。调试导出 raw payload 必须由用户显式开启、先脱敏、再确认导出。
