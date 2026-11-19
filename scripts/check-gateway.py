#!/usr/bin/env python3
"""Exercise the installed CLI and two real gateways without any public infrastructure."""
import argparse
import asyncio
import contextlib
import json
import os
from pathlib import Path
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


def check_native_terminal(root, keys, call, start, cli, ipc):
    import errno
    import fcntl
    import re
    import select
    import signal
    import struct
    import termios

    # An existing TCP port grant does not confer native shell access.
    (root / "host.toml").write_text(
        f'version=1\n[peers.client]\nendpoint_id="{keys["client"]}"\nports.connect=["127.0.0.1:22"]\n')
    host_process, host = start("host")
    (root / "client.toml").write_text(
        f'version=1\n[peers.host]\nendpoint_id="{keys["host"]}"\nticket="{host["ticket"]}"\n')
    start("client")
    assert not host["terminal_access"] and "denied" in call("host", "terminal-access", "show", "client")

    @contextlib.contextmanager
    def session():
        master, slave = os.openpty()
        mode = termios.tcgetattr(slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 31, 89, 0, 0))
        child = subprocess.Popen([cli, "--ipc", ipc("client"), "shell", "host"],
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

    with session() as (child, fd, _):
        assert child.wait(14) != 0
        read_until(fd, b"Terminal access denied")
    call("host", "terminal-access", "allow", "client")
    assert "client" in json.loads(call("host", "status", "--json"))["terminal_access"]
    with session() as (child, fd, slave):
        ready(slave)
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

    with session() as (child, fd, slave):
        ready(slave)
        os.write(fd, b"""printf 'shell-pid=%s\\n' "$$"; sh -c 'printf "job-pid=%s\\n" "$$"; exec sleep 60'\r""")
        match = read_until(fd, rb"shell-pid=(\d+).*?job-pid=(\d+)")
        pids = [int(p) for p in match.groups()]
        call("host", "terminal-access", "deny", "client")
        assert child.wait(10) != 0
        end = time.monotonic() + 5
        while any(Path(f"/proc/{pid}").exists() for pid in pids):
            assert time.monotonic() < end, "revocation left a shell or foreground job running"
            time.sleep(.05)
    assert host_process.poll() is None
    print("Native PTY denial, grant, real shell, resize, final output, exit status and live revocation passed.")


def check_pairing(root, keys, call, start, target_port):
    platform = {"darwin": "macos", "win32": "windows", "linux": "linux"}.get(sys.platform, "unknown")
    def status(node):
        return json.loads(call(node, "status", "--json"))

    def wait(node, condition):
        deadline = time.monotonic() + 14
        while time.monotonic() < deadline:
            state = status(node)
            if condition(state):
                return state
            time.sleep(.08)
        raise AssertionError(f"Timed out waiting for pairing on {node}")

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
    assert status("client")["peer_platforms"] == {"Studio-2": platform}
    assert not status("host")["port_access"] and not status("client")["port_access"]
    call("spare", "pair", "join", old_code)
    wait("spare", lambda s: not s["pairing"]["outgoing"] and "invalid" in (s["pairing"]["message"] or "").lower())
    assert not status("spare")["peers"]

    call("host", "port-access", "set", "Laptop", "--connect", str(target_port))
    call("client", "forward", "add", "echo", "--peer", "Studio-2", "--listen", "0", "--target", str(target_port))
    port = int(status("client")["forwards"]["echo"]["bound"].rsplit(":", 1)[1])
    with socket.create_connection(("127.0.0.1", port), timeout=8) as conn, conn.makefile("rb") as received:
        assert received.readline() == b"ready\n"
        conn.sendall(b"paired")
        conn.shutdown(socket.SHUT_WR)
        assert received.read() == b"deriap"

    # Re-pairing the same identities preserves local aliases, ACLs and rules.
    join_and_wait(call("host", "pair", "code"))
    call("host", "pair", "accept", keys["client"])
    wait("client", lambda s: not s["pairing"]["outgoing"])
    for node in ["client", "host"]:
        processes[node].terminate()
        processes[node].wait(timeout=12)
        if node == "client":
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
    assert status("host")["port_access"]["Laptop"]["connect"] == [f"127.0.0.1:{target_port}"]
    assert call("host", "pair", "code") == code
    # Even direct-only devices can use the previously copied code after the issuer restarts.
    call("spare", "pair", "join", code)
    wait("host", lambda s: any(p["device_id"] == spare for p in s["pairing"]["incoming"]))
    call("spare", "pair", "cancel")
    wait("host", lambda s: not s["pairing"]["incoming"])
    assert status("host")["peers"] == {"Laptop": keys["client"]}
    print("Pairing, peer platform exchange and backfill, live forwarding and shared settings passed.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, default=Path("target/debug"))
    parser.add_argument("--pairing-only", action="store_true", help="exercise persistent pairing codes, rotation and bilateral approval")
    parser.add_argument("--native-terminal", action="store_true", help="exercise a native PTY between two local Linux gateways")
    args = parser.parse_args()
    if args.native_terminal and (not sys.platform.startswith("linux") or os.geteuid() == 0):
        parser.error("--native-terminal requires a standard Linux user")
    bins = args.bin_dir.resolve()
    suffix = ".exe" if os.name == "nt" else ""
    cli, gateway = (str(bins / (n + suffix)) for n in ("tunnel", "tunnel-gateway"))

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
            process = subprocess.Popen([gateway, "--no-relay", "--config", str(root / (node + ".toml")),
                                        "--key", str(root / (node + ".key")), "--ipc", ipc(node)], stderr=log,
                                       env={**os.environ, "SHELL": "/bin/sh"} if args.native_terminal else None)

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

        if args.native_terminal:
            check_native_terminal(root, keys, call, start, cli, ipc)
            return
        if args.pairing_only:
            check_pairing(root, keys, call, start, target.server_address[1])
            return

        (root / "host.toml").write_text(f'''version=1
[peers.client]
endpoint_id="{keys['client']}"
[services.echo]
target="127.0.0.1:{target.server_address[1]}"
allow_peers=["client"]
[services.private]
target="127.0.0.1:{target.server_address[1]}"
allow_peers=[]
[services.finish-first]
target="127.0.0.1:{finish_first.server_address[1]}"
allow_peers=["client"]
[services.streaming]
target="127.0.0.1:{streaming.server_address[1]}"
allow_peers=["client"]
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
        command = [cli, "--ipc", ipc("client"), "connect", "host"]
        payload = bytes(range(256)) * 1024
        echo = subprocess.run([*command, "echo"], input=payload, capture_output=True, timeout=15)
        assert echo.returncode == 0 and echo.stdout == b"ready\n" + payload[::-1], echo.stderr
        duplex = subprocess.run([*command, "streaming"], input=payload, capture_output=True, timeout=15)
        assert duplex.returncode == 0 and duplex.stdout == payload, duplex.stderr
        asyncio.run(check_remote_eof([*command, "finish-first"]))
        assert finish_first.received.wait(5)
        denial = subprocess.run([*command, "private"], input=b"", capture_output=True, timeout=10)
        assert denial.returncode != 0 and not denial.stdout and b"DENIED" in denial.stderr
        direct.terminate()
        direct.wait(timeout=12)
        print("PASS: stdio service access with no forwards, binary data, EOF in both orders and access denial")
        (root / "client.toml").write_text(client_config + f'''[forwards.test]
listen="127.0.0.1:{listen_port}"
peer="host"
service="echo"
[forwards.denied]
listen="127.0.0.1:0"
peer="host"
service="private"
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
        with connect(status, "denied") as conn:
            assert conn.recv(64) == b""  # denied without contacting the target
        assert "DENIED" in (root / "client.log").read_text(encoding="utf-8")
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
        _, restarted = start("client")
        assert restarted["device_id"] == keys["client"]
        with connect(restarted, "test") as conn, conn.makefile("rb") as replies:
            assert replies.readline() == b"ready\n"
        print("PASS: paired TCP forwarding, server greeting, 256 KiB after half-close, "
              "pause/resume, bind conflict, service denial and gateway crash recovery")

        # A TCP port with no published service requires its own explicit grant.
        with TargetServer(("127.0.0.1", 0), Target) as extra:
            threading.Thread(target=extra.serve_forever, daemon=True).start()
            target_port = str(extra.server_address[1])
            call("client", "forward", "add", "pull", "--peer", "host", "--listen", "0", "--target", target_port)
            state = json.loads(call("client", "status", "--json"))
            with connect(state, "pull") as conn:
                assert conn.recv(1) == b""
            call("client", "forward", "add", "push", "--remote", "--peer", "host", "--listen", "0", "--target", target_port)
            rule_state("client", "push", lambda f: "DENIED" in (f["error"] or ""))
            call("host", "port-access", "set", "client", "--listen", "0", "--connect", target_port)
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
            call("host", "port-access", "set", "client")
            rule_state("client", "push", lambda f: "DENIED" in (f["error"] or ""))
            call("client", "forward", "remove", "push")
            call("client", "forward", "remove", "pull")
            extra.shutdown()
        print("PASS: local and remote TCP listeners, live grants and revocation, retry, concurrent streams and half-close through pause")


if __name__ == "__main__":
    main()
