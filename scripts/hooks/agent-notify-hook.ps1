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

    $number = 0
    if ([int]::TryParse($text, [ref]$number)) {
        return $number
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
        'task.started' { return 'session started' }
        'task.completed' { return 'task completed' }
        'task.failed' { return 'execution failed' }
        'user.confirmation_required' { return 'needs confirmation' }
        'user.input_required' { return 'waiting for input' }
        'tool.blocked' { return 'execution blocked' }
        default { return 'session updated' }
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
        'task.started' { return 'Session started.' }
        'task.completed' {
            if ($null -ne $exitCode) {
                return "Exit code $exitCode."
            }

            return 'Task completed.'
        }
        'task.failed' {
            if ($null -ne $exitCode) {
                return "Execution failed with exit code $exitCode."
            }

            return 'Execution failed; details hidden.'
        }
        'tool.blocked' { return 'Permission, sandbox, or tool call blocked.' }
        'user.confirmation_required' { return 'Permission request; arguments hidden.' }
        'user.input_required' { return 'Waiting for your next input; details hidden.' }
        default { return 'Session status updated.' }
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

function Write-AgentNotifyHookLog {
    param(
        [string]$Code,
        [string]$EventType,
        [long]$ElapsedMs
    )

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
        $timestamp = (Get-Date).ToString('o')
        $safeCode = Limit-AgentNotifyText -Text $Code -MaxLength 48
        $safeEventType = Limit-AgentNotifyText -Text $EventType -MaxLength 64
        $line = "$timestamp component=hook eventType=$safeEventType code=$safeCode elapsedMs=$ElapsedMs"
        Add-Content -Path $logPath -Value $line -Encoding UTF8
    }
    catch {
    }
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
        $startInfo.FileName = 'agent-notify'
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
        'conversationId',
        'conversation_id',
        'codexSessionId',
        'claudeSessionId',
        'runId',
        'run_id'
    )
    if ([string]::IsNullOrWhiteSpace($sessionId)) {
        $sessionId = Get-AgentNotifyEnvValue 'AGENT_NOTIFY_SESSION_ID'
    }
    if ([string]::IsNullOrWhiteSpace($sessionId)) {
        $sessionSeed = Get-AgentNotifyHash "$tool|$cwd"
        $sessionId = "$tool-$($sessionSeed.Substring(0, 16))"
    }

    $eventType = Resolve-AgentNotifyEventType -Tool $tool -HookEvent $hookEvent -ExplicitEvent $explicitEvent -Payload $Payload
    $severity = Get-AgentNotifySeverity -EventType $eventType
    $toolTitle = (Get-Culture).TextInfo.ToTitleCase($tool)
    $stateLabel = Get-AgentNotifyStateLabel -EventType $eventType

    $detail = Limit-AgentNotifyText -Text (Get-AgentNotifyDetail -EventType $eventType -Payload $Payload) -MaxLength 160
    $bodyAction = 'click to return terminal'
    if ($eventType -eq 'user.input_required') {
        $bodyAction = 'needs your next step'
    }
    elseif ($eventType -eq 'task.completed') {
        $bodyAction = 'click to view result'
    }
    elseif ($eventType -eq 'task.started' -or $eventType -eq 'heartbeat') {
        $bodyAction = 'session is active'
    }

    $messageTitle = Limit-AgentNotifyText -Text "$toolTitle $stateLabel" -MaxLength 120
    $messageBody = Limit-AgentNotifyText -Text "$projectName - current session - $bodyAction" -MaxLength 160

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

    $windowTitle = Get-AgentNotifyFirstScalar -Payload $Payload -Paths @(
        'window.title',
        'windowTitle',
        'terminal.title',
        'terminalTitle'
    )
    if ([string]::IsNullOrWhiteSpace($windowTitle)) {
        $windowTitle = Get-AgentNotifyEnvValue 'AGENT_NOTIFY_WINDOW_TITLE'
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
