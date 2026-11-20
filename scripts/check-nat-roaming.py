"""Exercise a running gateway while its physical interface and NAT-PMP router change.

Called inside the disposable namespaces created by check-nat.sh --roam.
"""
import base64
from contextlib import ExitStack
import json
from pathlib import Path
import selectors
import socket
import socketserver
import struct
import subprocess
import sys
import time


def router(lan, wan, external):
    client = lan.rsplit(".", 1)[0] + ".2"
    external = int(external)

    def nft(rules):
        subprocess.run(["nft", "-f", "-"], input=rules, text=True, check=True)

    nft("add table ip lease\n"
        "add chain ip lease incoming { type nat hook prerouting priority -101; }\n"
        "add chain ip lease outgoing { type nat hook postrouting priority 99; }\n"
        f'insert rule ip lab forward_traffic iifname "external" ip daddr {client} ct status dnat accept\n')
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.bind((lan, 5351))
        print("ready", flush=True)
        started = time.monotonic()
        while True:
            packet, source = sock.recvfrom(32)
            if source[0] != client or len(packet) < 2 or packet[0] != 0:
                continue
            opcode = packet[1]
            if opcode == 0:
                body = socket.inet_aton(wan)
            elif opcode == 1 and len(packet) == 12:
                port, _, lifetime = struct.unpack("!HHI", packet[4:])
                rules = "flush chain ip lease incoming\nflush chain ip lease outgoing\n"
                if lifetime:
                    rules += (f'add rule ip lease incoming iifname "external" ip daddr {wan} '
                              f'udp dport {external} dnat to {client}:{port}\n'
                              f'add rule ip lease outgoing oifname "external" ip saddr {client} '
                              f'udp sport {port} snat to {wan}:{external}\n')
                nft(rules)
                body = struct.pack("!HHI", port, external, min(lifetime, 120))
            else:
                continue
            header = struct.pack("!BBHI", 0, opcode + 128, 0, int(time.monotonic() - started))
            sock.sendto(header + body, source)


class Echo(socketserver.StreamRequestHandler):
    def handle(self):
        for line in self.rfile:
            self.wfile.write(line)


def run(peerhatch):
    script = Path(__file__).resolve()
    gateway = Path(peerhatch).with_name("peerhatch-gateway")
    a = [peerhatch, "--ipc", str(Path("a.sock").resolve())]
    b = [peerhatch, "--ipc", str(Path("b.sock").resolve())]
    config = Path("b.toml")
    config.write_text(config.read_text().replace("[native]", "[native]\nlan_port_mapping=true") +
                      '\n[services.echo]\ntarget="127.0.0.1:9001"\nallow_peers=["other"]\n')

    with ExitStack() as stack:
        def start(ns, *args, **options):
            proc = subprocess.Popen(["ip", "netns", "exec", ns, *args], **options)
            def stop():
                proc.terminate()
                try:
                    proc.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()
            stack.callback(stop)
            return proc

        for ns, args in [("gw-b", ["router", "10.77.0.1", "203.0.113.20", "45678"]),
                         ("gw-c", ["router", "10.78.0.1", "203.0.113.30", "45679"]),
                         ("node-b", ["echo"])]:
            proc = start(ns, sys.executable, script, *args, stdout=subprocess.PIPE, text=True)
            assert proc.stdout.readline().strip() == "ready"
        for node in ("b", "a"):
            start(f"node-{node}", gateway, "--config", f"{node}.toml", "--key", f"{node}.key",
                  "--ipc", str(Path(f"{node}.sock").resolve()))
        deadline = time.monotonic() + 15
        while not all(Path(n + ".sock").is_socket() for n in ("a", "b")):
            if time.monotonic() > deadline:
                raise RuntimeError("Gateway IPC did not start")
            time.sleep(.1)
        stream = start("node-a", *a, "connect", "other", "echo", stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        ready = stack.enter_context(selectors.DefaultSelector())
        ready.register(stream.stdout, selectors.EVENT_READ)
        seq = 0

        def wait_direct(address):
            nonlocal seq
            deadline = time.monotonic() + 35
            while time.monotonic() < deadline:
                seq += 1
                payload = f"echo-{seq}\n".encode()
                stream.stdin.write(payload)
                stream.stdin.flush()
                assert ready.select(25) and stream.stdout.readline() == payload, "Existing stream lost"
                links = json.loads(subprocess.check_output([*a, "status", "other", "--json"]))["connections"]
                if len(links) == 1 and (links[0]["path"], links[0]["address"]) == ("direct", address):
                    print(f"Existing echo stream is direct through {address}", flush=True)
                    return
                time.sleep(.3)
            raise RuntimeError(f"No direct path to {address}")

        wait_direct("203.0.113.20:45678")
        for command in [("link", "set", "wifi", "up"), ("addr", "add", "10.78.0.2/24", "dev", "wifi"),
                        ("route", "replace", "default", "via", "10.78.0.1", "dev", "wifi"),
                        ("link", "set", "eth0", "down"), ("addr", "flush", "dev", "eth0")]:
            subprocess.run(["ip", "-n", "node-b", *command], check=True)
        wait_direct("203.0.113.30:45679")
        # Check the exact postcard-encoded IPv4 socket entries in the exported ticket.
        ticket = subprocess.check_output([*b, "ticket"], text=True).strip()
        assert ticket.startswith("endpoint")
        ticket = ticket[len("endpoint"):]
        wire = base64.b32decode(ticket.upper() + "=" * (-len(ticket) % 8))
        def entry(ip, port):
            encoded = bytearray(b"\x01\x00" + socket.inet_aton(ip))
            while port >= 128:
                encoded.append((port & 127) | 128)
                port >>= 7
            encoded.append(port)
            return bytes(encoded)
        assert entry("203.0.113.30", 45679) in wire
        assert entry("203.0.113.20", 45678) not in wire, "Old mapped address still advertised"
        print("PASS: physical LAN roaming, old ticket address withdrawn, same bidirectional stream")


if __name__ == "__main__":
    if sys.argv[1] == "router":
        router(*sys.argv[2:])
    elif sys.argv[1] == "echo":
        with socketserver.ThreadingTCPServer(("127.0.0.1", 9001), Echo) as server:
            print("ready", flush=True)
            server.serve_forever()
    else:
        run(sys.argv[1])
