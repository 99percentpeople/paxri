"""Activate a P2P Tunnel development build in its Windows user's desktop session."""
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import subprocess
import sys
import uuid


def copy_dacl(source, target):
    security = ctypes.WinDLL("advapi32", use_last_error=True)
    read = security.GetFileSecurityW
    read.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.LPVOID, wintypes.DWORD,
                     ctypes.POINTER(wintypes.DWORD)]
    write = security.SetFileSecurityW
    write.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.LPVOID]
    dacl = 4  # DACL_SECURITY_INFORMATION
    size = wintypes.DWORD()
    read(str(source), dacl, None, 0, ctypes.byref(size))
    if not size.value:
        raise ctypes.WinError(ctypes.get_last_error())
    descriptor = ctypes.create_string_buffer(size.value)
    if not read(str(source), dacl, descriptor, size.value, ctypes.byref(size)):
        raise ctypes.WinError(ctypes.get_last_error())
    if not write(str(target), dacl, descriptor):
        raise ctypes.WinError(ctypes.get_last_error())


def powershell(command, *, timeout=30, **variables):
    env = os.environ.copy()
    env.update({k: str(v) for k, v in variables.items()})
    return subprocess.check_output(["powershell", "-NoProfile", "-NonInteractive", "-Command",
        "$ErrorActionPreference='Stop';[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false);" + command],
        env=env, timeout=timeout, text=True, encoding="utf8", creationflags=subprocess.CREATE_NO_WINDOW)


def argv(command):
    parse = ctypes.windll.shell32.CommandLineToArgvW
    parse.argtypes = [wintypes.LPCWSTR, ctypes.POINTER(ctypes.c_int)]
    parse.restype = ctypes.POINTER(wintypes.LPWSTR)
    count = ctypes.c_int()
    args = parse(command, ctypes.byref(count))
    if not args:
        raise ctypes.WinError()
    try:
        return args[:count.value]
    finally:
        ctypes.windll.kernel32.LocalFree(ctypes.cast(args, ctypes.c_void_p))


def apps(live, all_sessions=False):
    processes = json.loads(powershell("""
        $s=(Get-Process -Id $PID).SessionId
        $all=@(Get-CimInstance Win32_Process -Filter "Name='tunnel-gateway.exe' OR Name='tunnel-gui.exe'" |
          Where-Object {$env:P2P_DEV_ALL -eq '1' -or $_.SessionId -eq $s} |
          Select-Object ProcessId,ExecutablePath,CommandLine,Name)
        ConvertTo-Json -Compress -InputObject $all
    """, P2P_DEV_ALL=int(all_sessions)))
    return [dict(name=p["Name"], pid=p["ProcessId"], exe=p["ExecutablePath"], command=p["CommandLine"],
                 args=argv(p["CommandLine"])) for p in processes
            if p["ExecutablePath"] and Path(p["ExecutablePath"]) == live / p["Name"]]


def stop(app):
    powershell("""
        $p=Get-Process -Id ([int]$env:P2P_DEV_PID) -ErrorAction SilentlyContinue
        if(!$p){return}
        if($p.Path -ine $env:P2P_DEV_EXE -or $p.SessionId -ne (Get-Process -Id $PID).SessionId) {
            throw 'The running development process changed'
        }
        if($p.MainWindowHandle -ne 0){[void]$p.CloseMainWindow();[void]$p.WaitForExit(3000)}
        if(!$p.HasExited){Stop-Process -InputObject $p}
        if(!$p.WaitForExit(6000)){throw 'Process stop timed out'}
    """, P2P_DEV_PID=app["pid"], P2P_DEV_EXE=app["exe"])


def start(app, root):
    logs = Path(os.environ["LOCALAPPDATA"]) / "P2PTunnel"
    logs.mkdir(parents=True, exist_ok=True)
    with (logs / (Path(app["name"]).stem.removeprefix("tunnel-") + ".log")).open("ab") as log:
        return subprocess.Popen(app["command"], executable=app["exe"], cwd=root, stdin=subprocess.DEVNULL,
                                stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW).pid


def responsive(pid):
    found = []
    callback = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    user = ctypes.windll.user32
    user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user.IsWindowVisible.argtypes = user.IsHungAppWindow.argtypes = [wintypes.HWND]

    @callback
    def visit(window, _):
        owner = wintypes.DWORD()
        user.GetWindowThreadProcessId(window, ctypes.byref(owner))
        if owner.value == pid and user.IsWindowVisible(window) and not user.IsHungAppWindow(window):
            found.append(window)
        return True
    user.EnumWindows(visit, 0)
    return bool(found)


def interactive(script, state):
    name = "P2PTunnel-Dev-" + uuid.uuid4().hex
    receipt = state / (name + ".json")
    # An SSH shell runs in session 0. Launch the replacement GUI on its owner's desktop.
    try:
        powershell("""
          $action=New-ScheduledTaskAction -Execute $env:P2P_DEV_PY -Argument ('"'+$env:P2P_DEV_SCRIPT+'" desktop "'+$env:P2P_DEV_RESULT+'"')
          $user=New-ScheduledTaskPrincipal -UserId ([Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
          $settings=New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
          Register-ScheduledTask -TaskName $env:P2P_DEV_TASK -Action $action -Principal $user -Settings $settings | Out-Null
          try {
            Start-ScheduledTask -TaskName $env:P2P_DEV_TASK
            $deadline=[DateTime]::UtcNow.AddSeconds(100)
            while (!(Test-Path -LiteralPath $env:P2P_DEV_RESULT)) {
              if([DateTime]::UtcNow -gt $deadline){throw 'Waiting for the interactive Windows desktop timed out'}
              Start-Sleep -Milliseconds 250
            }
          } finally {Unregister-ScheduledTask -TaskName $env:P2P_DEV_TASK -Confirm:$false}
        """, timeout=115, P2P_DEV_PY=Path(sys.executable).with_name("pythonw.exe"), P2P_DEV_TASK=name,
                   P2P_DEV_SCRIPT=script, P2P_DEV_RESULT=receipt)
        result = json.loads(receipt.read_text(encoding="utf8"))
        if "error" in result:
            raise RuntimeError(result["error"])
        print(result.get("message", "Windows desktop updated."), flush=True)
    finally:
        receipt.unlink(missing_ok=True)
