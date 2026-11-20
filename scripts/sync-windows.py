#!/usr/bin/env python3
"""Sync project sources to a Windows OpenSSH host, once or on local changes."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import io
import json
import ntpath
import os
from pathlib import Path, PureWindowsPath
import subprocess
import sys
import tempfile
import time
import zipfile


ROOT_FILES = {".dockerignore", ".gitignore", "Cargo.toml", "Cargo.lock", "Dockerfile", "README.md", "compose.yaml"}
SOURCE_DIRS = {"apps", "compat", "crates", "docs", "examples", "packaging", "scripts"}
EXCLUDED_DIRS = {".git", "target", "__pycache__"}
# The command is constant. Paths and source bytes travel inside the zipapp on stdin.
RECEIVE_COMMAND = (
    'python -c "import pathlib,shutil,subprocess,sys,tempfile; '
    "d=tempfile.TemporaryDirectory(); p=pathlib.Path(d.name)/'sync.pyz'; "
    "f=p.open('wb'); shutil.copyfileobj(sys.stdin.buffer,f); f.close(); "
    "rc=subprocess.call([sys.executable,str(p),'--receive']); d.cleanup(); sys.exit(rc)\""
)


def source_path(name):
    parts = name.split("/")
    leaf = parts[-1].lower()
    if name not in ROOT_FILES and parts[0] not in SOURCE_DIRS:
        return False
    return not (EXCLUDED_DIRS.intersection(p.lower() for p in parts)
                or leaf == ".env" or leaf.startswith(".env.") or leaf == "node.toml"
                or leaf.endswith((".key", ".pem", ".pfx", ".p12", ".pyc")))


def snapshot():
    root = Path(__file__).resolve().parents[1]
    listing = subprocess.check_output(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"], cwd=root)
    files = {}
    for entry in listing.split(b"\0"):
        name = os.fsdecode(entry)
        if not name or not source_path(name):
            continue
        path = root / name
        if path.resolve() != path:
            raise ValueError(f"Cannot sync a symbolic link: {name}")
        if path.is_file():
            files[name] = path.read_bytes()
    return files


def digest(data):
    return hashlib.sha256(data).hexdigest()


def transfer(host, files, hashes):
    payload = io.BytesIO()
    with zipfile.ZipFile(payload, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("__main__.py", files["scripts/sync-windows.py"])
        archive.writestr("manifest.json", json.dumps(hashes))
        for name, content in files.items():
            archive.writestr("src/" + name, content)
    subprocess.run(["ssh", "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8",
                    "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3",
                    "--", host, RECEIVE_COMMAND],
                   input=payload.getvalue(), check=True)


def reserved_name(part):
    if hasattr(ntpath, "isreserved"):
        return ntpath.isreserved(part)
    return PureWindowsPath(part).is_reserved()


def destination(root, name):
    parts = name.split("/")
    if (not source_path(name) or any(p in {"", ".", ".."} or p.endswith((" ", "."))
            or any(c in '<>:"\\|?*' or ord(c) < 32 for c in p)
            or reserved_name(p) for p in parts)):
        raise ValueError(f"Unsupported Windows source path: {name!r}")
    path = root.joinpath(*parts)
    if path.resolve() != path:
        raise ValueError(f"Destination is redirected: {path}")
    return path


def file_hash(path):
    try:
        return digest(path.read_bytes())
    except FileNotFoundError:
        return None


def replace_file(path, data):
    path.parent.mkdir(exist_ok=True, parents=True)
    temp = tempfile.NamedTemporaryFile(dir=path.parent, delete=False)
    try:
        with temp:
            temp.write(data)
            temp.flush()
            os.fsync(temp.fileno())
        Path(temp.name).replace(path)
    finally:
        Path(temp.name).unlink(missing_ok=True)


def apply_archive(archive, root, manifest):
    wanted = json.loads(archive.read("manifest.json"))
    old = json.loads(manifest.read_text(encoding="utf-8")) if manifest.exists() else {}
    names = sorted(set(old) | set(wanted))
    if len({n.casefold() for n in names}) != len(names):
        raise ValueError("Source paths differ only by case. Resolve their Windows name collision.")
    changes, conflicts = [], []
    for name in names:
        target = destination(root, name)
        current = file_hash(target)
        if current == wanted.get(name):
            continue
        if current != old.get(name):
            conflicts.append(name)
        else:
            changes.append((name, target, current))
    if conflicts:
        raise ValueError("Windows has independent changes. Reconcile these files and retry:\n  "
                         + "\n  ".join(conflicts))

    written = removed = 0
    for name, path, before in changes:
        if file_hash(path) != before:
            raise ValueError(f"File changed during sync: {path}")
        if name in wanted:
            data = archive.read("src/" + name)
            if digest(data) != wanted[name]:
                raise ValueError(f"Archive checksum mismatch: {name}")
            replace_file(path, data)
            if file_hash(path) != wanted[name]:
                raise OSError(f"Write verification failed: {path}")
            written += 1
        else:
            path.unlink()
            removed += 1
    if old != wanted:
        replace_file(manifest, json.dumps(wanted, sort_keys=True).encode())
    print(f"{root}: {len(wanted)} files verified, {written} written, {removed} removed.", flush=True)


def receive():
    import msvcrt

    root = (Path.home() / "peerhatch").resolve()
    state = Path(os.environ["LOCALAPPDATA"]) / "PeerHatch" / "source-sync"
    state.mkdir(parents=True, exist_ok=True)
    with (state / "lock").open("a+b") as lock, zipfile.ZipFile(sys.argv[0]) as archive:
        lock.seek(0)
        msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
        apply_archive(archive, root, state / "manifest.json")


def build_inputs(hashes):
    return {name: value for name, value in hashes.items()
            if name.startswith(("apps/", "crates/", "compat/")) or name in {
                "Cargo.toml", "Cargo.lock", "scripts/dev-update.py", "scripts/dev_windows.py"}}


def dev_command(host, action):
    update = Path(__file__).with_name("dev-update.py")
    if host is None:
        command = [sys.executable, "-u", str(update), action]
    else:
        command = ["ssh", "-nT", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8",
                   "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3", "--",
                   host, "python -u peerhatch/scripts/dev-update.py " + action]
    subprocess.run(command, check=True)


def update_pair(host, version):
    print("Building the Linux and Windows development binaries.", flush=True)
    with ThreadPoolExecutor(max_workers=2) as pool:
        jobs = [pool.submit(dev_command, node, "build") for node in (None, host)]
        for job in jobs:
            job.result()
    if version != build_inputs({name: digest(data) for name, data in snapshot().items()}):
        print("Sources changed during the build. Updating to the newer revision.", flush=True)
        return False
    dev_command(host, "apply")
    dev_command(None, "apply")
    print("Both development checkouts are up to date.", flush=True)
    return True


def main():
    if sys.argv[1:] == ["--receive"]:
        receive()
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default=os.getenv("PEERHATCH_DEV_WINDOWS_HOST"), help="Windows SSH host (or PEERHATCH_DEV_WINDOWS_HOST)")
    parser.add_argument("--watch", action="store_true", help="continue syncing local source changes")
    parser.add_argument("--dev", action="store_true", help="build on both machines and update the running development apps")
    args = parser.parse_args()
    if not args.host:
        parser.error("Set --host or PEERHATCH_DEV_WINDOWS_HOST to your Windows SSH host")
    if args.dev:
        if not sys.platform.startswith("linux"):
            parser.error("--dev runs on the Linux development machine")
        import fcntl
        state = Path(__file__).resolve().parents[1] / "target" / "dev-watch"
        state.mkdir(parents=True, exist_ok=True)
        owner = (state / "watch.lock").open("a+b")
        try:
            fcntl.flock(owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            parser.error("A development update is already running for this checkout")
    last = None
    own_source = digest(Path(__file__).read_bytes())
    observed = applied = failed = None
    retry_after = 0
    while True:
        try:
            files = snapshot()
            hashes = {name: digest(data) for name, data in files.items()}
            if args.dev and args.watch and hashes != observed:
                observed = hashes
                time.sleep(1)
                continue
            if args.dev and args.watch and hashes["scripts/sync-windows.py"] != own_source:
                compile(files["scripts/sync-windows.py"], __file__, "exec")
                print("Reloading the development watcher.", flush=True)
                os.execv(sys.executable, [sys.executable, "-u", __file__, *sys.argv[1:]])
            if hashes != last:
                transfer(args.host, files, hashes)
                last = hashes
            if args.dev:
                version = build_inputs(hashes)
                if applied != version and (failed != version or time.monotonic() >= retry_after):
                    # A failed activation may have updated just one checkout. Even
                    # a revert to the previous revision must reconcile both copies.
                    applied = None
                    failed = version
                    retry_after = time.monotonic() + 30
                    if update_pair(args.host, version):
                        applied, failed = version, None
                    elif not args.watch:
                        raise ValueError("Sources changed during the build. Run the update again.")
        except (OSError, ValueError, SyntaxError, subprocess.SubprocessError) as error:
            if not args.watch:
                raise
            print(f"{error}\nWaiting to retry.", file=sys.stderr, flush=True)
            retry_after = time.monotonic() + 30
            time.sleep(2 if args.dev else 15)
            continue
        if not args.watch:
            return
        time.sleep(2)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        sys.exit(str(error))
