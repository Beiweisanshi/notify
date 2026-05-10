# Hook Requirements

本文档定义本项目内置 hook 和运行时安装器必须满足的条件。Claude/Codex 主路径只使用官方 hooks；普通长任务 wrapper 是后续可选能力，不得作为 Codex fallback。

## 当前实现状态

当前代码已实现：

- `agent-notify-hook.ps1` 读取 stdin、参数和环境变量，生成统一事件，并通过 `agent-notify emit --stdin` 转发。
- `agent-notify emit --stdin` 读取事件 JSON，校验后用 Bearer token POST 到本地后台 `/events`。
- `agent-notify-tray` 当前是 Axum localhost 后台，提供 `/events`、`/sessions`、`/focus/{sessionId}`，所有路由均鉴权。
- Hook Manager 会复制运行时 hook、生成 manifest、备份并合并 Claude/Codex 用户级配置，并把 hook 命令写成当前用户的绝对路径。

当前尚未实现完整 Tauri 托盘 UI、Toast 点击 deep link、activation nonce、PID/标题 fallback、ACL 加固、备份保留清理和自动回滚恢复。

## 项目交付文件

项目至少应实现这些文件，而不是只提供示例：

```text
scripts/hooks/agent-notify-hook.ps1
scripts/hooks/claude-hook.template.json
scripts/hooks/codex-hook.template.json
src/hook-manager/
src/agent-notify-core/
src/agent-notify/
src/agent-notify-tray/
```

- `agent-notify-hook.ps1` 是通用 hook 入口，当前由 Claude/Codex hooks 调用；普通长任务 wrapper 待实现。
- `claude-hook.template.json` 是 Claude 用户级 hook 合并参考模板。
- `codex-hook.template.json` 是 Codex 用户级 `hooks.json` 合并参考模板。
- `src/hook-manager/` 负责检测、备份、安装、修复、校验 Claude/Codex hooks。

模板文件必须带本项目标识，例如 `agent-notify` 或 `managedBy = "agent-notify"`，便于后续幂等更新和卸载。当前 Hook Manager 代码会根据内部模板生成等价配置，并在安装时把 hook 路径展开为绝对路径；仓库中的 JSON 模板是参考/来源模板，不应原样把 `{{agentNotifyHookPath}}` 写入用户配置。

## 运行时安装文件

应用运行后应把 hook 安装到用户本地数据目录，避免用户级 CLI 配置指向开发仓库路径：

```text
%LOCALAPPDATA%\AgentNotify\
  hooks\
    agent-notify-hook.ps1
    manifest.json
  backups\
  config.json
  token
  logs\
```

- `hooks\agent-notify-hook.ps1` 是实际被 Claude/Codex 调用的脚本副本。
- `hooks\manifest.json` 保存 hook 版本、源版本、安装时间、支持事件和脚本 SHA-256。
- `backups\` 保存每次修改 Claude/Codex 用户配置前的备份。
- 用户级 Claude/Codex 配置只写入调用该 hook 副本的命令。

## 运行时写入文件

hook 运行时只能写入用户本地数据目录。Hook Manager 额外允许修改明确列出的 Claude/Codex 用户级配置：

```text
%LOCALAPPDATA%\AgentNotify\
  config.json
  hooks\
  backups\
  logs\
%USERPROFILE%\.claude\settings.json
%USERPROFILE%\.codex\hooks.json
%USERPROFILE%\.codex\config.toml
```

写入规则：

- `config.json` 保存本地后台地址、端口、通知开关、去重窗口和 hook 安装配置；token 存放在单独的 `token` 文件中。
- `logs\hook.log` 只记录时间、组件、事件类型、错误码、耗时，不记录 payload、完整 cwd、命令参数、token、Authorization header、终端输出或异常堆栈中的 payload。
- `backups\` 只保存自动安装前的用户配置备份。目标要求是当前 Windows 用户私有 ACL；当前代码尚未显式加固 ACL。
- 后台离线、监听关闭或 token 无效时，事件直接丢弃，不写离线队列，不补发过期通知。

hook 不应写入项目源码目录，除非用户明确要求导出调试样本。

## 自动检查和安装

当前 `agent-notify-tray serve` 启动时会在 `auto_check` 和 `auto_install` 均启用时运行 Hook Manager；`agent-notify-tray check-hooks` 和 `agent-notify-tray repair-hooks` 也会执行同一套安装/修复流程。Tauri UI 中的监听开关和“修复 hooks”按钮是后续入口。

1. 检测 `claude --version`，标记 Claude CLI 是否可用。
2. 检测 `codex --version` 和 `codex features list`，确认 Codex CLI 可用且 `hooks` 为 enabled。
3. 校验 `%LOCALAPPDATA%\AgentNotify\hooks\agent-notify-hook.ps1` 是否存在且版本匹配。
4. 不存在或过期时，从项目随包资源复制最新 hook 到本地数据目录。
5. 读取 Claude 用户级 hook 配置，缺失本项目 hook 时合并安装。
6. 读取 Codex 用户级 `hooks.json` 或 `config.toml`，缺失本项目 hook 时合并安装。
7. 每次修改用户配置前先备份；安装后写入 manifest。目标行为还包括完整重读验证和失败回滚。

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

自动安装不得覆盖用户已有 hook，不得删除用户配置，不得修改 Claude/Codex 安装目录。

备份规则：

```text
%LOCALAPPDATA%\AgentNotify\backups\<tool>\<yyyyMMdd-HHmmss>-<config-name>-<sha256>.bak
```

写入规则：

- 只修改带 `managedBy: agent-notify` 和稳定 id 的配置块。
- 遇到无标记同名块、同命令块或重复块时进入 `merge_conflict`，不自动覆盖。
- 备份成功后写临时文件，校验 JSON/TOML 后原子替换。
- 目标要求是安装后校验 hook 命令、事件列表、版本标记和脚本 SHA-256；当前实现会写入 manifest，但未做完整触发验证。
- 目标要求是写入或校验失败时尝试回滚最近备份；当前实现只做写前备份，尚未自动恢复。

## 输入来源

hook 必须支持三种输入：

1. 从 `stdin` 读取 Claude/Codex 传入的 JSON payload。
2. 从命令行参数读取显式事件，例如 `--tool claude --event task.completed`。
3. 从环境变量补齐上下文。

推荐环境变量：

```text
AGENT_NOTIFY_SESSION_ID
AGENT_NOTIFY_TOOL
AGENT_NOTIFY_PROJECT
AGENT_NOTIFY_WINDOW_TITLE
AGENT_NOTIFY_PARENT_PID
AGENT_NOTIFY_WINDOW_HWND
AGENT_NOTIFY_TERMINAL
AGENT_NOTIFY_CWD
```

`AGENT_NOTIFY_TOKEN` 由 `agent-notify` CLI 读取，优先级高于 token 文件；hook 脚本本身不需要读取 token。token 不能继续传给 Claude/Codex 子进程或写入日志，默认从当前用户本地 token 文件读取。

运行时根目录可通过 `AGENT_NOTIFY_HOME` 覆盖。CLI 调试可使用 `AGENT_NOTIFY_ENDPOINT`、`AGENT_NOTIFY_TOKEN`、`AGENT_NOTIFY_STRICT`；hook 严格模式使用 `AGENT_NOTIFY_STRICT_HOOK=1`。

## 输出事件格式

hook 必须生成统一 JSON 事件，并写入 `agent-notify emit --stdin` 子进程的 stdin。hook 不得把该 JSON 写到自身 stdout/stderr，因为 Claude/Codex 会把 stdout 作为 hook 控制输出解析。

```json
{
  "version": 1,
  "eventId": "uuid",
  "eventType": "user.confirmation_required",
  "severity": "warning",
  "tool": "claude",
  "sessionId": "claude-backend-20260510-153012",
  "project": {
    "cwd": "D:\\repo\\project",
    "name": "project"
  },
  "process": {
    "pid": 12345,
    "parentPid": 8888
  },
  "window": {
    "title": "AI-MONITOR:session-id",
    "hwnd": null,
    "terminal": "WindowsTerminal"
  },
  "message": {
    "title": "Claude 需要确认",
    "body": "backend 会话正在等待你处理",
    "detail": "可选的简短确认摘要"
  }
}
```

统一事件默认不包含 `raw`。调试样本必须用户显式开启、脱敏、确认导出；默认不持久化 raw payload。

必填字段：

```text
version
eventId
eventType
severity
tool
sessionId
project.cwd
message.title
message.body
```

## 支持事件

第一版 hook 只需要支持这些事件：

```text
task.completed
task.failed
user.confirmation_required
user.input_required
tool.blocked
heartbeat
```

其中必须触发系统通知的事件：

```text
task.completed
task.failed
user.confirmation_required
user.input_required
tool.blocked
```

`heartbeat` 只更新状态，不弹通知。

## 工具事件映射

Claude 和 Codex 都使用官方 hook，不依赖输出关键词监听。

Claude 推荐映射：

```text
PermissionRequest   -> user.confirmation_required
Notification        -> user.confirmation_required 或 user.input_required
Stop                -> task.completed 或 user.input_required
StopFailure         -> task.failed
PostToolUseFailure  -> tool.blocked 或 task.failed
SessionEnd          -> task.completed 或 task.failed
```

Codex 推荐映射：

```text
SessionStart       -> task.started
PermissionRequest  -> user.confirmation_required
Stop               -> task.completed 或 user.input_required
PostToolUse        -> tool.blocked 或 task.failed
```

当前方案只支持带官方 hooks 的 Codex 最新版本。

Codex 0.130.0 的 hook 配置应按用户级 `%USERPROFILE%\.codex\hooks.json` 或 `%USERPROFILE%\.codex\config.toml` 合并，不使用输出监听或 wrapper fallback。不要把插件打包用的 `hooks/hooks.json` 当成用户级安装路径。若需要启用 hooks，写入：

```toml
[features]
codex_hooks = true
```

Claude 和 Codex 的模板都必须采用三层结构：`event -> matcher group -> hooks[] -> handler`。`PermissionRequest` / `PostToolUse` 类事件使用 matcher；`Stop` 类事件不使用 matcher，并要求 hook stdout 为空。

## 触发系统通知

hook 不直接调用 Windows Toast API。hook 只负责把事件交给本地后台，由后台统一触发系统通知。

首选方式：

```powershell
agent-notify emit --stdin
```

hook 将标准事件 JSON 写入 `stdin`，由 `agent-notify` 转发给后台。

备用方式：

```text
POST http://127.0.0.1:17891/events
Authorization: Bearer <AGENT_NOTIFY_TOKEN>
Content-Type: application/json
```

`/events`、`/sessions`、`/focus/{sessionId}` 等所有 localhost 接口都必须使用 Bearer token。当前代码会生成随机本地 token 文件且不写入日志；目标要求是 token 文件使用当前用户私有 ACL，并支持手动轮换。

失败降级：

1. 如果后台在线，发送事件后退出。
2. 如果后台离线、监听关闭、token 无效、HTTP 401/403、连接拒绝、超时或 5xx，直接丢弃事件。
3. 丢弃事件时可写一条白名单 `hook.log`，但不阻塞 Claude/Codex。

## 性能和失败行为

hook 必须满足：

- 总执行时间默认不超过 2 秒。
- stdin 读取和 payload 解析预算 300ms。
- `agent-notify emit` 或 HTTP 请求预算 1500ms。
- 日志和清理预算 200ms。
- 后台不可用时默认不重试；如果未来允许 1 次重试，也必须包含在 2 秒预算内。
- hook 失败不能导致 Claude/Codex 主流程失败。
- hook 退出码默认返回 `0`，除非用户显式开启严格模式。
- 同一 payload 重放时必须使用稳定 `eventId`：`sha256(tool + sessionId + hookEvent + payloadStableId + sanitizedSummaryHash)`。

## 通知内容规则

具体展示策略见 `NOTIFICATION_POLICY.md`。

默认通知内容只显示：

```text
工具名
项目名
事件类型
简短状态
```

默认不显示完整命令、完整终端输出、密钥、路径中的敏感参数。

推荐通知模板：

```text
标题：{Claude|Codex} {需要确认|等待输入|任务完成|执行失败}
正文：{项目名} · {会话名或目录名} · 点击返回终端
详情：默认隐藏完整命令；只显示工具名和脱敏摘要
```

确认类通知默认只显示脱敏类别，例如 `Bash 请求执行命令，参数已隐藏`。`message.detail` 最多 160 个字符，超过必须截断。当前 Windows MVP 只展示 Toast 文本，未实现点击回调；后续点击 deep link 必须使用短期 activation nonce，不能把 Bearer token 放进 URI。“忽略/静音”放在后续 Tauri 托盘 UI 中，不在 Toast 按钮中承诺。不允许在通知里批准命令。

## 安全边界

hook 禁止做这些事：

- 自动批准 Claude/Codex 权限请求。
- 执行用户没有明确要求的命令。
- 上传 payload 到外部服务。
- 修改 Claude/Codex 安装文件。
- 读取完整终端缓冲区或截图。
- 把敏感 payload 原样写入通知内容。

## 验收条件

生成的 hook 视为合格需要满足：

1. 能从 `stdin` 读取 payload。
2. 能生成标准事件 JSON。
3. 能通过 `agent-notify emit --stdin` 触发后台通知。
4. 后台离线、监听关闭或 token 无效时能静默丢弃事件。
5. 聚焦所需的 `sessionId`、窗口标题、PID/HWND 信息能被携带；当前后台只使用 HWND。
6. 不会因为 hook 自身错误中断 Claude/Codex。
7. 不会在通知中泄露完整命令或敏感内容。
