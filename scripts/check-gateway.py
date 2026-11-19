#!/usr/bin/env python3
"""Exercise the installed CLI and two real gateways without any public infrastructure."""
import argparse
import asyncio
import contextlib
import getpass
import json
import os
from pathlib import Path
import socket
import socketserver
import subprocess
import shlex
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


def configure_sshd(root, terminal=False):
    for key in ("host-key", "user-key"):
        subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(root/key)], check=True)
    (root/"known_hosts").write_text("p2p-test " + (root/"host-key.pub").read_text())
    (root/"sshd_config").write_text(
        f'HostKey "{root/"host-key"}"\nAuthorizedKeysFile "{root/"user-key.pub"}"\n'
        'AuthenticationMethods publickey\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n'
        'UsePAM no\nPermitRootLogin prohibit-password\nPermitUserRC no\nDisableForwarding yes\n'
        + ('PermitTTY yes\nForceCommand if test -t 0; then stty -echo; stty size; fi; exec cat\n'
           if terminal else 'PermitTTY no\nForceCommand cat\n'))


def check_ssh_terminal(command):
    import fcntl
    import select
    import struct
    import termios

    with contextlib.ExitStack() as stack:
        master, slave = os.openpty()
        stack.callback(os.close, master)
        stack.callback(os.close, slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 31, 89, 0, 0))
        child = subprocess.Popen(command, stdin=slave, stdout=slave, stderr=slave)
        try:
            def read_until(marker):
                data = bytearray()
                deadline = time.monotonic() + 12
                while marker not in data:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0 or not select.select([master], [], [], remaining)[0]:
                        raise AssertionError(f"Expected {marker!r} on SSH terminal, got {data[-500:]!r}")
                    data.extend(os.read(master, 4096))
            read_until(b"31 89")
            os.write(master, b"remote-pty-input\n")
            read_until(b"remote-pty-input")
            os.write(master, b"\x04")
            assert child.wait(timeout=8) == 0
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, default=Path("target/debug"))
    parser.add_argument("--ssh-stdio", action="store_true", help="also exercise Linux OpenSSH in inetd mode")
    args = parser.parse_args()
    if args.ssh_stdio and not sys.platform.startswith("linux"):
        parser.error("--ssh-stdio requires Linux")
    bins = args.bin_dir.resolve()
    suffix = ".exe" if os.name == "nt" else ""
    cli, gateway = (str(bins / (n + suffix)) for n in ("tunnel", "tunnel-gateway"))

    # OpenSSH checks ownership and modes up to the home directory for authorized_keys.
    with tempfile.TemporaryDirectory(dir=Path.home() if args.ssh_stdio else None) as tmp, contextlib.ExitStack() as stack:
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
                                        "--key", str(root / (node + ".key")), "--ipc", ipc(node)], stderr=log)

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
        if args.ssh_stdio:
            configure_sshd(root, terminal=True)
            with (root / "host.toml").open("a") as cfg:
                cfg.write(f'\n[services.ssh]\ntarget={{sshd_config={json.dumps(str(root/"sshd_config"))}}}\nallow_peers=["client"]\n')
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
        if args.ssh_stdio:
            proxy = shlex.join([*command, "ssh"])
            session = subprocess.run([
                "ssh", "-F", "/dev/null", "-T", "-i", str(root/"user-key"),
                "-o", "IdentitiesOnly=yes", "-o", "IdentityAgent=none", "-o", "BatchMode=yes",
                "-o", "StrictHostKeyChecking=yes", "-o", "GlobalKnownHostsFile=/dev/null",
                "-o", f"UserKnownHostsFile={root/'known_hosts'}", "-o", f"ProxyCommand={proxy}",
                "-l", getpass.getuser(), "p2p-test", "cat",
            ], input=payload, capture_output=True, timeout=20)
            assert session.returncode == 0 and session.stdout == payload, session.stderr
            client_config_path = root / "OpenSSH client.conf"
            client_config_path.write_text(
                'Host host\n    HostKeyAlias p2p-test\n    IdentitiesOnly yes\n'
                '    IdentityAgent none\n    BatchMode yes\n    StrictHostKeyChecking yes\n'
                f'    UserKnownHostsFile "{root/"known_hosts"}"\n')
            ssh_command = [
                cli, "--ipc", ipc("client"), "ssh", f"{getpass.getuser()}@host",
                "-i", str(root/"user-key"), "-F", str(client_config_path),
            ]
            terminal = subprocess.run(ssh_command, input=payload, capture_output=True, timeout=20)
            assert terminal.returncode == 0 and terminal.stdout == payload, terminal.stderr
            check_ssh_terminal(ssh_command)
            call("client", "ssh-config", "set", "host", "--user", getpass.getuser(),
                 "--identity", str(root/"user-key"), "--service", "ssh")
            # The GUI calls this entry point with no application prompts or per-session flags.
            saved = [cli, "--ipc", ipc("client"), "ssh", "host", "-F", str(client_config_path)]
            check_ssh_terminal(saved)
            refused = subprocess.run([*saved, "--service", "private"], input=b"", capture_output=True, timeout=15)
            assert refused.returncode and b"DENIED" in refused.stderr
            call("client", "ssh-config", "reset", "host")
            print("PASS: OpenSSH auth, 256 KiB round trip, interactive remote PTY with input and window size, no SSH TCP listener")
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
