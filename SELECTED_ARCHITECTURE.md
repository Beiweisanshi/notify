# Selected Architecture

## 用户选择

```text
事件来源：Claude hook + Codex hooks
触发方式：hook -> agent-notify emit --stdin -> 后台
通知实现：Tauri 托盘应用
点击唤窗：从任意外部终端启动，点击通知时尽量唤起原窗口
Codex 边界：只支持带官方 lifecycle hooks 的 Codex 最新版本
```

当前代码实现的是该架构的后端 MVP：`agent-notify-tray` 目前是 Axum localhost 后台，负责 HTTP 事件入口、Bearer 鉴权、session 表、去重、当前用户 Start Menu AppID 注册、Windows Toast 展示、hook 检查/修复和 HWND-only `/focus/{sessionId}`。完整 Tauri 托盘 UI、Toast 点击 deep link、activation nonce、PID/标题 fallback、session 详情页和 `agentrun` 尚未实现。

## 最终链路

```text
agent-notify-tray 启动
  -> Hook Manager 检查 Claude/Codex CLI 与用户级 hook 配置
  -> 缺失时自动备份并安装本项目 hook
  -> Claude hook / Codex lifecycle hooks
  -> agent-notify-hook.ps1
  -> 标准事件 JSON 写入 agent-notify 子进程 stdin
  -> agent-notify emit --stdin
  -> agent-notify-tray 本地后台
  -> 系统气泡通知
  -> 后续 Tauri/Toast 点击回调
  -> 根据 session/window/process 信息尽量唤起原终端窗口
```

## 关键含义

用户可以从任意终端启动 Claude/Codex。通知能力不依赖内置终端，也不要求通过 wrapper 启动。

Claude 使用 Claude Code hook。Codex 使用 Codex 官方 lifecycle hooks，本机 `codex-cli 0.130.0` 的 feature 列表中 `hooks` 为 stable/enabled。Codex 的关键事件映射为：

- `PermissionRequest` -> `user.confirmation_required`
- `Stop` -> `task.completed` 或 `user.input_required`
- `PostToolUse` 非零退出或阻塞信息 -> `tool.blocked` 或 `task.failed`
- `SessionStart` -> `task.started`

点击通知唤起外部终端是目标能力。当前 Toast 没有点击回调，已实现的是 Bearer 鉴权的 `POST /focus/{sessionId}`；它只在 session 携带 HWND 时尝试聚焦窗口。传统独立窗口、PID/标题 fallback 和 Windows Terminal 多 tab 降级仍需后续实现。

运行时必须自动检查 hook 状态。当前 `agent-notify-tray serve` 启动时会按配置自动审计 Claude/Codex 的用户级配置，`check-hooks` / `repair-hooks` 命令也可手动触发；Tauri UI 中的监听开关和“修复 hooks”按钮是后续入口。如果缺失、指向旧路径或版本不匹配，Hook Manager 会安装或修复本项目提供的 hook。安装过程先备份用户配置，只合并本项目命名的配置块，不覆盖用户已有的其他 hook。

“任意终端启动”限定为同一 Windows 用户、默认 Claude/Codex 配置层、本地后台在线或 hook 可以静默丢弃事件。企业策略禁用 hooks、Claude `disableAllHooks` 或用户显式禁用 Codex hooks 时，本项目只能显示不可用状态，不能强制接管。

## 组件职责

- Hook Manager：检测 `claude`、`codex`、hook feature 状态、用户级配置文件和本项目 hook 版本；负责自动安装、修复、备份和状态展示。
- Claude hook：捕获 Claude 的完成、确认、阻塞、等待输入等事件。
- Codex hooks：通过 Codex 用户级 `%USERPROFILE%\.codex\hooks.json` 或 `%USERPROFILE%\.codex\config.toml` 捕获生命周期事件，只支持当前最新版官方 hooks。
- `agent-notify-hook.ps1`：把工具 payload、参数和环境变量转换为标准 JSON，并写入 `agent-notify emit --stdin` 子进程；不得把事件 JSON 写到 hook 自身 stdout。
- `agent-notify emit --stdin`：把标准事件送入本地后台；默认静默丢弃失败，严格模式由 `AGENT_NOTIFY_STRICT` 开启。
- `agent-notify-tray`：当前维护内存 session 列表、显示通知、接收 hook 事件、创建/更新 `Agent Notify.lnk` 以获取 Windows Toast AppID，并提供 HWND-only focus 接口。
- Tauri 托盘应用：后续提供常驻托盘壳、运行时监听开关、hook 健康状态、session 面板、通知点击回调和静音入口。
- 通知内容：遵循 `NOTIFICATION_POLICY.md`，默认只展示脱敏摘要。

## Hook 自动安装策略

Hook Manager 必须维护这些状态：

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

检查流程：

1. 执行 `claude --version`、`codex --version`、`codex features list`，确认 CLI 可用且 Codex `hooks` 为 enabled。
2. 校验本项目 hook 源文件存在，例如 `scripts/hooks/agent-notify-hook.ps1`。
3. 校验运行时安装副本存在，例如 `%LOCALAPPDATA%\AgentNotify\hooks\agent-notify-hook.ps1`。
4. 读取 Claude 用户级 settings 和 Codex 用户级 `hooks.json` / `config.toml`，判断是否包含本项目标识的 hook 命令。
5. 缺失或过期时备份原配置到 `%LOCALAPPDATA%\AgentNotify\backups\<tool>\`，再通过临时文件 + 结构校验合并本项目 hook。
6. 安装后写入 manifest，记录命令路径、版本标记、事件列表和脚本 SHA-256；完整重读校验和真实触发验证仍是后续工作。

自动安装只表示“不需要用户手工配置”。它不自动批准 Claude/Codex 权限，不修改 Claude/Codex 安装目录，不删除用户已有 hook。

Hook Manager 只允许写入 `%LOCALAPPDATA%\AgentNotify\**`、`%USERPROFILE%\.claude\settings.json`、`%USERPROFILE%\.codex\hooks.json`、`%USERPROFILE%\.codex\config.toml`。其他路径一律不写。

## 第一版承诺

后端 MVP 当前支持：

- `agent-notify-tray` 启动或显式 `check-hooks` / `repair-hooks` 时自动检查并安装 Claude/Codex hooks。
- Claude 完成通知。
- Claude 需要确认或输入时通知。
- Claude `StopFailure` / `PostToolUseFailure` 阻塞或失败通知。
- Codex `PermissionRequest` 确认通知。
- Codex `Stop` 完成/等待下一步通知。
- Codex `PostToolUse` 异常或阻塞通知。
- 监听关闭或后台离线时丢弃事件，不补发过期通知。
- localhost 所有接口都使用 Bearer token。
- `/focus/{sessionId}` 在 session 携带 HWND 时尽量唤起原终端窗口。

当前不承诺：

- 自动批准 Claude/Codex 权限请求。
- 精确控制外部 Windows Terminal 的某个 tab；能唤起窗口但不能确认 tab 时只标记 best-effort。
- 读取或恢复完整终端屏幕内容。
- 兼容不支持官方 hooks 的旧版 Codex。
- Windows Toast 点击回调或按钮动作。
- 完整 Tauri 托盘 UI、运行时监听开关和 session 详情页。

## 实施顺序

1. 已实现本地 HTTP 后台事件入口；Tauri 托盘应用壳待实现。
2. 已实现 Hook Manager：CLI 检测、用户配置备份、hook 合并安装和修复；状态展示待 UI 接入。
3. 已实现 `agent-notify emit --stdin` 到本地后台的通信。
4. 已实现标准事件模型、去重、通知展示和离线丢弃策略。
5. 已实现 HWND-only 聚焦；PID、窗口标题、进程树逐级降级待实现。
6. 已实现 Claude hook adapter 和自动安装配置的核心路径。
7. 已实现 Codex lifecycle hook adapter 和自动安装配置的核心路径。

## 主要风险

- Claude hook payload 需要先采样确认，避免事件映射错误。
- Codex hook payload 需要先采样确认，尤其是 `PermissionRequest`、`Stop`、`PostToolUse`。
- Hook Manager 修改用户级配置必须严格备份和幂等，避免破坏用户已有 hooks。
- Claude/Codex hook 配置 schema 需要通过真实触发验证，尤其是 Codex 用户级路径不能误用插件 `hooks/hooks.json`。
- 从任意外部终端唤窗无法保证精确切到 Windows Terminal 的具体 tab。
- 通知内容默认不能展示完整命令或敏感 payload。
