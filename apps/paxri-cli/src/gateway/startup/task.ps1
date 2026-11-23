$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$name = $env:P2P_STARTUP_TASK
$programs = [Environment]::GetFolderPath([Environment+SpecialFolder]::ProgramFiles)
$helper = Join-Path $programs ("paxri Startup " + $name + ".exe")
$draft = $null
$backup = $null
$created = $false
try {
    $user = [Security.Principal.WindowsIdentity]::GetCurrent()
    $sid = $user.User.Value
    $mode = $env:P2P_STARTUP_MODE
    $service = New-Object -ComObject Schedule.Service
    $service.Connect()
    $folder = $service.GetFolder('\')
    # Keep a moved/renamed installation on the same per-profile registration.
    $found = $false
    foreach ($brand in @('paxri', 'peerhatch', 'p2ptunnel')) {
        $candidate = $env:P2P_STARTUP_TASK.Replace('org.paxri.', "org.$brand.")
        try { $null = $folder.GetTask($candidate) }
        catch { if ($_.Exception.GetBaseException().HResult -eq -2147024894) { continue }; throw }
        if ($found) { throw 'Multiple startup tasks exist for this profile. Resolve the duplicate registrations in Task Scheduler.' }
        $found = $true
        $name = $candidate
        $title = switch ($brand) { 'paxri' { 'paxri' } 'peerhatch' { 'PeerHatch' } 'p2ptunnel' { 'P2P Tunnel' } }
        $helper = Join-Path $programs ("$title Startup " + $name + ".exe")
    }
    if ($mode -eq 'system' -or $mode -eq 'login') {
        $task = $service.NewTask(0)
        $task.RegistrationInfo.Description = 'Start the paxri gateway for this account.'
        $task.Principal.UserId = $sid
        $task.Principal.RunLevel = 0
        $task.Settings.DisallowStartIfOnBatteries = $false
        $task.Settings.StopIfGoingOnBatteries = $false
        $task.Settings.StartWhenAvailable = $true
        $action = $task.Actions.Create(0)
        if ($mode -eq 'system') {
            if (!([Security.Principal.WindowsPrincipal]::new($user)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
                throw 'System startup requires administrator permission.'
            }
            # The batch logon can be elevated. Protect its first executable and task definition.
            $draft = Join-Path $programs ([Guid]::NewGuid().ToString() + '.tmp')
            [IO.File]::Copy($env:P2P_BOOT_SOURCE, $draft)
            $fileAcl = [Security.AccessControl.FileSecurity]::new()
            $fileAcl.SetAccessRuleProtection($true, $false)
            $admins = [Security.Principal.SecurityIdentifier]::new('S-1-5-32-544')
            $fileAcl.SetOwner($admins)
            foreach ($grant in @(@('S-1-5-18', 'FullControl'), @('S-1-5-32-544', 'FullControl'), @('S-1-5-32-545', 'ReadAndExecute'))) {
                $principal = [Security.Principal.SecurityIdentifier]::new($grant[0])
                $fileAcl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($principal, $grant[1], 'Allow'))
            }
            [IO.File]::SetAccessControl($draft, $fileAcl)
            if ([IO.File]::Exists($helper)) {
                $backup = Join-Path $programs ([Guid]::NewGuid().ToString() + '.bak')
                if (([IO.File]::GetAttributes($helper) -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Unexpected link at the boot helper path.' }
                [IO.File]::Replace($draft, $helper, $backup)
                [IO.File]::SetAccessControl($helper, $fileAcl)
            } else {
                [IO.File]::Move($draft, $helper)
                $created = $true
            }
            $action.Path = $helper
            $action.Arguments = $env:P2P_BOOT_ARGS
            $action.WorkingDirectory = [Environment]::SystemDirectory
            $null = $task.Triggers.Create(8) # TASK_TRIGGER_BOOT
            $logon = 2 # TASK_LOGON_S4U
            $acl = 'O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FR;;;AU)'
        } else {
            $action.Path = $env:P2P_STARTUP_EXE
            $action.Arguments = $env:P2P_STARTUP_ARGS
            $task.Triggers.Create(9).UserId = $sid # TASK_TRIGGER_LOGON
            $logon = 3 # TASK_LOGON_INTERACTIVE_TOKEN
            $acl = "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;$sid)"
        }
        $task.Principal.LogonType = $logon
        # CREATE_OR_UPDATE | DONT_ADD_PRINCIPAL_ACE preserves the exact access policy.
        $null = $folder.RegisterTaskDefinition($name, $task, 22, $sid, $null, $logon, $acl)
        $created = $false
    } else {
        try { $folder.DeleteTask($name, 0) }
        catch { if ($_.Exception.GetBaseException().HResult -ne -2147024894) { throw } }
    }
    if ($env:P2P_PREVIOUS_MODE -eq 'system' -and $mode -ne 'system') { [IO.File]::Delete($helper) }
} catch {
    if ($created) { [IO.File]::Delete($helper) }
    if ($backup -and [IO.File]::Exists($backup)) { [IO.File]::Replace($backup, $helper, [System.Management.Automation.Language.NullString]::Value) }
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
} finally {
    if ($draft) { [IO.File]::Delete($draft) }
    if ($backup) { [IO.File]::Delete($backup) }
}
exit 0
