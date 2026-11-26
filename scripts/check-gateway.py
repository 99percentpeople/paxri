#!/usr/bin/env python3
"""Exercise the installed CLI and two real gateways without any public infrastructure."""
import argparse
import asyncio
import contextlib
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import shutil
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading
import time


class Target(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.sendall(b"ready\n")
        with self.request.makefile("rb") as source:
            data = source.read()
        self.request.sendall(data[::-1])


class TargetServer(socketserver.ThreadingTCPServer):
    daemon_threads = True


class StreamingEcho(socketserver.BaseRequestHandler):
    def handle(self):
        while block := self.request.recv(8192):
            self.request.sendall(block)


class FinishFirst(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.sendall(b"greeting")
        self.request.shutdown(socket.SHUT_WR)
        with self.request.makefile("rb") as reader:
            if reader.read() == b"after EOF":
                self.server.received.set()


async def check_remote_eof(command):
    process = await asyncio.create_subprocess_exec(
        *command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        assert await asyncio.wait_for(process.stdout.read(), 10) == b"greeting"
        process.stdin.write(b"after EOF")
        await process.stdin.drain()
        process.stdin.close()
        assert await asyncio.wait_for(process.wait(), 10) == 0, await process.stderr.read()
    finally:
        if process.returncode is None:
            process.kill()
            await process.wait()


def check_native_terminal(root, keys, call, start, cli, ipc, wait):
    import errno
    import fcntl
    import re
    import select
    import signal
    import struct
    import termios

    # The client knows the host address, but the host has not approved the client yet.
    (root / "host.toml").write_text('version=1\ndevice_name="host"\n')
    host_process, host = start("host")
    (root / "client.toml").write_text(
        f'version=1\ndevice_name="client"\n[peers.host]\nendpoint_id="{keys["host"]}"\n'
        f'ticket="{host["ticket"]}"\nallow_terminal=false\n')
    client_process, _ = start("client")

    @contextlib.contextmanager
    def session(*command):
        master, slave = os.openpty()
        mode = termios.tcgetattr(slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 31, 89, 0, 0))
        child = subprocess.Popen([cli, "--ipc", ipc("client"), *(command or ("shell", "host"))],
                                 stdin=slave, stdout=slave, stderr=slave)
        try:
            yield child, master, slave
            if child.poll() is not None:
                assert termios.tcgetattr(slave) == mode, "CLI did not restore the terminal"
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
            os.close(master)
            os.close(slave)

    def read_until(fd, pattern):
        data = bytearray()
        end = time.monotonic() + 15
        while time.monotonic() < end:
            if not select.select([fd], [], [], max(0, end - time.monotonic()))[0]:
                break
            try:
                block = os.read(fd, 32768)
            except OSError as e:
                if e.errno == errno.EIO:
                    break
                raise
            if not block:
                break
            data.extend(block)
            if match := re.search(pattern, data, re.DOTALL):
                return match
        raise AssertionError(f"missing {pattern!r} in {data[-600:]!r}")

    def ready(slave):
        end = time.monotonic() + 12
        while termios.tcgetattr(slave)[3] & termios.ICANON:
            assert time.monotonic() < end, "CLI never entered raw mode"
            time.sleep(.02)

    call("client", "pair", "join", call("host", "pair", "code"))
    pending = wait("host", lambda s: s["pairing"]["incoming"])
    assert not pending["peers"] and pending["pairing"]["incoming"][0]["device_id"] == keys["client"]
    with session() as (child, _, _):
        assert child.wait(14) != 0
    marker = root / "exec-ran"
    with session("exec", keys["host"][:8], "--", "touch", str(marker)) as (child, _, _):
        assert child.wait(14) != 0
        assert not marker.exists()
    with session("exec", "--pipe", "host", "--", "touch", str(marker)) as (child, _, _):
        assert child.wait(14) != 0 and not marker.exists()
    call("host", "pair", "accept", keys["client"])
    wait("client", lambda s: s["pairing"]["message"] == "Paired with host.")
    wait("host", lambda s: s["peer_connections"].get("client"))
    assert call("host", "exec", keys["client"][:8].upper(), "--", "printf", "paired") == "paired"
    # Launch the foreground program directly, including an empty argument and a literal shell metacharacter.
    screen = r"""
import os, sys, time, tty
assert all(os.isatty(n) for n in (0, 1, 2)) and os.environ['TERM'] == 'xterm-256color'
assert os.environ['COLORTERM'] == 'truecolor' and 'NO_COLOR' not in os.environ
assert sys.argv[1:] == ['two words', '', '-x', '$HOME']
assert os.get_terminal_size() == (89, 31)
tty.setraw(0)
os.write(1, b'\x1b[?1049h\x1b[6n')
assert sys.stdin.buffer.read(9) == b'\x1b[5;7R\x1b[A'
end = time.monotonic() + 8
while os.get_terminal_size() != (121, 42):
    assert time.monotonic() < end
    time.sleep(.02)
os.write(2, b'resized\x1b[?1049l')
sys.exit(37)
"""
    with session("exec", "host", "--", sys.executable, "-c", screen, "two words", "", "-x", "$HOME") as (child, fd, slave):
        ready(slave)
        read_until(fd, re.escape(b"\x1b[?1049h\x1b[6n"))
        os.write(fd, b"\x1b[5;7R\x1b[A")
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 42, 121, 0, 0))
        child.send_signal(signal.SIGWINCH)
        read_until(fd, re.escape(b"resized\x1b[?1049l"))
        assert child.wait(12) == 37
    captured = subprocess.run([cli, "--ipc", ipc("client"), "exec", "host", "-c",
                               "test -t 0 && test -t 1 && test -t 2 || exit 2; stty -echo -icanon min 1; "
                               "printf '\\033[6n'; reply=$(dd bs=1 count=6 2>/dev/null); "
                               "test \"$reply\" = \"$(printf '\\033[1;1R')\" || exit 3; "
                               "printf output; printf error >&2; exit 17"],
                              stdin=subprocess.DEVNULL, capture_output=True, timeout=12)
    assert (captured.returncode, captured.stdout, captured.stderr) == (17, b"\x1b[6noutputerror", b""), captured
    with session() as (child, fd, slave):
        ready(slave)
        os.write(fd, b"""paxri status >/dev/null && printf 'TOOLS[%s]\\n' "$PAXRI_CALLER_ID"\r""")
        read_until(fd, re.escape(f"TOOLS[{keys['client']}]".encode()))
        with session("shell", keys["host"][:8]) as (second, second_fd, second_slave):
            ready(second_slave)
            assert call("host", "exec", "client", "--", "printf", "reverse") == "reverse"
            for node, peer in (("client", "host"), ("host", "client")):
                links = json.loads(call(node, "status", peer, "--check", "--json"))["connections"]
                assert len(links) == 1 and links[0]["streams"]["terminal"] == 2, links
            os.write(second_fd, b"exit 13\r")
            assert second.wait(10) == 13
        query = b"""test -t 0 && printf 'PTY[%s]\\n' "$(stty size)"\r"""
        os.write(fd, query)
        read_until(fd, rb"PTY\[31 89\]")
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 42, 121, 0, 0))
        child.send_signal(signal.SIGWINCH)
        os.write(fd, b"""while [ "$(stty size)" != "42 121" ]; do sleep .05; done; printf 'SIZE=%s\\n' "$(stty size)"\r""")
        read_until(fd, b"SIZE=42 121")
        os.write(fd, b"sh -c 'echo starting-$$; exec sleep 60'\r")
        read_until(fd, rb"starting-\d+")
        os.write(fd, b"\x03")
        os.write(fd, b"printf 'signal-%s\\n' returned\r")
        read_until(fd, b"signal-returned")
        os.write(fd, b"printf 'last-%s\\n' output; exit 23\r")
        read_until(fd, b"last-output")
        assert child.wait(12) == 23

    @contextlib.contextmanager
    def managed(action):
        with socket.socket(socket.AF_UNIX) as stream:
            stream.settimeout(6)
            stream.connect(ipc("client"))
            request = json.dumps(dict(version=1, command="managed_terminal", peer="host", action=action)).encode()
            stream.sendall(struct.pack(">I", len(request)) + request)
            with stream.makefile("rb") as reader:
                length, = struct.unpack(">I", reader.read(4))
                result = json.loads(reader.read(length))["result"]
                assert "Ok" in result, result
                yield stream, reader, result["Ok"]

    def directory():
        with managed("List") as (_, _, entries):
            return entries

    size = dict(rows=31, cols=89)
    with session() as (child, fd, slave):
        ready(slave)
        os.write(fd, b"""v=kept; printf 'shell-pid=%s\\n' "$$"\r""")
        pid = int(read_until(fd, rb"shell-pid=(\d+)").group(1))
        key, = directory()
        child.terminate()
        child.wait(10)
    assert Path(f"/proc/{pid}").exists()
    assert directory() == [key]
    with managed({"Attach": [key, size]}) as (stream, reader, _):
        length, = struct.unpack(">I", reader.read(4))
        assert f"shell-pid={pid}".encode() in reader.read(length)
        command = b"""printf 'resumed=%s,%s\\n' "$$" "$v"; sh -c 'echo job-pid=$$; exec sleep 600'\r"""
        stream.sendall(struct.pack(">BH", 1, len(command)) + command)
        output = bytearray()
        while b"job-pid=" not in output or not re.search(rb"job-pid=(\d+)\r?\n", output):
            assert reader.read(1) == b"\x01"
            length, = struct.unpack(">H", reader.read(2))
            output.extend(reader.read(length))
        assert f"resumed={pid},kept".encode() in output
        job = int(re.search(rb"job-pid=(\d+)\r?\n", output).group(1))

    with session("exec", "host", "-c", 'printf "exec-pid=%s\\n" "$$"; exec sleep 60') as (cmd, cmd_fd, _):
        command_pid = int(read_until(cmd_fd, rb"exec-pid=(\d+)").group(1))
        client_process.terminate()
        client_process.wait(10)
        assert cmd.wait(10) != 0

    def wait_gone(pids):
        end = time.monotonic() + 8
        while any(Path(f"/proc/{p}").exists() for p in pids):
            assert time.monotonic() < end, f"process still running: {pids}"
            time.sleep(.05)

    wait_gone([command_pid])
    assert all(Path(f"/proc/{p}").exists() for p in (pid, job))
    start("client")
    call("client", "status", "host", "--check")
    assert directory() == [key]
    with managed({"End": key}):
        pass
    assert directory() == []
    wait_gone([pid, job])
    # Closing the drawing surface and requesting End are different on the wire.
    with managed({"Create": [key, size]}) as (stream, reader, _):
        assert reader.read(4) == bytes(4)
        stream.sendall(struct.pack(">BI", 3, 0))
        while (kind := reader.read(1)) == b"\x01":
            length, = struct.unpack(">H", reader.read(2))
            reader.read(length)
        assert kind == b"\x03" and reader.read(4) == bytes(4)
    assert not directory()
    with managed({"Create": [key, size]}) as (stream, reader, _):
        assert reader.read(4) == bytes(4)
        command = b"sleep 0.3; sleep 5 & exit 17\r"
        stream.sendall(struct.pack(">BH", 1, len(command)) + command)
        # Ensure the host received the command before detaching the socket.
        assert reader.read(1) == b"\x01"
        length, = struct.unpack(">H", reader.read(2))
        pending = reader.read(length)
        while command.rstrip() not in pending:
            assert reader.read(1) == b"\x01"
            length, = struct.unpack(">H", reader.read(2))
            pending += reader.read(length)
    deadline = time.monotonic() + 3
    while directory():
        assert time.monotonic() < deadline, "background job held an exited shell open"
        time.sleep(.05)
    assert host_process.poll() is None
    print("Native PTYs passed detach, replay, shell state, peer reconnect, explicit end and process cleanup checks.")


def check_pipe(cli, ipc):
    cmd = [cli, "--ipc", ipc("client"), "exec", "--pipe", "host"]
    payload = b"\x89PNG\r\n\x1a\n" + bytes(range(256)) * 4097
    errors = bytes(reversed(range(256))) * 351
    program = """
import os, sys
assert sys.argv[1:] == ['two words', '', '--pipe', '$HOME']
assert not any(os.isatty(fd) for fd in range(3))
sys.stderr.buffer.write(bytes(reversed(range(256))) * 351)
sys.stderr.buffer.flush()
sys.stdout.buffer.write(sys.stdin.buffer.read())
sys.exit(37)
"""
    run = subprocess.run([*cmd, "--", sys.executable, "-c", program, "two words", "", "--pipe", "$HOME"],
                         input=payload, capture_output=True, timeout=30)
    assert (run.returncode, run.stdout == payload, run.stderr == errors) == (37, True, True), (
        run.returncode, len(run.stdout), len(run.stderr), run.stderr[:160])
    if os.name == "nt":
        text = "中文测试，繁體"
        script = """using namespace System.Text
param($prefix='ok')
$line = @($input) -join '|'
if ($OutputEncoding.CodePage -ne 65001 -or $OutputEncoding.GetPreamble().Length) { exit 24 }
[Console]::Out.Write($prefix + $line)
[Console]::Error.Write('中文')
exit 23"""
        result = subprocess.run([*cmd, "-c", script], input=text.encode() + b"\r\n",
                                capture_output=True, timeout=20)
        assert (result.returncode, result.stdout, result.stderr) == (
            23, ("ok" + text).encode(), "中文".encode()), result
    script = ("[Console]::Out.Write('out'); [Console]::Error.Write('err'); exit 7"
              if os.name == "nt" else "printf out; printf err >&2; exit 7")
    # A program which ignores stdin must finish even while the calling pipe is open.
    with subprocess.Popen([*cmd, "-c", script], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE) as child:
        assert child.wait(timeout=20) == 7
        assert (child.stdout.read(), child.stderr.read()) == (b"out", b"err")
    # A disconnect after stdin EOF also releases a subprocess started by the command.
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        listener.settimeout(15)
        port = listener.getsockname()[1]
        job = f"import socket,time; s=socket.create_connection(('127.0.0.1',{port})); time.sleep(30)"
        launcher = "import subprocess,sys; subprocess.call([sys.executable,'-c',sys.argv[1]])"
        with subprocess.Popen([*cmd, "--", sys.executable, "-c", launcher, job],
                              stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL) as client:
            try:
                conn, _ = listener.accept()
                with conn:
                    client.kill()
                    client.wait(10)
                    conn.settimeout(5)
                    try:
                        assert conn.recv(1) == b""
                    except ConnectionResetError:
                        pass
            finally:
                if client.poll() is None:
                    client.kill()
    print("PASS: piped binary I/O, argv, stdin EOF, exit status, early exit and disconnect")


def check_session_tools(root, keys, call, cli):
    host_code = call("host", "pair", "code")[:56]
    assert host_code != call("client", "pair", "code")[:56]

    def remote(*args):
        return call("client", "exec", "host", *args)

    assert host_code in remote("--", "paxri", "pair", "code")
    # Install a standalone executable after startup, outside the system PATH.
    directory = root / "tools" / "bin"
    directory.mkdir(parents=True)
    suffix = ".exe" if os.name == "nt" else ""
    shutil.copy2(Path(cli).with_name("paxri" + suffix), directory / ("paxri-test" + suffix))
    assert host_code in remote("--", "paxri-test", "pair", "code")
    assert host_code in call("client", "exec", "--pipe", "host", "--", "paxri-test", "pair", "code")
    caller = "$env:PAXRI_CALLER_ID" if os.name == "nt" else '"$PAXRI_CALLER_ID"'
    output = remote("-c", f"echo {caller}; paxri-test pair code")
    assert keys["client"] in output and host_code in output, output
    print("PASS: bundled CLI, profile extension, host IPC and authenticated caller in remote exec")


def check_pairing(root, keys, call, start, target_port, wait):
    platform = {"darwin": "macos", "win32": "windows", "linux": "linux"}.get(sys.platform, "unknown")
    def status(node):
        return json.loads(call(node, "status", "--json"))

    def rejected(node, *args):
        try:
            call(node, *args)
        except subprocess.CalledProcessError:
            return
        raise AssertionError(f"Expected {args[:2]} to fail")

    def join_and_wait(code):
        call("client", "pair", "join", code)
        pending = wait("host", lambda s: s["pairing"]["incoming"])["pairing"]["incoming"][0]
        assert pending["name"] == "Laptop" and pending["device_id"] == keys["client"]
        assert pending["platform"] == platform

    # The client already knows a different device named Studio.
    spare = call("spare", "identity", str(root / "spare.key"))
    configs = {
        "host": 'version=1\ndevice_name="Studio"\npeers={}\n',
        "client": f'version=1\ndevice_name="Laptop"\n[peers.Studio]\nendpoint_id="{spare}"\n',
        "spare": 'version=1\ndevice_name="Spare"\n',
    }
    for node, text in configs.items():
        (root / (node + ".toml")).write_text(text)
    processes = {node: start(node)[0] for node in configs}
    originals = {node: (root / (node + ".toml")).read_bytes() for node in configs}
    code = call("host", "pair", "code")
    assert code and status("host")["pairing"]["code"] == code
    rejected("host", "pair", "join", code)
    rejected("client", "pair", "join", "incomplete")
    wrong_code = code[:-1] + ("f" if code[-1] != "f" else "0")
    call("client", "pair", "join", wrong_code)
    wait("client", lambda s: not s["pairing"]["outgoing"] and "invalid" in (s["pairing"]["message"] or "").lower())
    join_and_wait(code)
    rejected("host", "pair", "accept", spare)
    assert not status("host")["peers"] and status("client")["peers"] == {"Studio": spare}
    call("host", "pair", "decline", keys["client"])
    wait("client", lambda s: not s["pairing"]["outgoing"] and "declined" in s["pairing"]["message"])
    assert all((root / (node + ".toml")).read_bytes() == text for node, text in originals.items())

    assert call("host", "pair", "code") == code
    join_and_wait(code)
    call("spare", "pair", "join", code)
    wait("host", lambda s: len(s["pairing"]["incoming"]) == 2)
    old_code = code
    config_path = root / "host.toml"
    # A failed write must leave the old code and its pending confirmations intact.
    config_path.write_bytes(originals["host"] + b"# external edit\n")
    rejected("host", "pair", "refresh")
    assert call("host", "pair", "code") == old_code and len(status("host")["pairing"]["incoming"]) == 2
    config_path.write_bytes(originals["host"])
    code = call("host", "pair", "refresh")
    assert old_code != code and not status("host")["pairing"]["incoming"]
    for node in ["client", "spare"]:
        wait(node, lambda s: not s["pairing"]["outgoing"] and "refreshed" in s["pairing"]["message"])
    rejected("host", "pair", "accept", keys["client"])
    join_and_wait(code)
    call("host", "pair", "accept", keys["client"])
    wait("client", lambda s: s["peers"].get("Studio-2") == keys["host"])
    assert status("host")["peers"] == {"Laptop": keys["client"]}
    assert status("host")["peer_platforms"] == {"Laptop": platform}
    # Both gateways learn about the shared session before any explicit check.
    directions = [("client", "Studio-2"), ("host", "Laptop")]
    for node, peer in directions:
        wait(node, lambda s: len(s["peer_connections"].get(peer, [])) == 1)
    announced_at = {node: status(node)["peer_last_status_change_ms"][peer] for node, peer in directions}
    assert "Studio" not in status("client")["peer_last_status_change_ms"]  # An existing idle device has no history.
    barrier = threading.Barrier(2)

    def check_from(direction):
        node, peer = direction
        barrier.wait(timeout=5)
        return json.loads(call(node, "status", peer, "--check", "--json"))

    with ThreadPoolExecutor(2) as workers:
        checked = list(workers.map(check_from, directions))
    assert [r["device_id"] for r in checked] == [keys["host"], keys["client"]]
    connected_at = {node: status(node)["peer_last_status_change_ms"][peer] for node, peer in directions}
    assert connected_at == announced_at
    listing = call("client", "status")
    devices = listing.split("Paired devices (2)\n", 1)[1].split("\nConnections (", 1)[0]
    assert [name for name in devices.split() if name in {"Studio-2", "Studio"}] == ["Studio-2", "Studio"], devices
    host_short = next(word for word in devices.split() if len(word) >= 8 and keys["host"].startswith(word))
    selected = json.loads(call("client", "status", host_short, "--check", "--json"))
    assert (selected["peer"], selected["device_id"]) == ("Studio-2", keys["host"])
    idle = json.loads(call("client", "status", "Studio", "--json"))
    assert idle["state"] == "Offline" and idle["connections"] == [], idle
    for node, peer in directions:
        # QUIC can finish the application exchange before publishing its selected path.
        deadline = time.monotonic() + 5
        while True:
            links = json.loads(call(node, "status", peer, "--json"))["connections"]
            assert len(links) == 1 and links[0]["protocol"] == "Shared connection", links
            if links[0]["path"] != "selecting" or time.monotonic() >= deadline:
                break
            time.sleep(.05)
        assert links[0]["path"] == "direct" and links[0]["address"] and links[0]["rtt_ms"] is not None, links
        traffic = links[0]["traffic"]
        assert all(type(traffic[k]) is int and traffic[k] >= 0
                   for k in ("sent_bytes", "received_bytes", "connected_at_ms")), traffic
        assert "rates" not in links[0], links
        report = call(node, "status", peer)
        assert all(label in report for label in ("SENT (B)", "RECEIVED (B)", "RTT (ms)")), report
    rejected("host", "status", "missing", "--json")
    assert status("client")["peer_platforms"] == {"Studio-2": platform}
    call("spare", "pair", "join", old_code)
    wait("spare", lambda s: not s["pairing"]["outgoing"] and "invalid" in (s["pairing"]["message"] or "").lower())
    assert not status("spare")["peers"]

    call("client", "forward", "add", "echo", "--peer", host_short, "--listen", "0", "--target", str(target_port))
    assert status("client")["forwards"]["echo"]["peer"] == "Studio-2"
    port = int(status("client")["forwards"]["echo"]["bound"].rsplit(":", 1)[1])
    with socket.create_connection(("127.0.0.1", port), timeout=8) as conn, conn.makefile("rb") as received:
        assert received.readline() == b"ready\n"
        conn.sendall(b"paired")
        conn.shutdown(socket.SHUT_WR)
        assert received.read() == b"deriap"

    # Re-pairing the same identities preserves local aliases and rules.
    join_and_wait(call("host", "pair", "code"))
    call("host", "pair", "accept", keys["client"])
    wait("client", lambda s: not s["pairing"]["outgoing"])
    for node, peer in directions:
        assert status(node)["peer_last_status_change_ms"][peer] == connected_at[node]
    for node in ["client", "host"]:
        processes[node].terminate()
        processes[node].wait(timeout=12)
        if node == "client":
            if os.name != "nt":  # SIGTERM allows the client gateway to close QUIC before exiting.
                wait("host", lambda s: not s["peer_connections"]["Laptop"]
                     and s["peer_last_status_change_ms"]["Laptop"] > connected_at["host"])
            # Simulate a pairing saved before platform exchange existed.
            path = root / "client.toml"
            lines = path.read_text().splitlines(keepends=True)
            assert sum(line.startswith("platform =") for line in lines) == 1
            path.write_text("".join(line for line in lines if not line.startswith("platform =")))
        start(node)
        if node == "client":
            wait(node, lambda s: s["peer_platforms"].get("Studio-2") == platform)
            assert f'platform = "{platform}"' in path.read_text()
    assert status("client")["peers"] == {"Studio": spare, "Studio-2": keys["host"]}
    assert status("client")["forwards"]["echo"]["peer"] == "Studio-2"
    assert call("host", "pair", "code") == code
    # Even direct-only devices can use the previously copied code after the issuer restarts.
    call("spare", "pair", "join", code)
    wait("host", lambda s: any(p["device_id"] == spare for p in s["pairing"]["incoming"]))
    call("spare", "pair", "cancel")
    wait("host", lambda s: not s["pairing"]["incoming"])
    assert status("host")["peers"] == {"Laptop": keys["client"]}
    print("Pairing, peer platform exchange and backfill, live forwarding and shared settings passed.")


def check_file_copy(root, cli, ipc, host_short):
    src = root / "source tree"
    (src / "empty").mkdir(parents=True)
    (src / "nested").mkdir()
    data = bytes(range(256)) * 4097
    item = src / "nested" / "你好.bin"
    item.write_bytes(data)
    (src / ".hidden").write_bytes(b"")
    stamp = 1_650_000_123
    os.utime(item, (stamp, stamp))
    if os.name != "nt":
        item.chmod(0o751)
    remote = root / "remote"
    remote.mkdir()
    local = root / "downloads"
    local.mkdir()

    def cp(*args, ok=True):
        result = subprocess.run([cli, "--ipc", ipc("client"), "cp", *map(str, args)],
                                capture_output=True, timeout=30)
        assert (result.returncode == 0) == ok and not result.stdout, (args, result.stderr)
        return result.stderr

    peer = lambda p: f"host:{p}"
    cp(src, peer(remote), ok=False)
    assert not (remote / src.name).exists()
    assert cp("-rqp", src, f"{host_short}:{remote}") == b""
    tree = remote / src.name
    assert (tree / "nested" / item.name).read_bytes() == data
    assert (tree / ".hidden").is_file() and (tree / "empty").is_dir()
    cp("-rp", f"{host_short}:{tree}", local)
    copied = local / src.name / "nested" / item.name
    assert int(copied.stat().st_mtime) == stamp
    if os.name != "nt":
        assert copied.stat().st_mode & 0o777 == 0o751
    assert copied.read_bytes() == data

    copied.write_bytes(b"keep me")
    assert b"Skipped" in cp("-rnv", peer(tree), local)
    assert copied.read_bytes() == b"keep me"
    cp(peer(tree / ".hidden"), peer(tree / "nested" / item.name), local)
    assert (local / item.name).read_bytes() == data
    cp("--", src / ".hidden", item, peer(remote))
    assert (remote / item.name).read_bytes() == data
    missing = remote / "missing"
    for separator in (("/", "\\") if os.name == "nt" else ("/",)):
        err = cp(item, peer(missing) + separator, ok=False)
        assert b"Destination directory does not exist" in err and b"missing" in err, err
    err = cp(item, src / ".hidden", peer(missing), ok=False)
    assert b"Destination directory does not exist" in err, err
    err = cp(peer(tree / "nested" / item.name), str(local / "missing") + "/", ok=False)
    assert b"Destination directory does not exist" in err and b"missing" in err, err
    assert not missing.exists()
    assert cp("-q", peer(missing), copied, ok=False) and copied.read_bytes() == b"keep me"
    print("File copy passed: binary upload/download, recursive trees, metadata, no-clobber and multiple sources.")


def check_lifecycle(client):
    with tempfile.TemporaryDirectory(prefix="p2p-lifecycle-") as tmp:
        root = Path(tmp)
        ipc = root.name if os.name == "nt" else str(root / "run" / "gateway.sock")
        config = str(root / "node profile" / "node.toml")

        def call(*args):
            return subprocess.check_output([client, "--ipc", ipc, *args], stdin=subprocess.DEVNULL,
                                           stderr=subprocess.PIPE, timeout=45, encoding="utf-8")

        def manage(*args):
            return call("gateway", *args, "--config", config)

        try:
            # Captured pipes must reach EOF even while the background child is still running.
            with ThreadPoolExecutor(2) as pool:
                starts = [pool.submit(manage, "start") for _ in range(2)]
                assert all("Gateway running" in start.result() for start in starts)
            initial = json.loads(call("status", "--json"))
            assert initial["user_managed"] and initial["startup"] == "gui"
            assert initial["ipc_path"] == ipc
            assert Path(initial["executable"]).resolve().parent == Path(client).resolve().parent
            # A second installation can recover this address by the profile alone.
            env = dict(os.environ)
            env.pop("PAXRI_IPC", None)
            reused = subprocess.check_output([client, "gateway", "start", "--config", config],
                                             env=env, stderr=subprocess.PIPE, timeout=45, encoding="utf-8")
            assert ipc in reused and "Running executable" in reused
            manage("startup", "manual")
            manage("restart")
            again = json.loads(call("status", "--json"))
            assert again["startup"] == "manual" and again["device_id"] == initial["device_id"]
            assert again["pairing"]["code"] == initial["pairing"]["code"]
            assert "Gateway stopped" in manage("stop")
            assert "Gateway stopped" in manage("status")
            if os.name == "nt":
                try:
                    manage("startup", "login")
                    offline = manage("status")
                    assert "Gateway stopped" in offline and "Windows task [login, enabled]" in offline, offline
                finally:
                    manage("startup", "manual")
        finally:
            manage("stop")
    print("Background gateway lifecycle passed.")


def check_shared_forwards(root, call, start, rule_state, target_port):
    peers = ("first", "second", "unpaired")
    ids = {name: call(name, "identity", str(root / (name + ".key"))) for name in peers}
    address = f"127.0.0.1:{target_port}"
    host_file = root / "host.toml"
    host_file.write_text(
        f'version=1\n[peers.first]\nendpoint_id="{ids["first"]}"\n'
        f'[peers.second]\nendpoint_id="{ids["second"]}"\n')
    _, host = start("host")

    def greeting(bound):
        ip, port = bound.rsplit(":", 1)
        with socket.create_connection((ip, int(port)), timeout=8) as connection:
            try:
                with connection.makefile("rb") as reply:
                    return reply.readline()
            except ConnectionResetError:
                return b""

    listeners = {}
    for name in peers:
        (root / (name + ".toml")).write_text(
            f'version=1\n[peers.host]\nendpoint_id="{host["device_id"]}"\nticket="{host["ticket"]}"\n')
        start(name)
        call(name, "forward", "add", "pull", "--peer", "host", "--listen", "0", "--target", address)
        listeners[name] = rule_state(name, "pull", lambda rule: rule["bound"])["bound"]
    reverse = {}
    for name in peers[:2]:
        assert greeting(listeners[name]) == b"ready\n", name
        call(name, "forward", "add", "push", "--remote", "--peer", "host", "--listen", "0", "--target", address)
        reverse[name] = rule_state(name, "push", lambda rule: rule["bound"])["bound"]
        assert greeting(reverse[name]) == b"ready\n", name
    assert greeting(listeners["unpaired"]) == b"", "Unpaired identity could reach a local target"
    call("unpaired", "forward", "add", "push", "--remote", "--peer", "host", "--listen", "0", "--target", address)
    rule_state("unpaired", "push", lambda rule: not rule["bound"] and
               any(reason in (rule["error"] or "") for reason in ("DENIED", "peer not paired")))

    def directory(node):
        return json.loads(call(node, "forward", "list", "--json"))["peers"]["rules"]

    call("host", "forward", "add", "private", "--peer", "second", "--listen", "0", "--target", address)
    shared = directory("host")
    for peer in peers[:2]:
        assert shared[peer]["pull"]["direction"] == "remote"
        assert shared[peer]["push"]["direction"] == "local"
        assert shared[peer]["push"]["bound"] == reverse[peer]
    assert set(directory("first")["host"]) == set(), "Third-party rule was disclosed"
    try:
        call("first", "forward", "remove", "private", "--owner", "host")
        raise AssertionError("Unrelated peer modified a rule")
    except subprocess.CalledProcessError as error:
        assert "Unknown forwarding rule for this pairing" in error.stderr, error.stderr
    assert "private" in directory("second")["host"]
    call("host", "forward", "pause", "pull", "--owner", "first")
    assert not rule_state("first", "pull", lambda r: not r["bound"])["enabled"]
    assert directory("host")["first"]["pull"]["enabled"] is False
    assert greeting(listeners["second"]) == b"ready\n"
    call("host", "forward", "resume", "pull", "--owner", "first")
    resumed_pull = rule_state("first", "pull", lambda r: r["bound"])["bound"]
    assert greeting(resumed_pull) == b"ready\n"
    # Rebind a local rule to a different interface on the same port and rename it locally.
    pull_port = resumed_pull.rsplit(":", 1)[1]
    call("first", "forward", "edit", "pull", "--peer", "host", "--listen", f"0.0.0.0:{pull_port}", "--target", address)
    assert rule_state("first", "pull", lambda r: r["bound"] == f"0.0.0.0:{pull_port}")["bound"] == f"0.0.0.0:{pull_port}"
    assert greeting(f"127.0.0.1:{pull_port}") == b"ready\n"
    call("first", "forward", "edit", "pull", "--rename", "pull-renamed", "--peer", "host", "--listen", f"0.0.0.0:{pull_port}", "--target", address)
    assert "pull" not in json.loads(call("first", "status", "--json"))["forwards"]
    assert rule_state("first", "pull-renamed", lambda r: r["bound"] == f"0.0.0.0:{pull_port}")["bound"] == f"0.0.0.0:{pull_port}"
    assert greeting(f"127.0.0.1:{pull_port}") == b"ready\n"
    # Rename and flip direction from the other endpoint, keeping its view of the two addresses.
    call("host", "forward", "edit", "pull-renamed", "--rename", "pull", "--owner", "first", "--listen", "0", "--target", address)
    edited = rule_state("first", "pull", lambda r: r["direction"] == "remote" and r["bound"])
    assert "pull-renamed" not in directory("host")["first"]
    assert directory("host")["first"]["pull"]["direction"] == "local"
    assert greeting(edited["bound"]) == b"ready\n"
    call("host", "forward", "remove", "pull", "--owner", "first")
    assert "pull" not in directory("host")["first"]
    assert "pull" in json.loads(call("second", "status", "--json"))["forwards"]
    print("PASS: forward and reverse TCP, same-name rules on different owners, bilateral list/edit/rename/pause/resume/remove, same-port rebind, third-party and unpaired isolation")


def check_unpair(root, keys, call, start, wait, echo_port):
    address = f"127.0.0.1:{echo_port}"
    spare = call("spare", "identity", str(root / "spare.key"))
    (root / "host.toml").write_text(
        f'version=1\ndevice_name="Host"\n[peers.client]\nendpoint_id="{keys["client"]}"\n')
    host_process, host = start("host")
    peer = f'endpoint_id="{keys["host"]}"\nticket="{host["ticket"]}"\n'
    (root / "client.toml").write_text(
        f'version=1\ndevice_name="Client"\n[peers.host]\n{peer}[peers.other-name]\n{peer}'
        f'[peers.spare]\nendpoint_id="{spare}"\n'
        f'[forwards.pull]\npeer="host"\nlisten="127.0.0.1:0"\ntarget="{address}"\n'
        f'[forwards.push]\npeer="other-name"\ndirection="remote"\nlisten="127.0.0.1:0"\ntarget="{address}"\n')
    client, _ = start("client")
    rules = wait("client", lambda s: all(r["bound"] for r in s["forwards"].values()))["forwards"]
    addresses = [(ip, int(port)) for ip, port in (r["bound"].rsplit(":", 1) for r in rules.values())]
    with contextlib.ExitStack() as stack:
        flows = [stack.enter_context(socket.create_connection(addr, timeout=8)) for addr in addresses]
        for flow in flows:
            flow.sendall(b"x")
            assert flow.recv(1) == b"x"
        assert "both devices" in call("client", "pair", "remove", "other-name")
        for flow in flows:
            try:
                assert flow.recv(1) == b"", "An unpaired connection still carries data"
            except ConnectionResetError:
                pass
    def cleaned(state):
        assert state["peers"] == {"spare": spare} and state["forwards"] == {}
    cleaned(json.loads(call("client", "status", "--json")))
    for addr in addresses:
        with socket.socket() as probe:
            probe.settimeout(2)
            assert probe.connect_ex(addr) != 0, "Forward still listening"
    client.terminate()
    client.wait(timeout=10)
    client, restarted = start("client")
    cleaned(restarted)
    remote = json.loads(call("host", "status", "--json"))
    assert not remote["peers"]
    code = remote["pairing"]["code"]

    def pair_again():
        call("client", "pair", "join", code)
        wait("host", lambda s: s["pairing"]["incoming"])
        call("host", "pair", "accept", keys["client"])
        wait("client", lambda s: s["peers"].get("Host") == keys["host"])

    pair_again()
    host_process.terminate()
    host_process.wait(timeout=10)
    # A cold client knows the peer is offline without waiting for QUIC idle expiry.
    client.terminate()
    client.wait(timeout=10)
    client, _ = start("client")
    assert "locally" in call("client", "pair", "remove", keys["host"][:8])
    client.terminate()
    client.wait(timeout=10)
    client, state = start("client")
    assert state["pairing"]["pending_unpairs"] == ["Host"] and "Host" not in state["peers"]
    # Keep the old notice to simulate a late retry after another approved pairing.
    saved_notice = (root / "client.toml").read_bytes()
    start("host")

    def wait_confirmation():
        deadline = time.monotonic() + 70
        while time.monotonic() < deadline:
            if not json.loads(call("client", "status", "--json"))["pairing"].get("pending_unpairs"):
                return
            time.sleep(.4)
        raise AssertionError("Peer did not confirm the saved unpair notice")

    wait_confirmation()
    assert not json.loads(call("host", "status", "--json"))["peers"]
    pair_again()
    client.terminate()
    client.wait(timeout=10)
    (root / "client.toml").write_bytes(saved_notice)
    start("client")
    wait_confirmation()
    assert json.loads(call("host", "status", "--json"))["peers"] == {"Client": keys["client"]}
    # This peer is offline and has no ticket.
    call("client", "pair", "remove", spare[:8])
    status = json.loads(call("client", "status", "--json"))
    assert not status["peers"]
    print("PASS: unpair closes live TCP in both directions, releases listeners, removes all identity aliases, "
          "syncs both gateways, retries offline across restart, preserves a newer pairing and accepts a short ID")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, default=Path("target/debug"))
    parser.add_argument("--client", choices=("paxri", "paxri-gui"), default="paxri",
                        help="client executable to use for gateway commands")
    parser.add_argument("--unpair", action="store_true", help="revoke an online or offline device and its active forwards")
    parser.add_argument("--pairing-only", action="store_true", help="exercise persistent pairing codes, rotation and bilateral approval")
    parser.add_argument("--native-terminal", action="store_true", help="exercise a native PTY between two local Linux gateways")
    parser.add_argument("--session-tools", action="store_true", help="check the tool environment between two unprivileged gateways")
    parser.add_argument("--pipe-exec", action="store_true", help="check binary command I/O between two unprivileged gateways")
    parser.add_argument("--file-copy", action="store_true", help="copy files and directories between two isolated gateways")
    parser.add_argument("--forwarding", action="store_true", help="exercise shared forwarding and management from both endpoints")
    parser.add_argument("--lifecycle", action="store_true", help="exercise background launch, restart and stop")
    args = parser.parse_args()
    if args.native_terminal and (not sys.platform.startswith("linux") or os.geteuid() == 0):
        parser.error("--native-terminal requires a standard Linux user")
    bins = args.bin_dir.resolve()
    suffix = ".exe" if os.name == "nt" else ""
    cli, gateway = (str(bins / (n + suffix)) for n in (args.client, "paxri-gateway"))

    if args.lifecycle:
        return check_lifecycle(cli)

    with tempfile.TemporaryDirectory() as tmp, contextlib.ExitStack() as stack:
        root = Path(tmp)
        target = stack.enter_context(TargetServer(("127.0.0.1", 0), Target))
        threading.Thread(target=target.serve_forever, daemon=True).start()
        stack.callback(target.shutdown)
        finish_first = stack.enter_context(TargetServer(("127.0.0.1", 0), FinishFirst))
        finish_first.received = threading.Event()
        threading.Thread(target=finish_first.serve_forever, daemon=True).start()
        stack.callback(finish_first.shutdown)
        streaming = stack.enter_context(TargetServer(("127.0.0.1", 0), StreamingEcho))
        threading.Thread(target=streaming.serve_forever, daemon=True).start()
        stack.callback(streaming.shutdown)

        def ipc(node):
            return f"test-{root.name}-{node}" if os.name == "nt" else str(root / (node + ".sock"))

        def call(node, *args):
            return subprocess.check_output([cli, "--ipc", ipc(node), *args], encoding="utf-8", stderr=subprocess.PIPE).strip()

        def wait(node, condition):
            deadline = time.monotonic() + 14
            while time.monotonic() < deadline:
                state = json.loads(call(node, "status", "--json"))
                if condition(state):
                    return state
                time.sleep(.08)
            raise AssertionError(f"Timed out waiting for pairing on {node}")

        def rule_state(node, name, match):
            end = time.monotonic() + 25
            while time.monotonic() < end:
                rule = json.loads(call(node, "status", "--json"))["forwards"][name]
                if match(rule):
                    return rule
                time.sleep(.1)
            raise AssertionError(f"Forward {name} did not reach expected state: {rule}")

        keys = {node: call(node, "identity", str(root / (node + ".key"))) for node in ("client", "host")}

        def start(node):
            log = stack.enter_context((root / (node + ".log")).open("a+", encoding="utf-8"))
            env = None
            if args.native_terminal:
                env = {**os.environ, "SHELL": "/bin/sh", "NO_COLOR": "1", "COLORTERM": ""}
            process = subprocess.Popen([gateway, "--no-relay", "--config", str(root / (node + ".toml")),
                                        "--key", str(root / (node + ".key")), "--ipc", ipc(node)], stderr=log, env=env)

            def stop():
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=12)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()

            stack.callback(stop)
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    log.seek(0)
                    raise RuntimeError(log.read())
                try:
                    return process, json.loads(call(node, "status", "--json"))
                except subprocess.CalledProcessError:
                    time.sleep(0.1)
            raise RuntimeError(f"{node} IPC did not become ready")

        if args.unpair:
            return check_unpair(root, keys, call, start, wait, streaming.server_address[1])
        if args.forwarding:
            return check_shared_forwards(root, call, start, rule_state, target.server_address[1])
        if args.native_terminal:
            check_native_terminal(root, keys, call, start, cli, ipc, wait)
            return
        if args.pairing_only:
            check_pairing(root, keys, call, start, target.server_address[1], wait)
            return

        (root / "host.toml").write_text(f'''version=1
[peers.client]
endpoint_id="{keys['client']}"
''')
        _, host = start("host")
        assert host["device_id"] == keys["host"]
        ticket = call("host", "ticket")
        with socket.socket() as reserve:
            reserve.bind(("127.0.0.1", 0))
            listen_port = reserve.getsockname()[1]
        client_config = f'''version=1
[peers.host]
endpoint_id="{keys['host']}"
ticket="{ticket}"
'''
        (root / "client.toml").write_text(client_config)
        direct, status = start("client")
        assert not status["forwards"] and not host["forwards"]
        if args.session_tools:
            wait("client", lambda s: s["peer_connections"].get("host"))
            return check_session_tools(root, keys, call, cli)
        if args.pipe_exec:
            wait("client", lambda s: s["peer_connections"].get("host"))
            return check_pipe(cli, ipc)
        if args.file_copy:
            return check_file_copy(root, cli, ipc, keys["host"][:8])
        command = [cli, "--ipc", ipc("client"), "connect", "host"]
        payload = bytes(range(256)) * 1024
        echo = subprocess.run([cli, "--ipc", ipc("client"), "connect", keys["host"][:8], str(target.server_address[1])],
                              input=payload, capture_output=True, timeout=15)
        assert echo.returncode == 0 and echo.stdout == b"ready\n" + payload[::-1], echo.stderr
        duplex = subprocess.run([*command, str(streaming.server_address[1])], input=payload, capture_output=True, timeout=15)
        assert duplex.returncode == 0 and duplex.stdout == payload, duplex.stderr
        asyncio.run(check_remote_eof([*command, str(finish_first.server_address[1])]))
        assert finish_first.received.wait(5)
        with socket.socket() as unused:
            unused.bind(("127.0.0.1", 0))
            refused_port = str(unused.getsockname()[1])
        denial = subprocess.run([*command, refused_port], input=b"", capture_output=True, timeout=10)
        assert denial.returncode != 0 and not denial.stdout and b"TARGET_REFUSED" in denial.stderr
        direct.terminate()
        direct.wait(timeout=12)
        print("PASS: stdio TCP access with no forwards, binary data, EOF in both orders and target refusal")
        (root / "client.toml").write_text(client_config + f'''[forwards.test]
listen="127.0.0.1:{listen_port}"
peer="host"
target="{target.server_address[1]}"
[forwards.refused]
listen="127.0.0.1:0"
peer="host"
target="{refused_port}"
''')
        process, status = start("client")

        def connect(state, rule):
            host, port = state["forwards"][rule]["bound"].rsplit(":", 1)
            return socket.create_connection((host, int(port)), timeout=8)

        with connect(status, "test") as conn, conn.makefile("rb") as replies:
            assert replies.readline() == b"ready\n"  # target speaks first
            call("client", "forward", "pause", "test")
            assert json.loads(call("client", "status", "--json"))["forwards"]["test"]["bound"] is None
            try:
                with connect(status, "test"):
                    raise AssertionError("paused listener accepted a connection")
            except ConnectionRefusedError:
                pass
            conn.sendall(payload)
            conn.shutdown(socket.SHUT_WR)
            assert replies.read() == payload[::-1]  # half-close, data survives pause
        with connect(status, "refused") as conn:
            assert conn.recv(64) == b""
        deadline = time.monotonic() + 3
        while "TARGET_REFUSED" not in (root / "client.log").read_text(encoding="utf-8"):
            assert time.monotonic() < deadline, "missing target refusal for local forward"
            time.sleep(.02)
        with socket.socket() as blocker:
            blocker.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            blocker.bind(("127.0.0.1", listen_port))
            blocker.listen()
            try:
                call("client", "forward", "resume", "test")
                raise AssertionError("port collision was not reported")
            except subprocess.CalledProcessError as error:
                assert "bind forward" in error.stderr
            assert json.loads(call("client", "status", "--json"))["forwards"]["test"]["bound"] is None
        call("client", "forward", "resume", "test")
        resumed = json.loads(call("client", "status", "--json"))
        with connect(resumed, "test") as conn, conn.makefile("rb") as replies:
            assert replies.readline() == b"ready\n"

        process.kill()  # stale socket recovery after a crash, same key and rules
        process.wait(timeout=10)
        with socket.socket() as blocker:
            blocker.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            blocker.bind(("127.0.0.1", listen_port))
            blocker.listen()
            _, restarted = start("client")
            assert restarted["device_id"] == keys["client"]
            assert restarted["forwards"]["test"]["bound"] is None
            assert "bind forward" in (restarted["forwards"]["test"]["error"] or "")
        call("client", "forward", "resume", "test")
        restarted = json.loads(call("client", "status", "--json"))
        with connect(restarted, "test") as conn, conn.makefile("rb") as replies:
            assert replies.readline() == b"ready\n"
        print("PASS: paired TCP forwarding, server greeting, 256 KiB after half-close, "
              "pause/resume, bind conflict, target refusal and gateway crash recovery")

        # Both directions work with just the pairing.
        with TargetServer(("127.0.0.1", 0), Target) as extra:
            threading.Thread(target=extra.serve_forever, daemon=True).start()
            target_port = str(extra.server_address[1])
            call("client", "forward", "add", "pull", "--peer", "host", "--listen", "0", "--target", target_port)
            state = json.loads(call("client", "status", "--json"))
            call("client", "forward", "add", "push", "--remote", "--peer", "host", "--listen", "0", "--target", target_port)
            reverse = rule_state("client", "push", lambda f: f["bound"])
            assert reverse["direction"] == "remote" and reverse["bound"].startswith("127.0.0.1:")
            with connect(state, "pull") as conn, conn.makefile("rb") as replies:
                assert replies.readline() == b"ready\n"
                conn.sendall(payload)
                conn.shutdown(socket.SHUT_WR)
                assert replies.read() == payload[::-1]
            remote_state = {"forwards": {"push": reverse}}
            with connect(remote_state, "push") as first, first.makefile("rb") as replies:
                assert replies.readline() == b"ready\n"  # server speaks before the peer TCP client
                with connect(remote_state, "push") as second, second.makefile("rb") as other:
                    assert other.readline() == b"ready\n"
                    second.sendall(payload)
                    second.shutdown(socket.SHUT_WR)
                    assert other.read() == payload[::-1]
                # Scope incoming sockets to the right listener even while another rule is active.
                call("client", "forward", "add", "sibling", "--remote", "--peer", "host", "--listen", "0",
                     "--target", str(streaming.server_address[1]))
                sibling = rule_state("client", "sibling", lambda f: f["bound"])
                with connect({"forwards": {"sibling": sibling}}, "sibling") as other, other.makefile("rb") as echo:
                    other.sendall(b"sibling")
                    other.shutdown(socket.SHUT_WR)
                    assert echo.read() == b"sibling"
                links = json.loads(call("client", "status", "host", "--json"))["connections"]
                assert len(links) == 1 and links[0]["streams"]["listen"] == 2, links
                call("client", "forward", "remove", "sibling")
                call("client", "forward", "pause", "push")
                try:
                    with connect(remote_state, "push"):
                        raise AssertionError("paused remote listener accepted a connection")
                except ConnectionRefusedError:
                    pass
                first.sendall(b"still open")
                first.shutdown(socket.SHUT_WR)
                assert replies.read() == b"nepo llits"
            call("client", "forward", "resume", "push")
            rule_state("client", "push", lambda f: f["bound"])
            call("client", "forward", "remove", "push")
            call("client", "forward", "remove", "pull")
            extra.shutdown()
        print("PASS: local and remote TCP listeners, concurrent streams and half-close through pause")


if __name__ == "__main__":
    main()
