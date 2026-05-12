$ErrorActionPreference = 'SilentlyContinue'
$ProgressPreference = 'SilentlyContinue'
$VerbosePreference = 'SilentlyContinue'
$DebugPreference = 'SilentlyContinue'
$InformationPreference = 'SilentlyContinue'
$WarningPreference = 'SilentlyContinue'

function Get-AgentNotifyArgValue {
    param(
        [object[]]$Arguments,
        [string[]]$Names
    )

    if ($null -eq $Arguments) {
        return $null
    }

    for ($i = 0; $i -lt $Arguments.Count; $i++) {
        $current = [string]$Arguments[$i]
        foreach ($name in $Names) {
            if ($current -ieq $name) {
                if (($i + 1) -lt $Arguments.Count) {
                    return [string]$Arguments[$i + 1]
                }
            }

            $prefix = "$name="
            if ($current.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
                return $current.Substring($prefix.Length)
            }
        }
    }

    return $null
}

function Get-AgentNotifyEnvValue {
    param([string]$Name)

    $value = [Environment]::GetEnvironmentVariable($Name, 'Process')
    if ([string]::IsNullOrWhiteSpace($value)) {
        return $null
    }

    return $value.Trim()
}

function ConvertTo-AgentNotifyScalar {
    param([object]$Value)

    if ($null -eq $Value) {
        return $null
    }

    if ($Value -is [string]) {
        $text = $Value.Trim()
        if ($text.Length -eq 0) {
            return $null
        }

        return $text
    }

    if ($Value -is [ValueType]) {
        return [string]$Value
    }

    return $null
}

function Get-AgentNotifyProperty {
    param(
        [object]$Object,
        [string]$Name
    )

    if ($null -eq $Object) {
        return $null
    }

    if ($Object -is [System.Collections.IDictionary]) {
        foreach ($key in $Object.Keys) {
            if ([string]::Equals([string]$key, $Name, [System.StringComparison]::OrdinalIgnoreCase)) {
                return $Object[$key]
            }
        }

        return $null
    }

    foreach ($property in $Object.PSObject.Properties) {
        if ([string]::Equals($property.Name, $Name, [System.StringComparison]::OrdinalIgnoreCase)) {
            return $property.Value
        }
    }

    return $null
}

function Get-AgentNotifyPathValue {
    param(
        [object]$Object,
        [string]$Path
    )

    $current = $Object
    foreach ($part in ($Path -split '\.')) {
        if ($null -eq $current) {
            return $null
        }

        if (($current -is [System.Array]) -and $current.Count -gt 0) {
            $current = $current[0]
        }

        $current = Get-AgentNotifyProperty -Object $current -Name $part
    }

    return $current
}

function Get-AgentNotifyFirstScalar {
    param(
        [object]$Payload,
        [string[]]$Paths
    )

    foreach ($path in $Paths) {
        $value = ConvertTo-AgentNotifyScalar (Get-AgentNotifyPathValue -Object $Payload -Path $path)
        if ($null -ne $value) {
            return $value
        }
    }

    return $null
}

function Get-AgentNotifyInt {
    param([object]$Value)

    $text = ConvertTo-AgentNotifyScalar $Value
    if ($null -eq $text) {
        return $null
    }

    $number = [int64]0
    if ([int64]::TryParse($text, [ref]$number)) {
        return $number
    }
    if ($text.StartsWith('0x', [System.StringComparison]::OrdinalIgnoreCase)) {
        $hex = $text.Substring(2)
        if ([int64]::TryParse($hex, [System.Globalization.NumberStyles]::HexNumber, [System.Globalization.CultureInfo]::InvariantCulture, [ref]$number)) {
            return $number
        }
    }

    return $null
}

function Get-AgentNotifyHash {
    param([string]$Text)

    if ($null -eq $Text) {
        $Text = ''
    }

    $bytes = [System.Text.Encoding]::UTF8.GetBytes($Text)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $hash = $sha.ComputeHash($bytes)
        return (($hash | ForEach-Object { $_.ToString('x2') }) -join '')
    }
    finally {
        $sha.Dispose()
    }
}

function Limit-AgentNotifyText {
    param(
        [string]$Text,
        [int]$MaxLength = 160
    )

    if ([string]::IsNullOrWhiteSpace($Text)) {
        return $null
    }

    $normalized = (($Text -replace '\s+', ' ').Trim())
    if ($normalized.Length -le $MaxLength) {
        return $normalized
    }

    return $normalized.Substring(0, $MaxLength)
}

function Test-AgentNotifyPathLike {
    param([string]$Text)

    if ([string]::IsNullOrWhiteSpace($Text)) {
        return $false
    }

    return ($Text -match '^[A-Za-z]:[\\/]' -or $Text -match '[\\/]')
}

function Get-AgentNotifyProjectName {
    param(
        [string]$Cwd,
        [string]$ProjectValue
    )

    if (-not [string]::IsNullOrWhiteSpace($ProjectValue)) {
        if (-not (Test-AgentNotifyPathLike $ProjectValue)) {
            return (Limit-AgentNotifyText -Text $ProjectValue -MaxLength 80)
        }
    }

    if (-not [string]::IsNullOrWhiteSpace($Cwd)) {
        $trimmed = $Cwd.TrimEnd('\', '/')
        $name = [System.IO.Path]::GetFileName($trimmed)
        if (-not [string]::IsNullOrWhiteSpace($name)) {
            return (Limit-AgentNotifyText -Text $name -MaxLength 80)
        }
    }

    return 'project'
}

function Get-AgentNotifyParentPid {
    param([int]$ProcessId)

    if ($ProcessId -le 0) {
        return $null
    }

    $process = Get-CimInstance -ClassName Win32_Process -Filter "ProcessId=$ProcessId" -ErrorAction SilentlyContinue
    if ($null -ne $process) {
        return [int]$process.ParentProcessId
    }

    return $null
}

function Get-AgentNotifyProcessInfo {
    param([int]$ProcessId)

    if ($ProcessId -le 0) {
        return $null
    }

    return Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
}

function Walk-AgentNotifyAncestors {
    param(
        [int]$StartPid,
        [scriptblock]$Predicate
    )

    if ($StartPid -le 0 -or $null -eq $Predicate) {
        return $null
    }

    $seen = @{}
    $currentPid = $StartPid
    for ($i = 0; $i -lt 12; $i++) {
        if ($null -eq $currentPid -or $currentPid -le 0 -or $seen.ContainsKey($currentPid)) {
            break
        }
        $seen[$currentPid] = $true

        $outcome = & $Predicate $currentPid
        if ($null -ne $outcome) {
            if ($outcome -is [hashtable] -and $outcome['Stop']) {
                return $null
            }
            return $outcome
        }

        $parentPid = Get-AgentNotifyParentPid -ProcessId $currentPid
        if ($null -eq $parentPid -or $parentPid -eq $currentPid) {
            break
        }
        $currentPid = $parentPid
    }
    return $null
}

function Get-AgentNotifyAncestorWindowInfo {
    param([int]$ProcessId)

    return Walk-AgentNotifyAncestors -StartPid $ProcessId -Predicate {
        param([int]$AncestorPid)

        $processInfo = Get-AgentNotifyProcessInfo -ProcessId $AncestorPid
        if ($null -eq $processInfo) {
            return $null
        }
        $processName = $processInfo.ProcessName
        $title = $processInfo.MainWindowTitle
        $hwnd = [int64]$processInfo.MainWindowHandle
        if ($hwnd -ne 0 -and ($processName -notmatch '^explorer$' -or -not [string]::IsNullOrWhiteSpace($title))) {
            return [ordered]@{
                pid = $AncestorPid
                processName = $processName
                title = $title
                hwnd = $hwnd
            }
        }
        if ($processName -match '^explorer$') {
            return @{ Stop = $true }
        }
        return $null
    }
}

$script:AgentNotifyConsoleWindowTypeReady = $false

$script:AgentNotifyShellProcessNamePattern = '^(powershell|pwsh|cmd|conhost|openconsole|wt|windowsterminal)$'

function Initialize-AgentNotifyConsoleWindowType {
    if ($script:AgentNotifyConsoleWindowTypeReady) {
        return $true
    }
    try {
        if (-not ('AgentNotify.ConsoleWindow' -as [type])) {
            Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Collections.Generic;
namespace AgentNotify {
    public static class ConsoleWindow {
        public delegate bool EnumDelegate(IntPtr hWnd, IntPtr lParam);
        [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow();
        [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
        [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hWnd, StringBuilder lpString, int nMaxCount);
        [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder lpString, int nMaxCount);
        [DllImport("user32.dll")] public static extern bool EnumWindows(EnumDelegate lpEnumFunc, IntPtr lParam);
        [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr hWnd, uint flags);
        public static List<long> FindWindowsByPid(uint pid) {
            var found = new List<long>();
            EnumWindows((h, l) => {
                uint owner;
                GetWindowThreadProcessId(h, out owner);
                if (owner == pid) {
                    found.Add(h.ToInt64());
                }
                return true;
            }, IntPtr.Zero);
            return found;
        }
    }
}
'@ -ErrorAction Stop
        }
        $script:AgentNotifyConsoleWindowTypeReady = $true
        return $true
    }
    catch {
        return $false
    }
}

function Get-AgentNotifyConsoleWindowInfo {
    if (-not (Initialize-AgentNotifyConsoleWindowType)) {
        return $null
    }
    try {
        $hwnd = [AgentNotify.ConsoleWindow]::GetConsoleWindow()
        if ($null -eq $hwnd -or [int64]$hwnd -eq 0) {
            return $null
        }
        $ownerPid = 0
        [void][AgentNotify.ConsoleWindow]::GetWindowThreadProcessId($hwnd, [ref]$ownerPid)
        $title = New-Object System.Text.StringBuilder 512
        [void][AgentNotify.ConsoleWindow]::GetWindowTextW($hwnd, $title, 512)
        $className = New-Object System.Text.StringBuilder 256
        [void][AgentNotify.ConsoleWindow]::GetClassNameW($hwnd, $className, 256)
        return [ordered]@{
            hwnd = [int64]$hwnd
            pid = [int]$ownerPid
            title = $title.ToString()
            className = $className.ToString()
        }
    }
    catch {
        return $null
    }
}

function Find-AgentNotifyPseudoConsoleByAncestor {
    param([int]$ProcessId)

    if (-not (Initialize-AgentNotifyConsoleWindowType) -or $ProcessId -le 0) {
        return $null
    }

    return Walk-AgentNotifyAncestors -StartPid $ProcessId -Predicate {
        param([int]$AncestorPid)

        $processInfo = Get-AgentNotifyProcessInfo -ProcessId $AncestorPid
        if ($null -eq $processInfo -or $processInfo.ProcessName -notmatch $script:AgentNotifyShellProcessNamePattern) {
            return $null
        }
        try {
            $hwnds = [AgentNotify.ConsoleWindow]::FindWindowsByPid([uint32]$AncestorPid)
            foreach ($h in $hwnds) {
                $cls = New-Object System.Text.StringBuilder 256
                [void][AgentNotify.ConsoleWindow]::GetClassNameW([IntPtr]$h, $cls, 256)
                if ($cls.ToString() -eq 'PseudoConsoleWindow') {
                    $title = New-Object System.Text.StringBuilder 512
                    [void][AgentNotify.ConsoleWindow]::GetWindowTextW([IntPtr]$h, $title, 512)
                    return [ordered]@{
                        hwnd = [int64]$h
                        pid = [int]$AncestorPid
                        title = $title.ToString()
                        className = 'PseudoConsoleWindow'
                    }
                }
            }
        }
        catch {
        }
        return $null
    }
}

function Resolve-AgentNotifyEventType {
    param(
        [string]$Tool,
        [string]$HookEvent,
        [string]$ExplicitEvent,
        [object]$Payload
    )

    $supported = @(
        'task.started',
        'task.completed',
        'task.failed',
        'user.confirmation_required',
        'user.input_required',
        'tool.blocked',
        'heartbeat'
    )

    if ($supported -contains $ExplicitEvent) {
        return $ExplicitEvent
    }

    $status = Get-AgentNotifyFirstScalar -Payload $Payload -Paths @(
        'status',
        'result.status',
        'outcome',
        'state',
        'reason',
        'error.code',
        'error.reason'
    )
    $exitCode = Get-AgentNotifyInt (Get-AgentNotifyFirstScalar -Payload $Payload -Paths @(
        'exitCode',
        'exit_code',
        'result.exitCode',
        'process.exitCode'
    ))

    $statusText = ''
    if ($null -ne $status) {
        $statusText = $status.ToLowerInvariant()
    }

    switch -Regex ($HookEvent) {
        '^(SessionStart)$' { return 'task.started' }
        '^(PermissionRequest)$' { return 'user.confirmation_required' }
        '^(Notification)$' {
            if ($statusText -match 'permission|confirm|approval|approve') {
                return 'user.confirmation_required'
            }

            return 'user.input_required'
        }
        '^(Stop)$' {
            if ($statusText -match 'input|waiting|continue|prompt') {
                return 'user.input_required'
            }

            return 'task.completed'
        }
        '^(StopFailure)$' { return 'task.failed' }
        '^(PostToolUseFailure)$' {
            if ($statusText -match 'permission|sandbox|blocked|denied|rejected') {
                return 'tool.blocked'
            }

            return 'task.failed'
        }
        '^(PostToolUse)$' {
            if ($statusText -match 'permission|sandbox|blocked|denied|rejected') {
                return 'tool.blocked'
            }

            if (($null -ne $exitCode -and $exitCode -ne 0) -or $statusText -match 'fail|error|timeout') {
                return 'task.failed'
            }

            return 'heartbeat'
        }
        '^(SessionEnd)$' {
            if (($null -ne $exitCode -and $exitCode -ne 0) -or $statusText -match 'fail|error|cancel|abort|interrupt') {
                return 'task.failed'
            }

            return 'task.completed'
        }
        '^(Heartbeat)$' { return 'heartbeat' }
    }

    if ($ExplicitEvent -and ($ExplicitEvent -notin $supported)) {
        return Resolve-AgentNotifyEventType -Tool $Tool -HookEvent $ExplicitEvent -ExplicitEvent $null -Payload $Payload
    }

    return 'user.input_required'
}

function Get-AgentNotifySeverity {
    param([string]$EventType)

    switch ($EventType) {
        'task.failed' { return 'error' }
        'tool.blocked' { return 'error' }
        'user.confirmation_required' { return 'warning' }
        'user.input_required' { return 'warning' }
        default { return 'info' }
    }
}

function Get-AgentNotifyStateLabel {
    param([string]$EventType)

    switch ($EventType) {
        'task.started' { return '任务开始' }
        'task.completed' { return '任务完成' }
        'task.failed' { return '任务失败' }
        'user.confirmation_required' { return '需要确认' }
        'user.input_required' { return '等待输入' }
        'tool.blocked' { return '工具受阻' }
        default { return '状态更新' }
    }
}

function Get-AgentNotifyDetail {
    param(
        [string]$EventType,
        [object]$Payload
    )

    $exitCode = Get-AgentNotifyInt (Get-AgentNotifyFirstScalar -Payload $Payload -Paths @(
        'exitCode',
        'exit_code',
        'result.exitCode',
        'process.exitCode'
    ))

    switch ($EventType) {
        'task.started' { return '会话已开始。' }
        'task.completed' {
            if ($null -ne $exitCode) {
                return "退出码 $exitCode。"
            }

            return '任务已完成。'
        }
        'task.failed' {
            if ($null -ne $exitCode) {
                return "任务失败，退出码 $exitCode。"
            }

            return '任务失败；详情已隐藏。'
        }
        'tool.blocked' { return '权限、沙箱或工具调用被阻止。' }
        'user.confirmation_required' { return '工具请求需要确认；参数已隐藏。' }
        'user.input_required' { return '等待你的下一步输入；详情已隐藏。' }
        default { return '会话状态已更新。' }
    }
}

function Get-AgentNotifyToolLabel {
    param([string]$Tool)

    switch ($Tool) {
        'claude' { return 'Claude' }
        'codex' { return 'Codex' }
        default {
            if ([string]::IsNullOrWhiteSpace($Tool) -or $Tool -eq 'unknown') {
                return 'Task'
            }

            return (Get-Culture).TextInfo.ToTitleCase($Tool)
        }
    }
}

function Read-AgentNotifyStdin {
    try {
        if ([Console]::IsInputRedirected) {
            return [Console]::In.ReadToEnd()
        }
    }
    catch {
    }

    return ''
}

function Write-AgentNotifyLogLine {
    param([string]$Line)

    if ((Get-AgentNotifyEnvValue 'AGENT_NOTIFY_HOOK_LOG') -eq '0') {
        return
    }
    $localAppData = Get-AgentNotifyEnvValue 'LOCALAPPDATA'
    if ([string]::IsNullOrWhiteSpace($localAppData)) {
        return
    }
    try {
        $logDir = Join-Path $localAppData 'AgentNotify\logs'
        New-Item -ItemType Directory -Path $logDir -Force | Out-Null
        $logPath = Join-Path $logDir 'hook.log'
        Add-Content -Path $logPath -Value $Line -Encoding UTF8
    }
    catch {
    }
}

function Write-AgentNotifyDebug {
    param([string]$Message)

    $timestamp = (Get-Date).ToString('o')
    $safe = Limit-AgentNotifyText -Text $Message -MaxLength 480
    Write-AgentNotifyLogLine -Line "$timestamp component=hook-debug $safe"
}

function Write-AgentNotifyHookLog {
    param(
        [string]$Code,
        [string]$EventType,
        [long]$ElapsedMs
    )

    $timestamp = (Get-Date).ToString('o')
    $safeCode = Limit-AgentNotifyText -Text $Code -MaxLength 48
    $safeEventType = Limit-AgentNotifyText -Text $EventType -MaxLength 64
    Write-AgentNotifyLogLine -Line "$timestamp component=hook eventType=$safeEventType code=$safeCode elapsedMs=$ElapsedMs"
}

function Resolve-AgentNotifyEmitter {
    $override = Get-AgentNotifyEnvValue 'AGENT_NOTIFY_BIN'
    if (-not [string]::IsNullOrWhiteSpace($override) -and (Test-Path -LiteralPath $override -PathType Leaf)) {
        return $override
    }

    try {
        if (-not [string]::IsNullOrWhiteSpace($PSScriptRoot)) {
            $runtimeDir = Split-Path -Parent $PSScriptRoot
            $candidates = @(
                (Join-Path $runtimeDir 'bin\agent-notify.exe'),
                (Join-Path $runtimeDir 'agent-notify.exe'),
                (Join-Path $PSScriptRoot 'agent-notify.exe')
            )

            foreach ($candidate in $candidates) {
                if (-not [string]::IsNullOrWhiteSpace($candidate) -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
                    return $candidate
                }
            }
        }
    }
    catch {
    }

    return 'agent-notify'
}

function Invoke-AgentNotifyEmit {
    param(
        [string]$EventJson,
        [int]$TimeoutMs = 1500
    )

    if ([string]::IsNullOrWhiteSpace($EventJson)) {
        return 'emit_failed'
    }

    $originalInputEncoding = $null

    try {
        $utf8NoBom = New-Object System.Text.UTF8Encoding -ArgumentList $false
        $originalInputEncoding = [Console]::InputEncoding
        [Console]::InputEncoding = $utf8NoBom

        $startInfo = New-Object System.Diagnostics.ProcessStartInfo
        $startInfo.FileName = Resolve-AgentNotifyEmitter
        $startInfo.Arguments = 'emit --stdin'
        $startInfo.UseShellExecute = $false
        $startInfo.RedirectStandardInput = $true
        $startInfo.RedirectStandardOutput = $true
        $startInfo.RedirectStandardError = $true
        $startInfo.CreateNoWindow = $true

        $process = New-Object System.Diagnostics.Process
        $process.StartInfo = $startInfo

        if (-not $process.Start()) {
            return 'emit_failed'
        }

        $cleanEventJson = $EventJson.TrimStart([char]0xFEFF)
        $eventBytes = $utf8NoBom.GetBytes($cleanEventJson)
        $process.StandardInput.BaseStream.Write($eventBytes, 0, $eventBytes.Length)
        $process.StandardInput.BaseStream.Flush()
        $process.StandardInput.Close()

        if (-not $process.WaitForExit($TimeoutMs)) {
            try {
                $process.Kill()
            }
            catch {
            }

            return 'emit_timeout'
        }

        if ($process.ExitCode -ne 0) {
            return 'emit_failed'
        }

        return 'emit_ok'
    }
    catch {
        return 'emit_failed'
    }
    finally {
        if ($null -ne $originalInputEncoding) {
            [Console]::InputEncoding = $originalInputEncoding
        }
    }
}

function New-AgentNotifyEvent {
    param(
        [object[]]$Arguments,
        [object]$Payload
    )

    $argTool = Get-AgentNotifyArgValue -Arguments $Arguments -Names @('--tool', '-tool')
    $argHookEvent = Get-AgentNotifyArgValue -Arguments $Arguments -Names @('--hook-event', '-hook-event', '--hookEvent', '-hookEvent')
    $argEvent = Get-AgentNotifyArgValue -Arguments $Arguments -Names @('--event', '-event')

    $tool = $argTool
    if ([string]::IsNullOrWhiteSpace($tool)) {
        $tool = Get-AgentNotifyEnvValue 'AGENT_NOTIFY_TOOL'
    }
    if ([string]::IsNullOrWhiteSpace($tool)) {
        $tool = Get-AgentNotifyFirstScalar -Payload $Payload -Paths @('tool', 'agent', 'source.tool')
    }
    if ([string]::IsNullOrWhiteSpace($tool)) {
        $tool = 'unknown'
    }
    $tool = $tool.Trim().ToLowerInvariant()

    $hookEvent = $argHookEvent
    if ([string]::IsNullOrWhiteSpace($hookEvent)) {
        $hookEvent = Get-AgentNotifyEnvValue 'AGENT_NOTIFY_HOOK_EVENT'
    }
    if ([string]::IsNullOrWhiteSpace($hookEvent)) {
        $hookEvent = Get-AgentNotifyFirstScalar -Payload $Payload -Paths @(
            'hook_event_name',
            'hookEvent',
            'hook_event',
            'eventName',
            'event',
            'type'
        )
    }
    if ([string]::IsNullOrWhiteSpace($hookEvent)) {
        $hookEvent = 'Unknown'
    }

    $explicitEvent = $argEvent
    if ([string]::IsNullOrWhiteSpace($explicitEvent)) {
        $explicitEvent = Get-AgentNotifyEnvValue 'AGENT_NOTIFY_EVENT_TYPE'
    }

    $cwd = Get-AgentNotifyFirstScalar -Payload $Payload -Paths @(
        'cwd',
        'project.cwd',
        'workspace.cwd',
        'workingDirectory',
        'working_directory',
        'currentWorkingDirectory',
        'workspaceRoot',
        'projectRoot',
        'directory'
    )
    if ([string]::IsNullOrWhiteSpace($cwd)) {
        $projectEnv = Get-AgentNotifyEnvValue 'AGENT_NOTIFY_PROJECT'
        if (Test-AgentNotifyPathLike $projectEnv) {
            $cwd = $projectEnv
        }
    }
    if ([string]::IsNullOrWhiteSpace($cwd)) {
        $pwdEnv = Get-AgentNotifyEnvValue 'PWD'
        if (Test-AgentNotifyPathLike $pwdEnv) {
            $cwd = $pwdEnv
        }
    }
    if ([string]::IsNullOrWhiteSpace($cwd)) {
        $cwd = (Get-Location).Path
    }

    $projectValue = Get-AgentNotifyFirstScalar -Payload $Payload -Paths @(
        'project.name',
        'projectName',
        'workspace.name',
        'workspaceName'
    )
    if ([string]::IsNullOrWhiteSpace($projectValue)) {
        $projectValue = Get-AgentNotifyEnvValue 'AGENT_NOTIFY_PROJECT'
    }
    $projectName = Get-AgentNotifyProjectName -Cwd $cwd -ProjectValue $projectValue

    $sessionId = Get-AgentNotifyFirstScalar -Payload $Payload -Paths @(
        'sessionId',
        'session_id',
        'session.id',
        'session.uuid',
        'conversation.id',
        'conversationId',
        'conversation_id',
        'codexSessionId',
        'claudeSessionId',
        'thread.id',
        'threadId',
        'thread_id',
        'runId',
        'run_id'
    )
    if ([string]::IsNullOrWhiteSpace($sessionId)) {
        $sessionId = Get-AgentNotifyEnvValue 'AGENT_NOTIFY_SESSION_ID'
    }
    $payloadTranscriptPath = Get-AgentNotifyFirstScalar -Payload $Payload -Paths @(
        'transcriptPath',
        'transcript_path',
        'session.transcriptPath',
        'session.transcript_path',
        'conversation.transcriptPath',
        'conversation.transcript_path'
    )

    $eventType = Resolve-AgentNotifyEventType -Tool $tool -HookEvent $hookEvent -ExplicitEvent $explicitEvent -Payload $Payload
    $severity = Get-AgentNotifySeverity -EventType $eventType
    $toolTitle = Get-AgentNotifyToolLabel -Tool $tool
    $stateLabel = Get-AgentNotifyStateLabel -EventType $eventType

    $detail = Limit-AgentNotifyText -Text (Get-AgentNotifyDetail -EventType $eventType -Payload $Payload) -MaxLength 160
    $bodyAction = '点击返回终端'
    if ($eventType -eq 'user.input_required') {
        $bodyAction = '需要你的下一步'
    }
    elseif ($eventType -eq 'task.completed') {
        $bodyAction = '点击查看结果'
    }
    elseif ($eventType -eq 'task.started' -or $eventType -eq 'heartbeat') {
        $bodyAction = '会话运行中'
    }


    $messageTitle = Limit-AgentNotifyText -Text "$toolTitle $stateLabel" -MaxLength 120
    $messageBody = Limit-AgentNotifyText -Text "$projectName · 当前会话 · $bodyAction" -MaxLength 160

    $currentParentPid = Get-AgentNotifyParentPid -ProcessId $PID
    $payloadPid = Get-AgentNotifyInt (Get-AgentNotifyFirstScalar -Payload $Payload -Paths @(
        'process.pid',
        'pid',
        'processId',
        'process_id'
    ))
    $envPid = Get-AgentNotifyInt (Get-AgentNotifyEnvValue 'AGENT_NOTIFY_PROCESS_PID')
    if ($null -eq $envPid) {
        $envPid = Get-AgentNotifyInt (Get-AgentNotifyEnvValue 'AGENT_NOTIFY_PID')
    }

    $eventPid = $payloadPid
    if ($null -eq $eventPid) {
        $eventPid = $envPid
    }
    if ($null -eq $eventPid) {
        $eventPid = $currentParentPid
    }
    if ($null -eq $eventPid) {
        $eventPid = $PID
    }

    $payloadParentPid = Get-AgentNotifyInt (Get-AgentNotifyFirstScalar -Payload $Payload -Paths @(
        'process.parentPid',
        'process.parent_pid',
        'parentPid',
        'parent_pid'
    ))
    $parentPid = $payloadParentPid
    if ($null -eq $parentPid) {
        $parentPid = Get-AgentNotifyInt (Get-AgentNotifyEnvValue 'AGENT_NOTIFY_PARENT_PID')
    }
    if ($null -eq $parentPid -and $null -ne $eventPid) {
        $parentPid = Get-AgentNotifyParentPid -ProcessId $eventPid
    }

    $processInfo = Get-AgentNotifyProcessInfo -ProcessId $eventPid
    $startedAt = $null
    if ($null -ne $processInfo -and $null -ne $processInfo.StartTime) {
        $startedAt = $processInfo.StartTime.ToString('o')
    }
    $consoleWindowInfo = Get-AgentNotifyConsoleWindowInfo
    if ($null -eq $consoleWindowInfo) {
        $consoleWindowInfo = Find-AgentNotifyPseudoConsoleByAncestor -ProcessId $eventPid
    }
    $consoleProbeHwnd = if ($null -ne $consoleWindowInfo) { [int64]$consoleWindowInfo.hwnd } else { 0 }
    $consoleProbeClass = if ($null -ne $consoleWindowInfo) { $consoleWindowInfo.className } else { '<null>' }
    $consoleProbePid = if ($null -ne $consoleWindowInfo) { [int]$consoleWindowInfo.pid } else { 0 }
    Write-AgentNotifyDebug -Message "console_probe hwnd=$consoleProbeHwnd class=$consoleProbeClass pid=$consoleProbePid hookPid=$PID hookParent=$currentParentPid eventPid=$eventPid"
    $windowProcessInfo = Get-AgentNotifyAncestorWindowInfo -ProcessId $eventPid
    $windowPid = Get-AgentNotifyInt (Get-AgentNotifyFirstScalar -Payload $Payload -Paths @(
        'window.pid',
        'windowPid',
        'terminal.pid',
        'terminalPid'
    ))
    if ($null -eq $windowPid) {
        $windowPid = Get-AgentNotifyInt (Get-AgentNotifyEnvValue 'AGENT_NOTIFY_WINDOW_PID')
    }
    if ($null -eq $windowPid -and $null -ne $consoleWindowInfo) {
        $windowPid = Get-AgentNotifyInt $consoleWindowInfo.pid
    }
    if ($null -eq $windowPid -and $null -ne $windowProcessInfo) {
        $windowPid = Get-AgentNotifyInt $windowProcessInfo.pid
    }

    $windowTitle = Get-AgentNotifyFirstScalar -Payload $Payload -Paths @(
        'window.title',
        'windowTitle',
        'terminal.title',
        'terminalTitle'
    )
    if ([string]::IsNullOrWhiteSpace($windowTitle)) {
        $windowTitle = Get-AgentNotifyEnvValue 'AGENT_NOTIFY_WINDOW_TITLE'
    }
    if ([string]::IsNullOrWhiteSpace($windowTitle) -and $null -ne $consoleWindowInfo) {
        $windowTitle = $consoleWindowInfo.title
    }
    if ([string]::IsNullOrWhiteSpace($windowTitle) -and $null -ne $windowProcessInfo) {
        $windowTitle = $windowProcessInfo.title
    }
    if ([string]::IsNullOrWhiteSpace($windowTitle) -and $null -ne $processInfo) {
        $windowTitle = $processInfo.MainWindowTitle
    }
    if ([string]::IsNullOrWhiteSpace($windowTitle) -and $null -ne $parentPid) {
        $parentProcessInfo = Get-AgentNotifyProcessInfo -ProcessId $parentPid
        if ($null -ne $parentProcessInfo) {
            $windowTitle = $parentProcessInfo.MainWindowTitle
        }
    }

    $hwnd = Get-AgentNotifyInt (Get-AgentNotifyFirstScalar -Payload $Payload -Paths @(
        'window.hwnd',
        'hwnd',
        'terminal.hwnd'
    ))
    if ($null -eq $hwnd) {
        $hwnd = Get-AgentNotifyInt (Get-AgentNotifyEnvValue 'AGENT_NOTIFY_WINDOW_HWND')
    }
    if (($null -eq $hwnd -or $hwnd -eq 0) -and $null -ne $consoleWindowInfo) {
        $hwnd = [int64]$consoleWindowInfo.hwnd
    }
    if (($null -eq $hwnd -or $hwnd -eq 0) -and $null -ne $windowProcessInfo) {
        $hwnd = $windowProcessInfo.hwnd
    }
    if (($null -eq $hwnd -or $hwnd -eq 0) -and $null -ne $processInfo -and $processInfo.MainWindowHandle -ne 0) {
        $hwnd = [int64]$processInfo.MainWindowHandle
    }

    $terminal = Get-AgentNotifyFirstScalar -Payload $Payload -Paths @(
        'window.terminal',
        'terminal',
        'terminal.name'
    )
    if ([string]::IsNullOrWhiteSpace($terminal)) {
        $terminal = Get-AgentNotifyEnvValue 'AGENT_NOTIFY_TERMINAL'
    }
    if ([string]::IsNullOrWhiteSpace($terminal)) {
        $processName = $null
        if ($null -ne $processInfo) {
            $processName = $processInfo.ProcessName
        }
        if ($null -ne $windowProcessInfo -and -not [string]::IsNullOrWhiteSpace($windowProcessInfo.processName)) {
            $processName = $windowProcessInfo.processName
        }
        if ($processName -match 'WindowsTerminal') {
            $terminal = 'WindowsTerminal'
        }
        elseif ($processName -match 'powershell|pwsh') {
            $terminal = 'PowerShell'
        }
        elseif ($processName -match '^cmd$') {
            $terminal = 'cmd'
        }
        elseif ($windowTitle -match 'Windows Terminal') {
            $terminal = 'WindowsTerminal'
        }
    }

    if ([string]::IsNullOrWhiteSpace($sessionId)) {
        $sessionIdentityParts = @($tool, $cwd)
        $hasStableIdentity = $false
        if ($null -ne $eventPid) {
            $sessionIdentityParts += "pid:$eventPid"
            $hasStableIdentity = $true
        }
        if (-not [string]::IsNullOrWhiteSpace($startedAt)) {
            $sessionIdentityParts += "started:$startedAt"
            $hasStableIdentity = $true
        }
        if (-not [string]::IsNullOrWhiteSpace($payloadTranscriptPath)) {
            $sessionIdentityParts += "transcript:$payloadTranscriptPath"
            $hasStableIdentity = $true
        }
        if ($null -ne $hwnd -and $hwnd -ne 0) {
            $sessionIdentityParts += "hwnd:$hwnd"
            $hasStableIdentity = $true
        }
        if ($null -ne $windowPid -and $windowPid -ne 0) {
            $sessionIdentityParts += "windowPid:$windowPid"
            $hasStableIdentity = $true
        }
        if (-not $hasStableIdentity -and -not [string]::IsNullOrWhiteSpace($windowTitle)) {
            $sessionIdentityParts += "title:$windowTitle"
        }
        $sessionSeed = Get-AgentNotifyHash ($sessionIdentityParts -join '|')
        $sessionId = "$tool-$($sessionSeed.Substring(0, 16))"
    }

    $payloadStableId = Get-AgentNotifyFirstScalar -Payload $Payload -Paths @(
        'eventId',
        'event_id',
        'id',
        'requestId',
        'request_id',
        'toolUseId',
        'tool_use_id',
        'toolCallId',
        'tool_call_id',
        'turnId',
        'turn_id'
    )
    if ([string]::IsNullOrWhiteSpace($payloadStableId)) {
        $payloadStableId = 'no-payload-id'
    }

    $summaryHash = Get-AgentNotifyHash "$eventType|$severity|$messageTitle|$messageBody|$detail"
    $eventId = Get-AgentNotifyHash "$tool|$sessionId|$hookEvent|$payloadStableId|$summaryHash"

    return [ordered]@{
        version = 1
        eventId = $eventId
        eventType = $eventType
        severity = $severity
        tool = $tool
        sessionId = $sessionId
        project = [ordered]@{
            cwd = $cwd
            name = $projectName
        }
        message = [ordered]@{
            title = $messageTitle
            body = $messageBody
            detail = $detail
        }
        process = [ordered]@{
            pid = $eventPid
            parentPid = $parentPid
            startedAt = $startedAt
        }
        window = [ordered]@{
            pid = $windowPid
            title = (Limit-AgentNotifyText -Text $windowTitle -MaxLength 160)
            hwnd = $hwnd
            terminal = (Limit-AgentNotifyText -Text $terminal -MaxLength 80)
        }
    }
}

$stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
$eventTypeForLog = 'unknown'
$logCode = 'emit_ok'
$strictMode = (Get-AgentNotifyEnvValue 'AGENT_NOTIFY_STRICT_HOOK') -eq '1'

try {
    $stdinText = Read-AgentNotifyStdin
    $payload = $null

    if (-not [string]::IsNullOrWhiteSpace($stdinText)) {
        try {
            $payload = $stdinText | ConvertFrom-Json -ErrorAction Stop
        }
        catch {
            $logCode = 'parse_failed'
        }
    }

    $event = New-AgentNotifyEvent -Arguments $args -Payload $payload
    $eventTypeForLog = $event.eventType
    $eventJson = $event | ConvertTo-Json -Depth 12 -Compress

    $timeoutMs = Get-AgentNotifyInt (Get-AgentNotifyEnvValue 'AGENT_NOTIFY_HOOK_EMIT_TIMEOUT_MS')
    if ($null -eq $timeoutMs -or $timeoutMs -le 0 -or $timeoutMs -gt 1500) {
        $timeoutMs = 1500
    }

    $emitCode = Invoke-AgentNotifyEmit -EventJson $eventJson -TimeoutMs $timeoutMs
    if ($emitCode -ne 'emit_ok') {
        $logCode = $emitCode
    }
}
catch {
    $logCode = 'unexpected'
    try {
        $errMsg = $_.Exception.Message
        $errPos = $_.InvocationInfo.PositionMessage
        Write-AgentNotifyDebug -Message "unexpected exception=$errMsg pos=$errPos"
    } catch {
    }
}
finally {
    $stopwatch.Stop()
    if ($logCode -ne 'emit_ok') {
        Write-AgentNotifyHookLog -Code $logCode -EventType $eventTypeForLog -ElapsedMs $stopwatch.ElapsedMilliseconds
    }
}

if ($strictMode -and $logCode -ne 'emit_ok') {
    exit 1
}

exit 0
