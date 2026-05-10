# Claude/Codex Hook 通知工具实施计划

## 目标

实现一个 Windows 本地通知工具，用户可以从任意终端直接运行 `claude` 或 `codex`。当任务完成、失败、等待用户确认或等待继续输入时，系统弹出类似微信消息的气泡通知；目标体验是点击通知后尽量唤起原终端窗口。当前代码尚未接入 Toast 点击回调。

第一版必须自动检查和安装 Claude/Codex hooks。用户不需要手动编辑 Claude 或 Codex 配置。

## 已确认目标方案

```text
事件来源：Claude 官方 hook + Codex 官方 lifecycle hooks
触发方式：hook -> agent-notify-hook.ps1 -> agent-notify emit --stdin -> 本地后台
通知实现：后台 MVP 先用 Windows Runtime Toast；完整 Tauri 托盘应用后续接入
点击唤窗：当前只提供 Bearer 鉴权的 /focus/{sessionId}，且仅支持 HWND；Toast deep link 后续实现
Codex 边界：只支持带官方 hooks 的最新版 Codex
离线策略：后台离线、监听关闭、token 无效时直接丢弃事件；监听开关当前只在启动配置中读取
```

本机验证结果：

```text
claude --version      2.1.138 (Claude Code)
codex --version       codex-cli 0.130.0
codex features list   hooks stable true
```

## 当前实现状态

当前代码已经交付后端 MVP：

- Rust workspace 包含 `agent-notify-core`、`agent-notify`、`agent-notify-tray` 和 `hook-manager`。
- `agent-notify emit --stdin` 从 stdin 读取统一事件 JSON，去除 UTF-8 BOM，校验后 POST 到 `/events`；默认静默丢弃失败，`AGENT_NOTIFY_STRICT` 才返回非零。`AGENT_NOTIFY_ENDPOINT`、`AGENT_NOTIFY_TOKEN` 可覆盖默认 endpoint/token。
- `agent-notify-tray` 当前是 Axum localhost 后台，不是完整 Tauri 托盘 UI。它支持 `serve`、`check-hooks`、`repair-hooks`，并提供 `POST /events`、`GET /sessions`、`POST /focus/{sessionId}`。
- 所有 localhost 路由都要求 Bearer token；token 默认在 `%LOCALAPPDATA%\AgentNotify\token`，也可通过 `AGENT_NOTIFY_TOKEN` 传给 CLI。`AGENT_NOTIFY_HOME` 可覆盖运行时根目录。
- 后台维护内存 session 表和 30 秒去重窗口，按通知策略发 Windows Toast；当前 Toast 没有点击回调。
- `/focus/{sessionId}` 当前只在事件携带 HWND 时尝试 `SetForegroundWindow`，未实现 PID、父进程、窗口标题 fallback，也未打开 session 详情页。
- Hook Manager 会复制 `agent-notify-hook.ps1` 到运行时目录、生成 manifest、备份并合并 Claude/Codex 用户级 hook 配置、启用 Codex `hooks` feature。
- 当前还未实现完整 ACL 加固、备份保留策略、失败后自动回滚恢复、Tauri 托盘 UI、运行时监听开关、deep link activation nonce、Toast 按钮和 `agentrun`。

## 交付物

项目需要交付这些核心文件和模块：

```text
scripts/hooks/agent-notify-hook.ps1
scripts/hooks/claude-hook.template.json
scripts/hooks/codex-hook.template.json
src/hook-manager/
src/agent-notify/
src/agent-notify-tray/
```

- `agent-notify-hook.ps1`：Claude/Codex 共用 hook 入口，读取工具传入的 stdin payload，在内存中转换为统一事件，并写入 `agent-notify emit --stdin` 子进程。
- `claude-hook.template.json`：Claude 用户级配置合并模板。
- `codex-hook.template.json`：Codex 用户级 `hooks.json` 合并模板。
- `src/hook-manager/`：自动检查、安装、修复、备份 hooks。
- `src/agent-notify/`：CLI，提供 `agent-notify emit --stdin`。
- `src/agent-notify-tray/`：当前实现为本地 HTTP 后台、事件入口、通知和 HWND-only focus；完整 Tauri 托盘应用待接入。
- 当前测试以内联 Rust 单元测试为主，尚未建立独立 `tests/` 目录。

## 运行时目录

运行时主要写入用户本地数据目录：

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

允许写入的路径只限：

```text
%LOCALAPPDATA%\AgentNotify\**
%USERPROFILE%\.claude\settings.json
%USERPROFILE%\.codex\hooks.json
%USERPROFILE%\.codex\config.toml
```

用户级 Claude/Codex 配置应指向 `%LOCALAPPDATA%\AgentNotify\hooks\agent-notify-hook.ps1` 对应的当前用户绝对路径，不要指向开发仓库路径。安装器写入 hook 命令时必须先展开为绝对路径，不能把 `%LOCALAPPDATA%` 原样写进命令。不得修改 Claude/Codex 安装目录。

## Hook Manager

Hook Manager 是第一版的关键模块。当前已经可以在 `agent-notify-tray serve` 启动时按配置自动运行，也可以用 `check-hooks` / `repair-hooks` 显式运行。目标 UI 中它会在三个时机运行：

1. Tauri 应用启动。
2. 用户开启监听。
3. 用户点击“修复 hooks”。

检查流程：

```text
检查 claude CLI
检查 codex CLI
检查 codex hooks feature
检查本项目 hook 安装副本
检查 Claude 用户级 hook 配置
检查 Codex 用户级 hook 配置
缺失或过期 -> 备份 -> 合并安装 -> 再校验
```

状态枚举：

```text
checking
installing
repairing
missing_cli
unsupported_version
hook_missing
hook_installed
hook_outdated
hook_ok
install_failed
config_parse_failed
backup_failed
merge_conflict
write_failed
verify_failed
rollback_available
rollback_failed
permission_denied
hook_tampered
```

Hook Manager 必须幂等。重复运行不会重复插入 hook，不会覆盖用户已有 hook，不会删除用户配置。

## 自动安装规则

安装流程：

1. 创建 `%LOCALAPPDATA%\AgentNotify\hooks\`。
2. 复制最新 `agent-notify-hook.ps1`。
3. 写入 `manifest.json`，记录 hook 版本、支持事件、安装时间和脚本 SHA-256。
4. 修改 Claude/Codex 用户级配置前，备份原文件到 `backups\`。
5. 写入临时文件，校验 JSON/TOML 后原子替换。
6. 合并带本项目标识的 hook 条目。
7. 重新读取配置，确认命令路径、事件列表和脚本 hash。

合并策略：

- 只管理标记为 `managedBy: agent-notify` 且带稳定 id 的配置块。
- 用户已有其他 hooks 保留原样。
- 如果本项目 hook 指向旧路径，自动修复。
- 如果配置文件结构异常，停止安装并显示 `config_parse_failed`。
- 如果遇到无标记同名块、同命令块或重复块，进入 `merge_conflict`，不自动覆盖。
- 目标行为是在写入或校验失败时从最近备份回滚；当前实现已做写前备份和临时文件写入，但尚未自动恢复最近备份。

备份命名：

```text
backups\<tool>\<yyyyMMdd-HHmmss>-<config-name>-<sha256>.bak
```

目标要求是备份目录和 token 文件设置为当前 Windows 用户私有 ACL，并按最近 20 份或 30 天保留。当前实现会创建 token 和备份文件，但尚未做 ACL 加固和保留清理。

## Claude Hook

第一版安装这些事件：

```text
PermissionRequest   -> user.confirmation_required
Notification        -> user.input_required 或 user.confirmation_required
Stop                -> task.completed 或 user.input_required
StopFailure         -> task.failed
PostToolUseFailure  -> tool.blocked 或 task.failed
SessionEnd          -> task.completed 或 task.failed
```

Claude hook 命令调用：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File "C:\Users\alice\AppData\Local\AgentNotify\hooks\agent-notify-hook.ps1" --tool claude --hook-event Stop
```

hook 读取 Claude 传入的 stdin payload，提取：

```text
sessionId
cwd
hook event
tool name
permission / confirmation summary
best-effort pid / parentPid / window title
```

Claude hook 模板必须使用官方三层结构：`event -> matcher group -> hooks[] -> handler`。`PermissionRequest`、`PostToolUseFailure` 使用 matcher；`Stop`、`StopFailure`、`Notification`、`SessionEnd` 不要求 matcher。hook 默认不得向自身 stdout 写入统一事件 JSON。

## Codex Hook

第一版安装这些 lifecycle events：

```text
SessionStart       -> task.started
PermissionRequest  -> user.confirmation_required
Stop               -> task.completed 或 user.input_required
PostToolUse        -> tool.blocked 或 task.failed
```

Codex 只支持最新版官方 hooks。安装器必须通过 `codex features list` 确认 `hooks` 为 enabled；如果当前版本支持但未启用，写入用户配置启用。

当前本机 Codex 0.130.0 暴露用户级 `hooks.json` 形态，Windows 默认候选路径为：

```text
%USERPROFILE%\.codex\hooks.json
```

实现时仍要以当前 Codex 实际加载的配置层为准：先读取、备份，再合并本项目命名的 hook handler。不要把插件打包用的 `hooks/hooks.json` 当成用户级安装路径。若需要启用 hooks，则在 `%USERPROFILE%\.codex\config.toml` 写入：

```toml
[features]
hooks = true
```

Codex hook 模板必须使用三层结构：`event -> matcher group -> hooks[] -> handler`，并明确 `timeout` 和 `statusMessage`。`PermissionRequest`、`PostToolUse` matcher 过滤 tool name；`SessionStart` matcher 过滤启动来源；`Stop` 不使用 matcher。

Codex hook 同样调用：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File "C:\Users\alice\AppData\Local\AgentNotify\hooks\agent-notify-hook.ps1" --tool codex --hook-event PermissionRequest
```

## 统一事件

hook 生成统一 JSON，并写入 `agent-notify emit --stdin` 子进程的 stdin：

```json
{
  "version": 1,
  "eventId": "uuid",
  "eventType": "user.confirmation_required",
  "severity": "warning",
  "tool": "codex",
  "sessionId": "codex-project-20260510-153012",
  "project": {
    "cwd": "D:\\own\\project",
    "name": "project"
  },
  "process": {
    "pid": 18432,
    "parentPid": 9912
  },
  "window": {
    "title": "Windows PowerShell",
    "hwnd": null,
    "terminal": "WindowsTerminal"
  },
  "message": {
    "title": "Codex 需要确认",
    "body": "project · 当前会话 · 点击返回终端",
    "detail": "请求执行 shell 命令，参数已隐藏"
  }
}
```

统一事件默认不包含 `raw`。调试样本必须显式开启、脱敏、用户确认导出。

hook 不直接调用 Windows Toast。它只执行：

```powershell
agent-notify emit --stdin
```

hook 不得把统一事件 JSON 写到自身 stdout/stderr。Claude/Codex 会解析 hook stdout 作为控制输出，尤其 Codex `Stop` 对 stdout 更敏感。默认失败也返回 `0`，不影响主流程。

## 本地后台

当前 `agent-notify-tray` 先作为本地后台负责：

- 本地事件入口。
- 启动时读取监听开关配置。
- session 状态表。
- 通知去重。
- Windows Toast。
- Bearer 鉴权的 `/focus/{sessionId}`。

完整 Tauri 托盘 UI、hook 健康状态展示、运行时监听开关、通知点击回调和 session 详情页尚未实现。第一版使用 localhost HTTP，后续可以替换为 named pipe：

```text
POST http://127.0.0.1:17891/events
GET  http://127.0.0.1:17891/sessions
POST http://127.0.0.1:17891/focus/{sessionId}
Authorization: Bearer <AGENT_NOTIFY_TOKEN>
```

所有 localhost 接口都必须鉴权。后台离线、监听关闭、token 无效、HTTP 401/403、连接拒绝、超时或 5xx 时，`agent-notify emit` 默认退出 `0` 并丢弃事件，不向 stdout/stderr 输出 payload。

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

通知只显示脱敏摘要：

```text
标题：Codex 需要确认
正文：project · 当前会话 · 点击返回终端
详情：请求执行 shell 命令，参数已隐藏
交互：当前仅展示 Toast 文本；点击主体、忽略/静音和托盘 UI 处理待实现
```

通知不能自动批准 Claude/Codex 权限请求。

## 唤窗策略

目标是在点击通知后按优先级定位窗口：

```text
HWND
PID
父 PID / 进程树
窗口标题
Tauri session 详情页
```

Windows Terminal 多 tab 场景只承诺 best-effort：可以唤起 Terminal 窗口，不承诺切到具体 tab。

当前实现边界：

- Toast 点击没有接入 deep link。
- `/focus/{sessionId}` 必须通过 HTTP Bearer token 调用。
- 只有事件携带 HWND 时才尝试聚焦窗口。
- PID、父 PID、进程树、窗口标题和 Tauri session 详情页是后续工作。

目标实现规则：

- HWND 先校验 `IsWindow`、`IsWindowVisible` 和进程归属。
- PID 匹配时校验启动时间，避免 PID 复用。
- 进程树匹配 shell、conhost/OpenConsole、WindowsTerminal.exe。
- 标题匹配只作为最后降级，冲突时不自动聚焦。
- `SetForegroundWindow` 失败时使用任务栏闪烁并打开 session 详情。

## 测试计划

单元测试：

- Claude payload -> 统一事件。
- Codex payload -> 统一事件。
- 敏感信息脱敏。
- 事件去重。
- hook 配置合并：保留用户 hook、只替换 agent-notify 块、旧路径修复、重复块冲突。
- hook 配置备份、原子写入、回滚和中断恢复。
- 稳定 `eventId` 生成。

集成测试：

- 本地后台接收 `agent-notify emit --stdin`。
- 启动配置中监听关闭时事件丢弃。
- token 无效时事件丢弃。
- HTTP 401/403、连接拒绝、超时、5xx 都静默丢弃。
- 缺失 Claude hook 时自动安装。
- 缺失 Codex hook 时自动安装。
- 删除 hook 后“修复 hooks”可恢复。
- Toast 主体点击 deep link 能聚焦窗口或打开 session 详情。当前未实现。
- 唤窗矩阵：PowerShell 独立窗口、CMD、Windows Terminal 单窗口、多 tab、定位失败。当前仅覆盖 HWND 路径。

手动验收：

- 从任意 PowerShell 启动 `claude`，等待确认时弹通知。
- 从任意 PowerShell 启动 `codex`，PermissionRequest 时弹通知。
- Codex Stop 时弹完成或等待输入通知。
- 点击通知能回到对应终端窗口或打开 session 详情。当前未实现，属于后续验收。

## 里程碑

1. 验证 Toast 和窗口唤起。
2. 已实现本地事件入口；Tauri 托盘壳待实现。
3. 已实现 `agent-notify emit --stdin`。
4. 已实现 Hook Manager 自动检查和安装的核心路径。
5. 已实现 Claude hook adapter 的通用映射。
6. 已实现 Codex hook adapter 的通用映射。
7. 已实现通知策略、去重和安全脱敏的核心逻辑。
8. 端到端真实 Claude/Codex 触发验收、Toast 点击回调和 UI 验收待完成。

## MVP 验收标准

后端 MVP 当前已覆盖：

- 缺失 hooks 时能自动安装。
- 安装前会备份用户配置。
- 从任意终端直接运行 `claude` 或 `codex` 都能触发 hook。
- 后台在线且监听开启时能弹 Windows 通知。
- 后台离线或监听关闭时事件直接丢弃。
- 通知不显示完整命令、完整输出或 secrets。
- 通知不自动批准任何权限请求。
- 所有本地 HTTP 接口都有 token 鉴权。
- raw payload 默认不持久化。

完整产品 MVP 仍需补齐：

- Tauri 托盘 UI 显示 Claude/Codex hook 健康状态。
- 运行时监听开关和“修复 hooks”按钮。
- Toast 主体点击 deep link 和 activation nonce。
- PID/父进程/窗口标题 fallback，无法定位时打开 session 详情。
