# Claude/Codex 通用任务通知工具计划

## 目标

做一个本地后台通知工具，用来监控多个 Claude Code 和 Codex CLI 终端会话。当任务完成、进入等待用户确认、需要用户继续输入、执行失败或权限被阻塞时，自动发出 Windows 系统气泡通知。目标体验是用户点击通知后尽量唤起对应终端窗口；当前代码尚未接入 Toast 点击回调。

第一阶段不做完整终端替代品，只做“自动安装 hooks + 通用 hook + 本地通知后台 + 通知展示”的轻量系统。窗口唤起当前只实现了 Bearer 鉴权 `/focus/{sessionId}` 的 HWND 路径。

## 当前实现状态

截至 2026-05-11，仓库已经实现后端 MVP：

- `agent-notify-core`：统一事件模型、Claude/Codex hook payload 映射、脱敏、稳定 `eventId`、通知策略、30 秒去重、运行时配置和 token 文件。
- `agent-notify`：当前只支持 `agent-notify emit --stdin`，从 stdin 读取 JSON、去除 UTF-8 BOM、校验事件后 POST 到 `/events`；失败默认退出 `0`，`AGENT_NOTIFY_STRICT` 才暴露错误。`AGENT_NOTIFY_ENDPOINT`、`AGENT_NOTIFY_TOKEN` 可覆盖默认 endpoint/token。
- `agent-notify-tray`：当前是 Axum localhost 后台，不是完整 Tauri 托盘 UI。支持 `serve`、`check-hooks`、`repair-hooks`，提供 `POST /events`、`GET /sessions`、`POST /focus/{sessionId}`，所有路由都要求 Bearer token。`AGENT_NOTIFY_HOME` 可覆盖运行时根目录。
- Windows Toast：当前通过 PowerShell Windows Runtime Toast 展示标题、正文和详情；未实现点击回调、deep link、activation nonce 或 Toast 按钮。
- `/focus/{sessionId}`：当前只在 session 里有 HWND 时尝试 `ShowWindow` + `SetForegroundWindow`；PID、父 PID、进程树、窗口标题 fallback 和 session 详情页未实现。
- Hook Manager：已复制运行时 hook、生成 manifest、备份并合并 Claude/Codex 用户级配置、启用 Codex `hooks` feature，并写入展开后的绝对 hook 路径。ACL 加固、备份保留清理、失败自动回滚恢复和真实触发验证仍未实现。
- `agentrun`、完整 Tauri 托盘 UI、运行时监听开关、静音入口和 session 面板仍是后续计划。

## 使用场景

- 同时打开多个终端运行 Claude/Codex。
- 用户离开当前终端去做别的事情。
- 某个会话完成任务、失败、等待批准命令、等待继续输入时，系统通知提醒用户。
- 目标体验是用户点击通知后切回对应的终端窗口；当前 Toast 点击回调尚未实现。

## 总体架构

```text
agent-notify-tray 启动
        |
        | 自动检查 / 安装 / 修复 hooks
        v
Claude Code hook / Codex hook
        |
        | 统一 JSON 事件
        v
本地通知后台 agent-notifyd
        |
        | Windows Toast 通知
        v
系统通知中心 / 气泡通知
        |
        | 后续点击通知
        v
agent-notifyd 根据 sessionId/HWND 唤起窗口
```

系统拆成五层：

1. Hook 管理层：检测 Claude/Codex CLI 和用户级 hooks，缺失时自动安装。
2. 事件来源层：Claude hook、Codex hook；普通长任务 wrapper 待实现。
3. 事件协议层：把不同工具的事件统一成同一种 JSON。
4. 通知后台层：本地常驻进程，负责接收事件、去重、展示通知、保存 session 状态。
5. 窗口控制层：当前根据 HWND 唤起窗口；PID、窗口标题等 fallback 待实现。

## 统一事件模型

所有适配器最终都发送同一种 JSON 给通知后台。

```json
{
  "version": 1,
  "eventId": "uuid",
  "eventType": "task.completed",
  "severity": "info",
  "tool": "claude",
  "sessionId": "claude-backend-20260510-153012",
  "project": {
    "cwd": "D:\\repo\\project",
    "name": "project"
  },
  "process": {
    "pid": 12345,
    "parentPid": 8888,
    "startedAt": "2026-05-10T15:30:12+08:00"
  },
  "window": {
    "title": "AI: claude backend",
    "hwnd": null,
    "terminal": "WindowsTerminal"
  },
  "message": {
    "title": "Claude 任务完成",
    "body": "backend 会话已完成，点击返回窗口。",
    "detail": "最后状态摘要或确认问题"
  }
}
```

统一事件默认不携带 Claude/Codex 原始 payload。调试样本必须由用户显式开启、先脱敏、再导出到本地文件；默认不持久化 `raw`、完整命令、完整输出、token 或 Authorization header。

### eventType

第一阶段支持这些事件：

| eventType | 含义 | 是否通知 |
| --- | --- | --- |
| `task.started` | 会话开始 | 默认不通知，只记录 |
| `task.completed` | 任务完成或进程正常退出 | 通知 |
| `task.failed` | 进程异常退出或 hook 报错 | 通知 |
| `user.confirmation_required` | 需要用户批准、确认命令或权限 | 通知 |
| `user.input_required` | 等待用户输入下一步 | 通知 |
| `tool.blocked` | 工具调用被权限、沙箱、网络等阻塞 | 通知 |
| `heartbeat` | 会话仍存活 | 不通知，只更新状态 |

### severity

```text
info      普通完成
warning   需要用户处理
error     失败或阻塞
```

通知展示规则：

- `task.completed`: 普通通知。
- `user.confirmation_required`: 高优先级通知。
- `user.input_required`: 高优先级通知。
- `task.failed`: 错误通知。
- 同一 session 的同类事件需要去重，避免通知刷屏。

## 运行时 Hook Manager

本项目必须实现 Hook Manager，让用户无需手工编辑 Claude/Codex 配置。当前 `agent-notify-tray serve` 启动时会按配置自动执行检查和安装，也可以通过 `agent-notify-tray check-hooks` / `agent-notify-tray repair-hooks` 显式运行。Tauri 应用中的监听开关和“修复 hooks”按钮是后续 UI 入口。

### 检查内容

```text
claude --version
codex --version
codex features list
%LOCALAPPDATA%\AgentNotify\hooks\agent-notify-hook.ps1
Claude 用户级 settings.json
Codex 用户级 hooks.json 或 config.toml
```

Hook Manager 需要判断：

- Claude/Codex CLI 是否存在。
- Codex `hooks` feature 是否启用。
- 本项目 hook 脚本是否已经安装到本地数据目录。
- 用户级配置中是否已有本项目管理的 hook 条目。
- hook 条目是否指向当前版本脚本，是否包含要求的事件。

### 自动安装策略

如果 hook 缺失或过期，Hook Manager 直接安装，不要求用户手工复制配置。安装步骤必须幂等：

1. 从应用资源或仓库 `scripts/hooks/` 复制最新 `agent-notify-hook.ps1` 到 `%LOCALAPPDATA%\AgentNotify\hooks\`。
2. 生成或更新 `%LOCALAPPDATA%\AgentNotify\hooks\manifest.json`，记录版本、支持事件、安装时间和脚本 SHA-256。
3. 修改 Claude/Codex 用户级配置前，先备份到 `%LOCALAPPDATA%\AgentNotify\backups\`。
4. 写入临时文件，校验 JSON/TOML 结构后原子替换目标配置。
5. 只合并本项目命名的 hook 块，不覆盖用户已有 hook。
6. 安装后重新读取配置，确认命令路径、事件列表、版本标记和脚本 hash 正确。

Hook Manager 的写入 allowlist：

```text
%LOCALAPPDATA%\AgentNotify\**
%USERPROFILE%\.claude\settings.json
%USERPROFILE%\.codex\hooks.json
%USERPROFILE%\.codex\config.toml
```

不得写入 Claude/Codex 安装目录，不得修改项目源码目录，除非用户显式导出调试样本。

备份命名规则：

```text
%LOCALAPPDATA%\AgentNotify\backups\<tool>\<yyyyMMdd-HHmmss>-<config-name>-<sha256>.bak
```

目标要求是每个备份记录原始路径、工具名、修改前 hash、修改后 hash、应用版本和 hook 版本，备份目录设置为当前 Windows 用户私有 ACL，并按数量或时间保留，例如每个工具最近 20 份或 30 天。当前实现会按工具写入备份文件，但尚未记录完整元数据、设置 ACL 或清理保留数量。

目标安装失败处理：

- 备份失败：停止安装，状态为 `backup_failed`。
- 解析失败：不写入，状态为 `config_parse_failed`。
- 合并冲突：不覆盖，状态为 `merge_conflict`。
- 写入或校验失败：尝试恢复最近备份，状态为 `rollback_available` 或 `rollback_failed`。当前实现尚未自动恢复。
- 发现 hook 脚本 hash 不一致：重新安装脚本并标记 `hook_tampered`。

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

这些状态目标上要显示在 Tauri 托盘界面中；当前 `check-hooks` / `repair-hooks` 以 JSON report 输出，例如：

```text
Claude hook: hook_ok
Codex hook: hook_installed
Listener: enabled
```

监听关闭、后台离线或 token 无效时，hook 事件仍然直接丢弃，不写离线队列。

## Claude Code 接入方案

Claude Code 优先使用 hook 能力。目标是让 Claude 在关键生命周期事件发生时调用本地 hook 脚本，hook 脚本把事件转发给 `agent-notifyd`。

### 计划方式

```text
Claude Code
  -> hook script
  -> agent-notify emit --stdin
  -> agent-notifyd
  -> Windows Toast
```

### 需要捕获的 Claude 状态

- 任务完成。
- 需要用户批准工具调用。
- 需要用户确认下一步。
- 工具调用失败。
- 会话被中断或异常退出。

### Claude hook 适配器职责

Claude hook 脚本只做三件事：

1. 读取 Claude 传入的 hook payload。
2. 从 payload 中提取 `session_id`、`cwd`、`hook_event_name`、`tool_name`、确认摘要等字段。
3. 在内存中转换成统一事件 JSON，并写入 `agent-notify emit --stdin` 子进程的 stdin。

不要在 Claude hook 内直接发系统通知。通知逻辑必须集中在后台服务里，这样以后 Codex、普通命令、其他 agent 都能复用。

hook 脚本不得把统一事件 JSON 写到自身 stdout。Claude 会解析 hook stdout 作为控制输出；本项目第一版只做通知，默认 stdout/stderr 为空，退出码为 `0`。

### Claude 配置策略

项目内置 Claude hook 模板，Hook Manager 负责合并到 Claude 用户级配置。模板中的 hook 脚本路径必须在安装时展开为当前 Windows 用户的绝对路径，例如 `C:\Users\alice\AppData\Local\AgentNotify\hooks\agent-notify-hook.ps1`。不要把 `%LOCALAPPDATA%` 原样写进 Claude/Codex hook 命令；Claude command hooks 可能经由非 PowerShell shell 启动，PowerShell `-File` 也不会展开 `%LOCALAPPDATA%`。

配置目标类似：

```json
{
  "hooks": {
    "PermissionRequest": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "powershell -NoProfile -ExecutionPolicy Bypass -File \"C:\\Users\\alice\\AppData\\Local\\AgentNotify\\hooks\\agent-notify-hook.ps1\" --tool claude --hook-event PermissionRequest",
            "timeout": 2
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "powershell -NoProfile -ExecutionPolicy Bypass -File \"C:\\Users\\alice\\AppData\\Local\\AgentNotify\\hooks\\agent-notify-hook.ps1\" --tool claude --hook-event Stop",
            "timeout": 2
          }
        ]
      }
    ],
    "StopFailure": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "powershell -NoProfile -ExecutionPolicy Bypass -File \"C:\\Users\\alice\\AppData\\Local\\AgentNotify\\hooks\\agent-notify-hook.ps1\" --tool claude --hook-event StopFailure",
            "timeout": 2
          }
        ]
      }
    ],
    "PostToolUseFailure": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "powershell -NoProfile -ExecutionPolicy Bypass -File \"C:\\Users\\alice\\AppData\\Local\\AgentNotify\\hooks\\agent-notify-hook.ps1\" --tool claude --hook-event PostToolUseFailure",
            "timeout": 2
          }
        ]
      }
    ],
    "Notification": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "powershell -NoProfile -ExecutionPolicy Bypass -File \"C:\\Users\\alice\\AppData\\Local\\AgentNotify\\hooks\\agent-notify-hook.ps1\" --tool claude --hook-event Notification",
            "timeout": 2
          }
        ]
      }
    ],
    "SessionEnd": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "powershell -NoProfile -ExecutionPolicy Bypass -File \"C:\\Users\\alice\\AppData\\Local\\AgentNotify\\hooks\\agent-notify-hook.ps1\" --tool claude --hook-event SessionEnd",
            "timeout": 2
          }
        ]
      }
    ]
  }
}
```

Claude hooks 使用“事件 -> matcher group -> hooks[] -> handler”三层结构。`PermissionRequest`、`PostToolUseFailure` 使用 matcher；`Stop`、`StopFailure`、`Notification`、`SessionEnd` 不需要 matcher。安装器必须读取现有配置、备份、合并本项目命名块，并在安装后通过 `claude --debug` 或 `/hooks` 验证注册结果。

## Codex 接入方案

Codex 优先使用官方 lifecycle hooks，而不是输出关键词监听。本机 `codex-cli 0.130.0` 的 feature 列表中 `hooks` 为 stable/enabled，因此可以和 Claude 一样从任意终端启动后由 hook 主动上报事件。

推荐事件映射：

| Codex hook event | 统一事件 |
| --- | --- |
| `SessionStart` | `task.started` |
| `PermissionRequest` | `user.confirmation_required` |
| `Stop` | `task.completed` 或 `user.input_required` |
| `PostToolUse` 阻塞/失败 | `tool.blocked` 或 `task.failed` |

Codex hook 配置应放在用户级配置中，这样同一 Windows 用户从任意目录、任意终端启动的 Codex 都能触发通知。当前方案只支持带官方 hooks 的 Codex 最新版本，不做旧版 fallback。

项目内置 Codex hook 模板，Hook Manager 负责根据当前 Codex 版本合并安装。用户级首选路径为 `%USERPROFILE%\.codex\hooks.json`，也可以使用 `%USERPROFILE%\.codex\config.toml` 的 inline `[hooks]` 表；不要把插件打包用的 `hooks/hooks.json` 当成用户级安装路径。

第一版安装这些生命周期事件：

```text
SessionStart
PermissionRequest
Stop
PostToolUse
```

安装器必须先通过 `codex features list` 确认 hooks 有效；如果没有启用但当前 Codex 支持，则在 `%USERPROFILE%\.codex\config.toml` 写入：

```toml
[features]
hooks = true
```

安装完成后必须用一次真实 Codex hook 触发验证，确认直接运行 `codex` 会加载用户级 hooks。

Codex `hooks.json` 同样使用三层结构：

```json
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "startup|resume|clear",
        "hooks": [
          {
            "type": "command",
            "command": "powershell -NoProfile -ExecutionPolicy Bypass -File \"C:\\Users\\alice\\AppData\\Local\\AgentNotify\\hooks\\agent-notify-hook.ps1\" --tool codex --hook-event SessionStart",
            "timeout": 2,
            "statusMessage": "Agent Notify"
          }
        ]
      }
    ],
    "PermissionRequest": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "powershell -NoProfile -ExecutionPolicy Bypass -File \"C:\\Users\\alice\\AppData\\Local\\AgentNotify\\hooks\\agent-notify-hook.ps1\" --tool codex --hook-event PermissionRequest",
            "timeout": 2,
            "statusMessage": "Agent Notify"
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "powershell -NoProfile -ExecutionPolicy Bypass -File \"C:\\Users\\alice\\AppData\\Local\\AgentNotify\\hooks\\agent-notify-hook.ps1\" --tool codex --hook-event PostToolUse",
            "timeout": 2,
            "statusMessage": "Agent Notify"
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "powershell -NoProfile -ExecutionPolicy Bypass -File \"C:\\Users\\alice\\AppData\\Local\\AgentNotify\\hooks\\agent-notify-hook.ps1\" --tool codex --hook-event Stop",
            "timeout": 2
          }
        ]
      }
    ]
  }
}
```

Codex 的 `PermissionRequest` 和 `PostToolUse` matcher 过滤 tool name；`SessionStart` matcher 过滤启动来源；`Stop` 不使用 matcher。`Stop` 事件在 stdout 上只接受 JSON 或空输出，因此本项目 hook 默认必须空 stdout。

## 通用命令 wrapper

除了 Claude/Codex，还可以支持任意长任务：

```powershell
notify-run npm test
notify-run python train.py
notify-run powershell -File .\deploy.ps1
```

普通命令只需要支持：

- started
- completed
- failed

这样这个工具不会只绑定 AI agent，也可以变成通用长任务通知器。

## 通知后台

当前 `agent-notify-tray` 先实现通知后台能力，负责统一处理所有事件。文档中仍可把这个后台能力称为 `agent-notifyd`；完整 Tauri 托盘壳后续会复用同一套核心逻辑。

### 职责

- 启动一个本地 HTTP 或 named pipe 服务。
- 接收事件 JSON。
- 校验事件字段。
- 保存 session 状态。
- 做通知去重。
- 发送 Windows Toast 通知。
- 提供 Bearer 鉴权的 `/focus/{sessionId}`。
- 目标能力是处理通知点击回调并唤起对应窗口；当前尚未接入 Toast 点击。

### 本地通信方式

优先级：

1. Named Pipe：本机安全性较好，不占端口。
2. localhost HTTP：调试方便，实现简单，必须带 token。
3. Tauri sidecar IPC：适合 `agent-notify emit` 与托盘后台通信。

当前第一版使用 localhost HTTP，降低开发复杂度：

```text
POST http://127.0.0.1:17891/events
GET  http://127.0.0.1:17891/sessions
POST http://127.0.0.1:17891/focus/{sessionId}
Authorization: Bearer <AGENT_NOTIFY_TOKEN>
```

所有 localhost 接口都必须鉴权，不只是 `/events`。后台未运行、监听关闭、token 无效、HTTP 401/403、连接拒绝、超时或 5xx 时，hook 事件直接丢弃，不写离线队列。

`agent-notify emit --stdin` 的失败行为：

- 默认总耗时不超过 1.5 秒，留 0.5 秒给 hook 自身清理。
- 后台不可用、HTTP 超时、401/403、5xx、监听关闭都等价于事件丢弃。
- 默认退出码为 `0`，不把失败传播给 Claude/Codex。
- 不向 stdout/stderr 打印 payload、token、命令参数或完整错误堆栈。
- 仅允许写一条白名单日志：时间、组件、事件类型、错误码。

## Windows 气泡通知

通知需要类似微信消息一样从系统通知中心弹出。

### 推荐实现

目标选择：

- Tauri 托盘应用承载常驻 UI。
- Windows Toast / Tauri notification plugin 或原生 Windows Toast。
- 点击通知主体通过短期 activation nonce 回到后台。
- Tauri 内部 session 状态表。

当前实现：

- `agent-notify-tray` 后台直接调用 PowerShell Windows Runtime Toast。
- Toast 只展示标题、正文和详情，没有点击回调。
- session 状态表只在内存中维护，可通过 `GET /sessions` 查询。

通知内容：

```text
标题：Claude 需要确认
内容：project-name · backend · 点击返回终端
详情：Bash 请求执行命令，参数已隐藏
交互：当前仅展示；点击回调待实现
```

完整通知策略见 `NOTIFICATION_POLICY.md`。默认不显示完整命令、完整终端输出和敏感参数。

目标通知 payload：

```text
agent-notify://focus?activationId=short-lived-nonce
```

`activationId` 由后台在创建 Toast 时生成，保存在本机内存中的 activation 表里，和 `sessionId`、`eventId`、过期时间绑定。它不是 localhost Bearer token，不能复用到 HTTP API，也不能写入日志。

### 通知交互

目标第一版至少支持：

- 点击通知主体：通过 `agent-notify://focus?activationId=short-lived-nonce` 唤起对应窗口。当前未实现。
- 如果 Windows 上继续使用 Tauri notification plugin，MVP 不承诺 Toast 按钮动作。
- `忽略`、`静音此项目` 第一版放在 Tauri 托盘 session 详情里，不放在 Windows Toast 按钮里。

第二版可以支持：

- “复制确认信息”。
- “打开项目目录”。
- “静音此 session”。
- “静音此项目 1 小时”。

禁止在通知里完成 Claude/Codex 的批准动作或继续对话，除非未来另做完整安全设计。

目标 Windows Toast 激活路径：

1. Tauri 应用注册 `agent-notify://` deep link，并启用 single-instance。
2. Toast payload 只携带短期 `activationId`，不携带命令、输出、cwd 或 bearer token。
3. 已运行实例接收 deep link 后，在内存 activation 表里校验 `activationId`，再取回绑定的 `sessionId` 和 `eventId`。
4. 找不到 session、nonce 过期、nonce 已使用或校验失败时打开 Tauri session 列表，不执行外部命令。
5. 如果后续需要 Toast 按钮，必须切换到原生 Windows Toast/AUMID/activation arguments，并单独验收按钮回调。

## 窗口信息与唤起策略

### 可携带的信息

```json
{
  "pid": 12345,
  "windowTitle": "AI: claude backend",
  "hwnd": "0x00123456",
  "terminal": "WindowsTerminal",
  "cwd": "D:\\repo\\project"
}
```

### 获取窗口方式

目标第一版：

- hook payload 和环境变量尽量携带 PID、父 PID、cwd、窗口标题。
- 后台按 HWND、PID、进程树、窗口标题逐级匹配，并校验启动时间，避免 PID 复用。
- 调用 `ShowWindow` + `SetForegroundWindow`；如果 Windows 拒绝前台切换，则使用 `FlashWindowEx` 提醒并打开 session 详情。

当前实现只读取 session 中的 HWND，并在 Windows 上调用 `ShowWindow` + `SetForegroundWindow`。PID、父 PID、进程树、窗口标题、`FlashWindowEx` 和打开 session 详情页尚未实现。

具体匹配规则：

1. HWND：先调用 `IsWindow` 和 `IsWindowVisible`，再校验窗口进程是否仍属于该 session。
2. PID：枚举顶层窗口，匹配 PID、父 PID、进程树和启动时间。
3. 标题：优先精确匹配本项目 session 前缀；模糊匹配只作为最后降级，冲突时不自动聚焦。
4. Windows Terminal：能定位窗口但无法确认 tab 时，视为 best-effort 成功，同时在 Tauri UI 展示“可能不是目标 tab”。
5. 所有定位失败：打开 Tauri session 详情页。

第二版：

- hook 辅助脚本尝试记录 HWND。
- 如果是 Windows Terminal，记录父窗口 PID。
- 支持多窗口精确唤起。

### Windows Terminal 限制

如果多个 Claude/Codex 会话运行在同一个 Windows Terminal 窗口的不同 tab 中，外部程序通常只能稳定唤起整个 Terminal 窗口，不一定能精确切换到对应 tab。

要实现更准确的点击回到对应任务，有三个现实边界：

1. 独立终端窗口更容易被准确唤起。
2. Windows Terminal 多 tab 通常只能唤起整个窗口，不能保证切到具体 tab。
3. 如果唤窗失败，打开 Tauri 托盘应用里的 session 详情作为降级。

第一版支持从任意外部终端启动，因此唤窗能力标记为 best-effort。

## 进程与 session 状态

后台维护一个 session 表。

```json
{
  "sessionId": "codex-project-20260510-153012",
  "tool": "codex",
  "cwdHash": "sha256:...",
  "projectName": "project",
  "commandKind": "codex",
  "pid": 12345,
  "windowTitle": "AI: codex project",
  "status": "waiting_user",
  "lastEventType": "user.confirmation_required",
  "lastMessage": "需要批准 shell 命令，参数已隐藏",
  "startedAt": "2026-05-10T15:30:12+08:00",
  "updatedAt": "2026-05-10T15:41:00+08:00"
}
```

session 表默认不持久化完整 cwd、完整命令或完整模型输出。UI 展示用项目名、hash、类别和脱敏摘要；完整路径只在用户显式打开详情且本地可见时展示。

状态枚举：

```text
running
waiting_user
completed
failed
unknown
```

## CLI 设计

当前提供两个核心命令；可选普通任务命令后续实现：

```powershell
agent-notify-tray
agent-notify emit
agentrun  # planned
```

### agent-notify-tray

当前启动本地通知后台。开机自启、Tauri 托盘壳和应用内监听开关后续实现。

```powershell
agent-notify-tray serve
agent-notify-tray check-hooks
agent-notify-tray repair-hooks
```

### agent-notify emit

给 hook 使用。当前只支持 `--stdin`，其他显式参数形式是后续扩展。

```powershell
agent-notify emit --stdin
```

### agentrun

普通长任务 wrapper。不用于 Claude/Codex 主路径，Claude/Codex 只走官方 hook。

```powershell
agentrun --name tests -- npm test
```

## 配置文件

当前用户配置文件是 JSON，由 `agent-notify-core` 自动创建：

```json
{
  "server": {
    "host": "127.0.0.1",
    "port": 17891
  },
  "auth": {
    "token_file": "%LOCALAPPDATA%\\AgentNotify\\token"
  },
  "notifications": {
    "enabled": true,
    "listener_enabled": true,
    "dedupe_seconds": 30
  },
  "hooks": {
    "auto_check": true,
    "auto_install": true,
    "install_dir": "%LOCALAPPDATA%\\AgentNotify\\hooks"
  }
}
```

目标产品配置可扩展为：

```toml
[server]
port = 17891
auto_start = false

[auth]
token_file = "%LOCALAPPDATA%\\AgentNotify\\token"
token_rotation = "manual"
require_token_for_all_localhost_routes = true

[notifications]
enabled = true
listener_enabled = true
dedupe_seconds = 30
notify_on_start = false
notify_on_complete = true
notify_on_confirmation = true
notify_on_failure = true
drop_when_offline = true

[window]
focus_on_click = true
title_prefix = "AI"
prefer_new_window = true

[hooks]
auto_check = true
auto_install = true
backup_before_write = true
install_dir = "%LOCALAPPDATA%\\AgentNotify\\hooks"

[tools.claude]
enabled = true
adapter = "hooks"
hook_status = "hook_ok"

[tools.codex]
enabled = true
adapter = "hooks"
hook_status = "hook_ok"
```

项目级配置可选：

```toml
[project]
name = "backend"

[notifications]
mute = false
```

## 安全边界

第一版不做这些事情：

- 不自动批准 Claude/Codex 权限请求。
- 不在通知中执行命令。
- 不把会话内容上传到外部服务。
- 不读取完整终端屏幕内容。
- 不修改 Claude/Codex 的核心安装文件。

通知里可以显示简短确认信息，但需要限制长度，避免把敏感内容完整暴露在系统通知中心。

建议：

- 默认最多显示 160 个字符。
- 可配置隐藏命令参数。
- 可配置只显示“需要确认”，不显示具体内容。

日志规则：

- `hook.log` 只记录时间、组件、事件类型、错误码、耗时。
- 禁止记录 raw payload、完整命令、完整 cwd、token、Authorization header、终端输出和异常堆栈中的 payload。
- 目标要求是 `config.json`、token 文件、备份目录设置为当前 Windows 用户私有 ACL；当前代码尚未显式加固 ACL。

## 技术选型建议

### 已选方案

```text
Tauri + Rust
Tauri tray
Windows Toast / notification plugin
PowerShell hook scripts
user32.dll / Windows API window focus
```

当前代码已经使用 Rust、PowerShell hook scripts、Windows Runtime Toast 和 `user32.dll` HWND focus；Tauri tray 和 notification plugin 尚未接入。

理由：

- 符合托盘常驻、监听开关、session 面板的需求。
- 可以承载本地事件入口和通知点击回调。
- 不要求从应用内启动 Claude/Codex，仍支持任意外部终端通过 hook 上报。

### 备选

```text
.NET 8 console/worker app
```

适合纯后台通知服务，但托盘设置和会话面板需要额外 UI。

### 不建议第一版使用

```text
Electron + xterm.js
```

除非决定做完整终端管理器，否则体积和复杂度偏大。

## 分阶段计划

### 阶段 0：验证系统能力

目标：确认本机能稳定发 Toast，并能通过窗口标题唤起终端。

交付：

- Toast 测试命令。
- 窗口枚举测试命令。
- `SetForegroundWindow` 测试。

验收：

- 能弹出 Windows 通知。
- 点击测试通知能唤起指定窗口。

### 阶段 1：通知后台 MVP

目标：实现统一事件入口和通知展示。

交付：

- `agent-notify-tray` 本地后台。已实现 Axum 后台；Tauri 托盘壳待实现。
- `agent-notify emit --stdin`
- `/events` 接口
- session 状态表
- 通知去重
- 启动配置中的监听开关；运行时 UI 切换待实现

验收：

- 手动发送 JSON 事件能弹出通知。
- 通知内容包含 tool、project、eventType、message。
- 相同事件短时间内不会重复刷屏。
- 监听关闭时事件直接丢弃。

### 阶段 2：Hook Manager 和自动安装

目标：运行时自动检查 Claude/Codex hooks，缺失或过期时直接安装。

交付：

- CLI 检测：`claude --version`、`codex --version`、`codex features list`
- hook 安装目录：`%LOCALAPPDATA%\AgentNotify\hooks\`
- `agent-notify-hook.ps1` 安装副本和 `manifest.json`
- Claude 用户级配置备份、合并、校验
- Codex 用户级配置备份、合并、校验
- CLI JSON report。Tauri UI 中的 hook 健康状态待实现。

验收：

- 全新机器只安装 Claude/Codex 后，启动 `agent-notify-tray serve` 或运行 `repair-hooks` 可安装 hooks。
- 监听开启时重新检查 hooks 是后续 UI 行为。
- 删除 hook 配置后运行 `agent-notify-tray repair-hooks` 能恢复。
- 用户已有 hook 不被删除或覆盖。
- 只替换带固定 `managedBy: agent-notify` 和稳定 id 的本项目块。
- 异常结构、重复块、同命令但无标记时进入 `merge_conflict`，不自动覆盖。
- 写入失败后从最近备份回滚是目标行为；当前只保留备份，不自动恢复。
- 安装失败时显示 `install_failed`，并保留备份。

### 阶段 3：Claude hook

目标：Claude 在需要用户确认或输入时主动通知。

交付：

- `scripts/hooks/agent-notify-hook.ps1`
- `scripts/hooks/claude-hook.template.json`
- Claude hook payload 到统一事件的映射
- Claude hook 自动安装校验
- Claude `PermissionRequest`、`Stop`、`StopFailure`、`PostToolUseFailure`、`Notification`、`SessionEnd` 脱敏 fixture

验收：

- Claude 请求权限确认时发通知。
- Claude 任务停止时发通知。
- Claude `StopFailure` / `PostToolUseFailure` 能映射为失败或阻塞通知。
- 通知中包含简短确认信息。
- 点击通知回到 Claude 窗口是后续 Toast deep link + focus fallback 验收项；当前可手动调用 `/focus/{sessionId}` 且仅支持 HWND。

### 阶段 4：Codex hook

目标：Codex 在需要用户确认、停止输出、工具阻塞时主动通知。

交付：

- `scripts/hooks/codex-hook.template.json`
- Codex 用户级 hook 自动安装
- `PermissionRequest` / `Stop` / `PostToolUse` payload 到统一事件的映射
- Codex `SessionStart`、`PermissionRequest`、`PostToolUse` 成功/失败、`Stop` 脱敏 fixture

验收：

- Codex 请求权限确认时发通知。
- Codex 本轮停止时发通知。
- Codex 工具阻塞或失败时发通知。
- Codex 用户级路径经真实触发验证，不使用 `%USERPROFILE%\.codex\hooks\hooks.json` 作为用户级路径。
- 从任意终端直接运行 `codex` 时仍能触发通知。

### 阶段 5：普通长任务 wrapper

目标：支持普通长任务命令，不用于 Claude/Codex 主路径。

交付：

- `agentrun`
- sessionId 生成
- 窗口标题设置
- 进程退出通知
- 失败退出通知

验收：

- `agentrun -- npm test` 退出后发通知。
- 点击通知能回到对应窗口。

当前状态：未实现。

### 阶段 6：托盘与管理界面

目标：提升日常使用体验。

交付：

- 托盘图标
- 当前 sessions 列表
- Claude/Codex hook 健康状态
- 修复 hooks 按钮
- 静音 session/project
- 打开日志目录
- 监听开关
- 开机自启配置说明

验收：

- 托盘可查看运行中的 Claude/Codex 会话。
- 可手动聚焦任一会话窗口。
- 可临时关闭某个项目通知。
- 可看到 Claude/Codex hooks 是否已安装，必要时一键修复。

当前状态：未实现；后端命令 `check-hooks` / `repair-hooks` 已可供 UI 后续调用。

## MVP 验收标准

后端 MVP 当前已满足：

- 能启动一个本地通知后台。
- 启动时能自动检查 Claude/Codex hooks。
- 缺失 hooks 时能自动安装，并备份用户配置。
- 能通过 `agent-notify emit --stdin` 发送标准事件 JSON，emit 失败不影响 Claude/Codex。
- 能弹出 Windows 气泡通知。
- 能通过 Claude/Codex hook 捕获确认、等待输入、完成、失败、阻塞事件。
- 监听关闭、后台离线、token 无效时事件直接丢弃。
- 通知不泄露完整命令、完整输出、token、raw payload 或 secrets。
- 通知不自动批准权限请求。

完整产品 MVP 仍需满足：

- 运行时监听开关能重新检查 hooks。
- 点击通知主体能通过短期 activation nonce 唤起对应终端窗口。
- 无法定位时打开 Tauri session 详情。
- PID、父进程、进程树和窗口标题 fallback 可用。
- Tauri 托盘 UI 可查看 hook 健康状态和 session 列表。

## 主要风险

### Claude hook 事件字段变化

Claude Code 版本升级可能改变 hook payload。解决方式：

- hook adapter 默认不保留 raw payload。
- 映射失败时降级为 `user.input_required` 或 `task.completed`。
- 增加脱敏 payload fixture 样本测试。

### Codex hook payload 需要确认

Codex hook payload 可能随版本变化。解决方式：

- hook adapter 默认不保留 raw payload。
- 为 `SessionStart`、`PermissionRequest`、`Stop`、`PostToolUse` 增加脱敏 fixture 样本测试。
- 当前方案只支持最新 Codex hooks。

### 自动修改用户级配置有破坏风险

Hook Manager 会写入 Claude/Codex 用户级配置。解决方式：

- 每次写入前备份到 `%LOCALAPPDATA%\AgentNotify\backups\`。
- 只修改带本项目标识的配置块。
- 使用临时文件 + 结构校验 + 原子替换。
- 安装后重新读取配置做结构校验和 hash 校验。
- 安装失败可从最近备份回滚。
- UI 中提供 hook 状态和“修复 hooks”，但不删除用户自定义 hooks。

### Windows Terminal 多 tab 难以精确聚焦

外部程序难以稳定切到指定 tab。解决方式：

- 第一版推荐每个 agent 使用独立窗口。
- 文档中明确限制。
- Windows Terminal 多 tab 只能承诺唤起整个窗口；如果无法确认 tab，UI 要提示“可能不是目标 tab”。
- 定位失败打开 Tauri session 详情。

### Toast 点击回调注册复杂

Windows 桌面程序需要正确注册 AUMID/协议回调。解决方式：

- 阶段 0 先单独验证。
- 完整产品 MVP 使用通知主体点击 + `agent-notify://focus?activationId=...` deep link；当前未实现。
- Toast 按钮回调不进入 MVP，除非切换到原生 Windows Toast/AUMID/activation arguments。

## 后续决策点

需要在正式实现前确定：

1. 是否只支持 Windows，还是预留 macOS/Linux。
2. Tauri 托盘监听首次启动默认是否开启。
3. Claude/Codex hook payload 字段采样结果是否覆盖所有目标事件。
4. Claude/Codex 通知中允许显示多少确认摘要。
5. 任意外部终端唤窗失败时是否只打开托盘 session 详情。
6. 自动安装 hooks 是否在首次启动时执行，还是仅在用户开启监听时执行；当前推荐两者都执行，监听关闭时只显示健康状态不接收事件。

## 推荐第一版范围

第一版建议控制在：

- Windows only。
- `agent-notify-tray` 本地后台；Tauri 托盘应用后续接入。
- 运行时自动检查和安装 Claude/Codex hooks。
- PowerShell hook scripts。
- localhost HTTP 事件入口。
- Windows Toast。
- 点击通知唤起窗口是后续范围；当前只提供 Bearer 鉴权 `/focus/{sessionId}` 且仅支持 HWND。
- Claude hook 支持确认/完成通知。
- Codex hook 支持确认/完成/阻塞通知。

这个范围能较快得到可用工具，同时不会过早陷入完整终端管理器的复杂度。
