#!/usr/bin/env python3
"""Build or activate this checkout's native development binaries on Linux or Windows."""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import uuid


ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "target"
STAGE = TARGET / "dev-watch" / "debug"
LIVE = TARGET / "debug"
WINDOWS = os.name == "nt"
NAMES = [n + (".exe" if WINDOWS else "") for n in ("paxri", "paxri-gateway", "paxri-gui")]
if WINDOWS:
    import windows_conpty
    NAMES.extend(windows_conpty.FILES)


def install_targets():
    yield LIVE, NAMES
    if WINDOWS:
        config = STAGE.parent / "gui-directory.txt"
        if config.is_file():
            directory = Path(config.read_text(encoding="utf-8-sig").strip())
            if not directory.is_absolute() or not directory.is_dir():
                raise ValueError(f"{config} must name an existing absolute directory")
            directory = directory.resolve()
            if directory == STAGE.resolve():
                raise ValueError(f"{config} points to the Cargo build directory")
            if directory != LIVE.resolve():
                yield directory, [NAMES[2]]


def changed_files(live, names):
    for name in names:
        if not (STAGE / name).is_file():
            raise ValueError(f"Missing development build: {STAGE / name}")
    return [name for name in names if digest(STAGE / name) != digest(live / name)]


def build():
    env = os.environ.copy()
    env.setdefault("CARGO_PROFILE_DEV_DEBUG", "0")
    env["CARGO_BUILD_BUILD_DIR"] = str(TARGET)
    subprocess.run(["cargo", "build", "--locked", "--target-dir", str(STAGE.parent),
                    "-p", "paxri-cli", "-p", "paxri-gateway", "-p", "paxri-gui"],
                   cwd=ROOT, env=env, check=True)
    if WINDOWS:
        for name, data in windows_conpty.runtime_files(STAGE / NAMES[1], TARGET / "conpty").items():
            (STAGE / name).write_bytes(data)


def digest(path):
    if not path.exists():
        return None
    result = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            result.update(chunk)
    return result.digest()


def systemctl(*args):
    return subprocess.check_output(["systemctl", "--user", *args], text=True).strip()


def linux_apps():
    apps = []
    for name in NAMES[1:]:
        unit = name + ".service"
        result = subprocess.run(["systemctl", "--user", "show", unit, "-p", "MainPID", "--value"],
                                capture_output=True, text=True)
        pid = int(result.stdout.strip() or 0)
        if not pid:
            continue
        process = Path(f"/proc/{pid}")
        image = os.readlink(process / "exe").removesuffix(" (deleted)")
        if Path(image) != LIVE / name:
            continue
        args = os.fsdecode((process / "cmdline").read_bytes()).rstrip("\0").split("\0")
        apps.append(dict(name=name, pid=pid, unit=unit, args=args))
    return apps


def gateway_status(app):
    args = app["args"]
    cli = [str(LIVE / NAMES[0])]
    for i, arg in enumerate(args):
        if arg == "--ipc":
            cli.extend(args[i:i + 2])
        elif arg.startswith("--ipc="):
            cli.append(arg)
    env = os.environ.copy()
    if not WINDOWS:
        proc_env = Path(f"/proc/{app['pid']}/environ").read_bytes().split(b"\0")
        for entry in proc_env:
            if entry.startswith(b"PAXRI_IPC="):
                env["PAXRI_IPC"] = os.fsdecode(entry.partition(b"=")[2])
    return json.loads(subprocess.check_output([*cli, "status", "--json"], env=env,
                      stderr=subprocess.DEVNULL, timeout=5))["device_id"]


def start(app):
    if WINDOWS:
        import dev_windows
        directory = Path(app["exe"]).parent
        app["pid"] = dev_windows.start(app, ROOT if directory == LIVE else directory)
    else:
        systemctl("restart", app["unit"])
        app["pid"] = int(systemctl("show", app["unit"], "-p", "MainPID", "--value"))


def verify(app):
    last = None
    for _ in range(30):
        try:
            if app["name"] == NAMES[1]:
                if gateway_status(app) != app["id"]:
                    raise ValueError("Gateway identity changed")
            elif WINDOWS:
                import dev_windows
                if not dev_windows.responsive(app["pid"]):
                    raise ValueError("Desktop window is not ready")
            elif not (Path(f"/proc/{app['pid']}/exe").samefile(LIVE / app["name"])):
                raise ValueError("Desktop executable changed")
            return
        except (OSError, ValueError, subprocess.SubprocessError) as e:
            last = e
            time.sleep(.25)
    raise RuntimeError(f"{app['name']} failed its startup check: {last}")


def install():
    # A remote activation can outlive the SSH shell that requested it.
    with (STAGE.parent / "apply.lock").open("a+b") as lock:
        if WINDOWS:
            import msvcrt
            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        for live, names in install_targets():
            activate(live, names)


def activate(live, names):
    if WINDOWS:
        import dev_windows
    changed = changed_files(live, names)
    apps = dev_windows.apps(live) if WINDOWS else linux_apps()
    apps = [a for a in apps if needs_restart(a["name"], changed) or
            (not WINDOWS and not Path(f"/proc/{a['pid']}/exe").samefile(live / a["name"]))]
    if not changed and not apps:
        print("Development binaries are current.", flush=True)
        return
    for app in apps:
        if app["name"] == NAMES[1]:
            app["id"] = gateway_status(app)
    live.mkdir(parents=True, exist_ok=True)
    if WINDOWS:
        # mkdtemp uses a protected owner-only ACL on Windows. Executables moved
        # from there can become inaccessible to the user's non-elevated desktop.
        work = live / (".dev-update-" + uuid.uuid4().hex)
        work.mkdir()
    else:
        work = Path(tempfile.mkdtemp(prefix="activate-", dir=STAGE.parent))
    installed, stopped, started = [], [], []
    cleanup = True
    try:
        for name in changed:
            shutil.copy2(STAGE / name, work / name)
            if (live / name).exists():
                if WINDOWS:
                    # Preserve any per-file DACL on both the new image and the rollback image.
                    dev_windows.copy_dacl(live / name, work / name)
                    os.link(live / name, work / (name + ".old"))
                else:
                    shutil.copy2(live / name, work / (name + ".old"))
        if WINDOWS:
            for app in apps:
                dev_windows.stop(app)
                stopped.append(app)
        for name in changed:
            dest = live / name
            try:
                os.replace(work / name, dest)
            except PermissionError:
                if not WINDOWS:
                    raise
                # A mapped CLI can keep serving its existing session under its old name.
                os.rename(dest, work / (name + ".mapped"))
                try:
                    os.replace(work / name, dest)
                except BaseException:
                    os.rename(work / (name + ".mapped"), dest)
                    raise
            installed.append(name)
        for app in apps:
            start(app)
            started.append(app)
            verify(app)
        print(f"Updated {live}:", ", ".join(changed) or "running images",
              "| Restarted:", ", ".join(a["name"] for a in apps) or "none", flush=True)
    except Exception:
        cleanup = False
        if WINDOWS:
            for app in reversed(started):
                dev_windows.stop(app)
        for name in reversed(installed):
            old = work / (name + ".old")
            if old.exists():
                os.replace(old, live / name)
            else:
                (live / name).unlink()
        for app in (stopped if WINDOWS else (apps if installed or started else [])):
            start(app)
            verify(app)
        cleanup = True
        raise
    finally:
        # Windows may keep the retired CLI mapped until its last SSH session exits.
        if cleanup:
            shutil.rmtree(work, ignore_errors=True)
        else:
            print(f"Recovery copies are in {work}", file=sys.stderr, flush=True)
        if WINDOWS:
            for old in live.glob(".dev-update-*/*.mapped"):
                with contextlib.suppress(OSError):
                    old.unlink()
                    old.parent.rmdir()


def needs_restart(name, changed):
    return name in changed or (WINDOWS and name == NAMES[1] and
                               any(file in changed for file in windows_conpty.FILES))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["build", "apply", "desktop"])
    parser.add_argument("receipt", type=Path, nargs="?")
    options = parser.parse_args()
    if options.action == "build":
        build()
    elif WINDOWS and options.action == "apply":
        import dev_windows
        pending = [(live, changed) for live, names in install_targets()
                   if (changed := changed_files(live, names))]
        if not pending:
            print("Windows development binaries are current.", flush=True)
        elif any(needs_restart(app["name"], changed) for live, changed in pending
                 for app in dev_windows.apps(live, all_sessions=True)):
            dev_windows.interactive(Path(__file__).resolve(), STAGE.parent)
        else:
            install()
    elif options.action == "desktop":
        if not WINDOWS or options.receipt is None:
            parser.error("desktop is the Windows interactive activation entry point")
        import io
        import traceback
        try:
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                install()
            result = {"message": output.getvalue().strip()}
        except Exception:
            result = {"error": traceback.format_exc()}
        temp = options.receipt.with_suffix(".tmp")
        temp.write_text(json.dumps(result), encoding="utf8")
        os.replace(temp, options.receipt)
    else:
        install()


if __name__ == "__main__":
    main()
