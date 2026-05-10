# Claude/Codex 通用任务通知工具计划

## 目标

做一个本地后台通知工具，用来监控多个 Claude Code 和 Codex CLI 终端会话。当任务完成、进入等待用户确认、需要用户继续输入、执行失败或权限被阻塞时，自动发出 Windows 系统气泡通知。用户点击通知后，应尽量唤起对应终端窗口。

第一阶段不做完整终端替代品，只做“通用 hook + 本地通知后台 + 窗口唤起”的轻量系统。

## 使用场景

- 同时打开多个终端运行 Claude/Codex。
- 用户离开当前终端去做别的事情。
- 某个会话完成任务、失败、等待批准命令、等待继续输入时，系统通知提醒用户。
- 用户点击通知后，切回对应的终端窗口。

## 总体架构

```text
Claude Code hook / Codex wrapper / 通用 CLI wrapper
        |
        | 统一 JSON 事件
        v
本地通知后台 agent-notifyd
        |
        | Windows Toast 通知
        v
系统通知中心 / 气泡通知
        |
        | 点击通知
        v
agent-notifyd 根据 sessionId 唤起窗口
```

系统拆成四层：

1. 事件来源层：Claude hook、Codex wrapper、通用命令 wrapper。
2. 事件协议层：把不同工具的事件统一成同一种 JSON。
3. 通知后台层：本地常驻进程，负责接收事件、去重、展示通知、保存 session 状态。
4. 窗口控制层：根据 PID、窗口标题、HWND 等信息唤起对应窗口。

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
  },
  "action": {
    "kind": "focus-window",
    "target": "sessionId"
  },
  "raw": {}
}
```

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
2. 从 payload 和环境变量中提取 `sessionId`、`cwd`、`eventType`、确认信息。
3. 转换成统一事件 JSON，并发送给本地后台。

不要在 Claude hook 内直接发系统通知。通知逻辑必须集中在后台服务里，这样以后 Codex、普通命令、其他 agent 都能复用。

### Claude 配置策略

后续落地时需要确认 Claude Code 当前版本的 hook 配置格式，并生成一份最小配置。配置目标类似：

```json
{
  "hooks": {
    "Stop": [
      {
        "type": "command",
        "command": "agent-notify emit --tool claude --event task.completed"
      }
    ],
    "Notification": [
      {
        "type": "command",
        "command": "agent-notify emit --tool claude --event user.confirmation_required"
      }
    ]
  }
}
```

上面只是目标形态，具体字段以本机 Claude Code 支持的 hook 配置为准。

## Codex 接入方案

Codex 第一阶段不要强依赖内部 experimental 能力。先通过统一启动器接入。

### 第一阶段：wrapper 方式

以后使用：

```powershell
agentrun codex -C D:\repo
agentrun claude
```

`agentrun` 的职责：

1. 生成 `sessionId`。
2. 设置当前终端窗口标题。
3. 记录 cwd、PID、启动命令、工具类型。
4. 启动真实的 `codex` 或 `claude`。
5. 等待子进程退出。
6. 根据退出码发出 `task.completed` 或 `task.failed`。

这个方式能稳定捕获“进程结束”，但无法完全准确判断 Codex 交互中途是否正在等待用户确认。

### 第二阶段：输出监听

在 wrapper 中读取输出流，识别 Codex 常见等待状态，例如：

- asking for approval
- approve command
- waiting for input
- continue
- permission denied
- sandbox blocked

输出监听只能作为增强能力，不能作为唯一依据，因为 TUI、ANSI 控制符和版本变化会影响稳定性。

### 第三阶段：Codex remote-control / app-server

Codex CLI 有 remote-control/app-server 相关能力，但目前属于实验性质。后续可以调研是否能获得结构化事件，例如：

- agent status changed
- approval requested
- task finished
- session idle

如果可以稳定获得事件，再替换第二阶段的输出监听。

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

## 通知后台 agent-notifyd

后台服务负责统一处理所有事件。

### 职责

- 启动一个本地 HTTP 或 named pipe 服务。
- 接收事件 JSON。
- 校验事件字段。
- 保存 session 状态。
- 做通知去重。
- 发送 Windows Toast 通知。
- 处理通知点击回调。
- 唤起对应窗口。

### 本地通信方式

优先级：

1. Named Pipe：本机安全性较好，不占端口。
2. localhost HTTP：调试方便，实现简单。
3. 文件队列：最简单但实时性和并发较差。

第一版建议用 localhost HTTP，降低开发复杂度：

```text
POST http://127.0.0.1:17891/events
GET  http://127.0.0.1:17891/sessions
POST http://127.0.0.1:17891/focus/{sessionId}
```

后续稳定后可以切到 named pipe。

## Windows 气泡通知

通知需要类似微信消息一样从系统通知中心弹出。

### 推荐实现

Windows 平台优先用：

- .NET 8
- Windows App SDK 或 CommunityToolkit.WinUI.Notifications
- Desktop app AUMID 注册
- Toast activation 回调

通知内容：

```text
标题：Claude 需要确认
内容：backend 会话请求批准命令：npm test
按钮：打开窗口
```

通知 payload：

```text
agent-notify://focus?sessionId=xxx&eventId=yyy
```

### 通知交互

第一版至少支持：

- 点击通知主体：唤起对应窗口。
- “打开窗口”按钮：唤起对应窗口。
- “忽略”按钮：关闭通知。

第二版可以支持：

- “复制确认信息”。
- “打开项目目录”。
- “静音此 session”。
- “静音此项目 1 小时”。

不建议第一版直接在通知里完成 Claude/Codex 的批准动作，因为这涉及权限安全和 TUI 状态同步，风险更高。

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

第一版：

- wrapper 启动时设置唯一窗口标题。
- 后台通过枚举窗口标题匹配 session。
- 调用 `ShowWindow` + `SetForegroundWindow`。

第二版：

- 启动时记录 HWND。
- 如果是 Windows Terminal，记录父窗口 PID。
- 支持多窗口精确唤起。

### Windows Terminal 限制

如果多个 Claude/Codex 会话运行在同一个 Windows Terminal 窗口的不同 tab 中，外部程序通常只能稳定唤起整个 Terminal 窗口，不一定能精确切换到对应 tab。

要实现更准确的点击回到对应任务，有三个选择：

1. 每个 agent 会话使用独立 Terminal 窗口。
2. 使用工具自带的远程控制/会话恢复能力。
3. 做一个内置终端管理器，用 ConPTY 托管所有会话。

第一版建议选择第 1 种。

## 进程与 session 状态

后台维护一个 session 表。

```json
{
  "sessionId": "codex-project-20260510-153012",
  "tool": "codex",
  "cwd": "D:\\repo\\project",
  "command": "codex -C D:\\repo\\project",
  "pid": 12345,
  "windowTitle": "AI: codex project",
  "status": "waiting_user",
  "lastEventType": "user.confirmation_required",
  "lastMessage": "需要批准命令 npm test",
  "startedAt": "2026-05-10T15:30:12+08:00",
  "updatedAt": "2026-05-10T15:41:00+08:00"
}
```

状态枚举：

```text
running
waiting_user
completed
failed
unknown
```

## CLI 设计

计划提供三个命令：

```powershell
agent-notifyd
agent-notify emit
agentrun
```

### agent-notifyd

启动后台通知服务。

```powershell
agent-notifyd
agent-notifyd --port 17891
agent-notifyd --startup
```

### agent-notify emit

给 hook 和 wrapper 使用。

```powershell
agent-notify emit --tool claude --event task.completed --session backend
agent-notify emit --stdin
```

### agentrun

统一启动 Claude/Codex/普通命令。

```powershell
agentrun claude -n backend
agentrun codex -C D:\repo
agentrun --name tests -- npm test
```

## 配置文件

用户配置：

```toml
[server]
port = 17891
auto_start = true

[notifications]
enabled = true
dedupe_seconds = 30
notify_on_start = false
notify_on_complete = true
notify_on_confirmation = true
notify_on_failure = true

[window]
focus_on_click = true
title_prefix = "AI"
prefer_new_window = true

[tools.claude]
enabled = true
adapter = "hooks"

[tools.codex]
enabled = true
adapter = "wrapper"
output_detection = true
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

- 默认最多显示 200 个字符。
- 可配置隐藏命令参数。
- 可配置只显示“需要确认”，不显示具体内容。

## 技术选型建议

### 首选

```text
.NET 8 console/worker app
Windows Toast API
PowerShell wrapper scripts
user32.dll P/Invoke
```

理由：

- Windows 通知和窗口唤起更直接。
- 单机工具分发简单。
- 后续可加托盘图标。

### 备选

```text
Tauri + Rust
```

适合后续要做设置界面、session 面板、托盘菜单。

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

- `agent-notifyd`
- `agent-notify emit --stdin`
- `/events` 接口
- session 状态表
- 通知去重

验收：

- 手动发送 JSON 事件能弹出通知。
- 通知内容包含 tool、project、eventType、message。
- 相同事件短时间内不会重复刷屏。

### 阶段 2：通用 wrapper

目标：支持通过 wrapper 运行 Claude/Codex/普通命令，并在退出时通知。

交付：

- `agentrun`
- sessionId 生成
- 窗口标题设置
- 进程退出通知
- 失败退出通知

验收：

- `agentrun codex ...` 退出后发通知。
- `agentrun claude ...` 退出后发通知。
- `agentrun -- npm test` 退出后发通知。
- 点击通知能回到对应窗口。

### 阶段 3：Claude hook

目标：Claude 在需要用户确认或输入时主动通知。

交付：

- Claude hook adapter
- Claude hook 配置生成命令
- hook payload 到统一事件的映射

验收：

- Claude 请求权限确认时发通知。
- Claude 任务停止时发通知。
- 通知中包含简短确认信息。
- 点击通知回到 Claude 窗口。

### 阶段 4：Codex 增强

目标：在 Codex 交互过程中尽量识别等待用户状态。

交付：

- Codex 输出监听规则
- ANSI 清理
- 可配置关键词
- experimental remote-control 调研结果

验收：

- Codex 退出通知稳定。
- 常见等待用户确认场景能触发通知。
- 误报可通过配置关闭。

### 阶段 5：托盘与管理界面

目标：提升日常使用体验。

交付：

- 托盘图标
- 当前 sessions 列表
- 静音 session/project
- 打开日志目录
- 开机自启

验收：

- 托盘可查看运行中的 Claude/Codex 会话。
- 可手动聚焦任一会话窗口。
- 可临时关闭某个项目通知。

## MVP 验收标准

MVP 完成时应满足：

- 能启动一个本地通知后台。
- 能通过命令发送标准事件 JSON。
- 能弹出 Windows 气泡通知。
- 能通过 `agentrun codex` 和 `agentrun claude` 捕获进程完成。
- 点击通知能唤起对应终端窗口，至少能唤起包含该 session 的 Windows Terminal 窗口。
- Claude 能在“需要用户确认”时触发通知。

## 主要风险

### Claude hook 事件字段变化

Claude Code 版本升级可能改变 hook payload。解决方式：

- hook adapter 保留 raw payload。
- 映射失败时降级为 `user.input_required` 或 `task.completed`。
- 增加 payload 样本测试。

### Codex 等待状态难以准确识别

Codex 如果没有稳定 hook，输出监听会有误报/漏报。解决方式：

- 第一版只承诺退出通知。
- 等待确认通知标记为 best-effort。
- 后续接 remote-control/app-server。

### Windows Terminal 多 tab 难以精确聚焦

外部程序难以稳定切到指定 tab。解决方式：

- 第一版推荐每个 agent 使用独立窗口。
- 文档中明确限制。
- 后续考虑内置终端管理器。

### Toast 点击回调注册复杂

Windows 桌面程序需要正确注册 AUMID/协议回调。解决方式：

- 阶段 0 先单独验证。
- 优先选成熟 .NET Toast 库。

## 后续决策点

需要在正式实现前确定：

1. 是否只支持 Windows，还是预留 macOS/Linux。
2. 第一版是否要求托盘图标。
3. Codex 第一版是否只做退出通知。
4. Claude 通知中是否允许显示完整确认命令。
5. 是否强制 `agentrun` 为每个会话打开独立 Windows Terminal 窗口。

## 推荐第一版范围

第一版建议控制在：

- Windows only。
- .NET 8 后台服务。
- PowerShell wrapper。
- localhost HTTP 事件入口。
- Windows Toast。
- 点击通知唤起窗口。
- Claude hook 支持确认/完成通知。
- Codex 支持完成/失败通知，等待确认作为实验增强。

这个范围能较快得到可用工具，同时不会过早陷入完整终端管理器的复杂度。
