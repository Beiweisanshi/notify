# Notification Policy

## 目标

通知只负责提醒用户“哪个会话需要处理”，不在通知中暴露完整命令、完整输出或敏感参数，也不允许直接批准 Claude/Codex 的权限请求。

当前实现中，通知由 `agent-notify-tray` 后台根据 `NotificationView` 生成，并通过 PowerShell Windows Runtime Toast 展示。后台启动时会创建或更新当前用户的 `智能任务通知.lnk`，写入自定义图标资源，发送时通过 `Get-StartApps` 获取 AppID。内置 hook 生成的 Toast 默认使用中文标题、正文和截断到 160 字符的详情；点击回调、忽略、静音、session 详情页和 Tauri 托盘 UI 尚未实现。

## 通知模板

### 需要确认

```text
标题：Claude 需要确认
正文：project-name · backend · 点击返回终端
详情：Bash 请求执行命令，参数已隐藏
```

```text
标题：Codex 需要确认
正文：project-name · 当前会话 · 点击返回终端
详情：请求执行 shell 命令，参数已隐藏
```

### 等待输入

```text
标题：Claude 等待输入
正文：project-name · 当前会话 · 需要你的下一步
详情：默认不显示模型完整问题
```

### 任务完成

```text
标题：Codex 任务完成
正文：project-name · 当前会话 · 点击查看结果
详情：退出码 0
```

### 执行失败或阻塞

```text
标题：Claude 执行受阻
正文：project-name · 当前会话 · 点击返回终端
详情：权限、沙箱或工具调用失败
```

## 显示规则

- 标题必须包含工具名和状态，例如 `Claude 需要确认`。
- 正文只显示项目名、会话名或目录名，以及动作提示。
- 详情最多 160 个字符。
- 默认隐藏完整命令参数、完整终端输出、token、路径中的敏感片段。
- 当前 Windows MVP 只承诺展示 Toast 文本；点击通知主体打开窗口尚未实现。后续点击 deep link 必须使用短期 activation nonce，不能把 Bearer token 放进 URI。
- `忽略`、`静音此项目` 放在后续 Tauri 托盘 UI 中。
- 不允许在通知里批准命令或继续对话。
- 默认不显示具体命令名。是否显示命令名必须作为用户显式隐私选项。

## 事件优先级

```text
user.confirmation_required  高
user.input_required         高
tool.blocked                高
task.failed                 高
task.completed              普通
task.started                不弹
heartbeat                   不弹
```

## 去重策略

同一个 `sessionId`、同一个 `eventType`、同一个 `eventId` 或摘要 hash，在 30 秒内只弹一次。

状态变化必须绕过去重并更新通知，例如 `confirmation_required -> task.completed`、`confirmation_required -> task.failed`、`input_required -> task.failed`。连续两次不同 `eventId` 的权限请求即使摘要相同，也必须更新 session 状态，是否再次弹通知由优先级和静音规则决定。

## 监听关闭和离线

用户关闭监听、`agent-notify-tray` 后台未运行、token 无效或本地通信失败时，hook 事件直接丢弃。当前监听开关只在启动时从配置读取，尚无运行时 UI 切换。

不写离线队列，不补发过期通知。

## 日志与存储

- `hook.log` 只记录时间、组件、事件类型、错误码和耗时。
- 不记录 raw payload、完整 cwd、完整命令、token、Authorization header、终端输出或异常堆栈中的 payload。
- 目标要求是备份文件、token 文件和配置文件设置为当前 Windows 用户私有 ACL；当前实现会创建这些文件，但尚未显式加固 ACL。
- raw payload 默认不持久化；调试导出必须显式开启、脱敏并由用户确认。
