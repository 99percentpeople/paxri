$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
function DisplayPath([string]$path) {
    if ($path.StartsWith('\\?\UNC\')) { return '\\' + $path.Substring(8) }
    if ($path.StartsWith('\\?\')) { return $path.Substring(4) }
    return $path
}
try {
    $service = New-Object -ComObject Schedule.Service
    $service.Connect()
    $folder = $service.GetFolder('\')
    $expected = DisplayPath $env:P2P_STARTUP_EXE
    $found = 0
    foreach ($name in @($env:P2P_STARTUP_TASK, $env:P2P_STARTUP_TASK.Replace('org.peerhatch.', 'org.p2ptunnel.'))) {
        try { $task = $folder.GetTask($name) }
        catch { if ($_.Exception.GetBaseException().HResult -eq -2147024894) { continue }; throw }
        $found++
        $definition = $task.Definition
        $modes = @(foreach ($trigger in $definition.Triggers) {
            if (!$trigger.Enabled) { 'disabled trigger' }
            elseif ($trigger.Type -eq 8) { 'system' }
            elseif ($trigger.Type -eq 9) { 'login' }
            else { 'other trigger' }
        })
        $state = if ($task.Enabled) { 'enabled' } else { 'disabled' }
        foreach ($action in $definition.Actions) {
            if ($action.Type -ne 0) { "Windows task $name uses a non-executable action."; continue }
            $image = DisplayPath $action.Path
            $client = $image
            if ($action.Arguments -match '(?:^|\s)--client\s+"([^"]+)"') { $client = DisplayPath $Matches[1] }
            $notes = @()
            if (![IO.File]::Exists($image)) { $notes += 'scheduled executable missing' }
            if ($client -ne $image -and ![IO.File]::Exists($client)) { $notes += 'client missing' }
            $gatewayName = switch ([IO.Path]::GetFileName($client)) {
                { $_ -in @('peerhatch.exe','peerhatch-gui.exe') } { 'peerhatch-gateway.exe' }
                { $_ -in @('tunnel.exe','tunnel-gui.exe') } { 'tunnel-gateway.exe' }
            }
            if ($gatewayName -and ![IO.File]::Exists((Join-Path ([IO.Path]::GetDirectoryName($client)) $gatewayName))) { $notes += 'gateway executable missing' }
            if ([IO.Path]::GetDirectoryName($client) -ne [IO.Path]::GetDirectoryName($expected) -or
                [IO.Path]::GetFileName($client) -notin @('peerhatch.exe', 'peerhatch-gui.exe')) { $notes += 'different installation' }
            if (!$task.Enabled -or $env:P2P_STARTUP_MODE -notin $modes) { $notes += 'does not match configured startup' }
            $detail = if ($notes.Count) { ' (' + ($notes -join ', ') + ')' }
            "Windows task [$($modes -join ', '), $state]: $client$detail"
            if ($client -ne $image) { "Boot helper: $image" }
        }
    }
    if ($found -eq 0) {
        if ($env:P2P_STARTUP_MODE -in @('login','system')) { 'Windows task: missing for the configured startup mode' }
        else { 'Windows task: none registered' }
    } elseif ($found -gt 1) { 'Multiple startup tasks are registered for this profile.' }
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
exit 0
