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
    $expected = DisplayPath $env:PAXRI_STARTUP_EXE
    $found = 0
    $id = $env:PAXRI_STARTUP_TASK
    try {
        $task = $folder.GetTask($id)
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
            if ($action.Type -ne 0) { "Windows task $id uses a non-executable action."; continue }
            $image = DisplayPath $action.Path
            $client = $image
            if ($action.Arguments -match '(?:^|\s)--client\s+"([^"]+)"') { $client = DisplayPath $Matches[1] }
            $notes = @()
            if (![IO.File]::Exists($image)) { $notes += 'scheduled executable missing' }
            if ($client -ne $image -and ![IO.File]::Exists($client)) { $notes += 'client missing' }
            if ([IO.Path]::GetFileName($client) -in @('paxri.exe','paxri-gui.exe') -and
                ![IO.File]::Exists((Join-Path ([IO.Path]::GetDirectoryName($client)) 'paxri-gateway.exe'))) { $notes += 'gateway executable missing' }
            if ([IO.Path]::GetDirectoryName($client) -ne [IO.Path]::GetDirectoryName($expected) -or
                [IO.Path]::GetFileName($client) -notin @('paxri.exe', 'paxri-gui.exe')) { $notes += 'different installation' }
            if (!$task.Enabled -or $env:PAXRI_STARTUP_MODE -notin $modes) { $notes += 'does not match configured startup' }
            $detail = if ($notes.Count) { ' (' + ($notes -join ', ') + ')' }
            "Windows task [$($modes -join ', '), $state]: $client$detail"
            if ($client -ne $image) { "Boot helper: $image" }
        }
    } catch {
        if ($_.Exception.GetBaseException().HResult -ne -2147024894) { throw }
    }
    if ($found -eq 0) {
        if ($env:PAXRI_STARTUP_MODE -in @('login','system')) { 'Windows task: missing for the configured startup mode' }
        else { 'Windows task: none registered' }
    }
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
exit 0
