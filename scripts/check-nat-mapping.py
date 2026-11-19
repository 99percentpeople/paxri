"""Check the isolated gateway before attributing the CLI result to a NAT type."""
import socket
import subprocess
import sys
from contextlib import ExitStack, closing

OBSERVERS = [("203.0.113.1", 4001), ("203.0.113.1", 4002), ("203.0.113.2", 4001)]
THIRD_PARTY = ("203.0.113.3", 4999)
OTHER_PORT = ("203.0.113.1", 4998)
INTERNAL_PORT = 45555


def udp_socket(address):
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.bind(address)
    sock.settimeout(2)
    return sock


def receive_or_timeout(sock):
    try:
        return sock.recvfrom(100)
    except TimeoutError:
        return None


def internal_client(mode):
    with closing(udp_socket(("10.77.0.2", INTERNAL_PORT))) as sock:
        print("ready", flush=True)
        assert receive_or_timeout(sock) is None, "inbound UDP admitted before mapping existed"
        for dest in OBSERVERS:
            sock.sendto(b"observe", dest)
        expected = {(b"reply", OBSERVERS[0])}
        if mode == "cone":
            expected.update({(b"after", THIRD_PARTY), (b"other-port", OTHER_PORT)})
        received = set()
        while (packet := receive_or_timeout(sock)) is not None:
            received.add(packet)
        assert received == expected, (received, expected)


def check(namespace, mode, gateway):
    with ExitStack() as stack:
        observers = [stack.enter_context(closing(udp_socket(a))) for a in OBSERVERS]
        third = stack.enter_context(closing(udp_socket(THIRD_PARTY)))
        other_port = stack.enter_context(closing(udp_socket(OTHER_PORT)))
        cmd = ["ip", "netns", "exec", namespace, sys.executable, __file__, "--inside", mode]
        with subprocess.Popen(cmd, stdout=subprocess.PIPE, text=True) as client:
            assert client.stdout.readline().strip() == "ready"
            third.sendto(b"before", (gateway, INTERNAL_PORT))
            for observer in observers:
                observer.settimeout(6)
            replies = [s.recvfrom(100) for s in observers]
            assert all(data == b"observe" and addr[0] == gateway for data, addr in replies)
            mapped = [addr for _, addr in replies]
            expected = 1 if mode == "cone" else len(OBSERVERS)
            assert len(set(mapped)) == expected, mapped
            observers[0].sendto(b"reply", mapped[0])
            other_port.sendto(b"other-port", mapped[0])
            third.sendto(b"after", mapped[0])
            assert client.wait(timeout=5) == 0
            print(f"{namespace} {mode}: {len(set(mapped))} distinct mappings for "
                  f"{len(mapped)} destinations; third-party inbound "
                  f"{'accepted' if mode == 'cone' else 'filtered'}", flush=True)


if __name__ == "__main__":
    if sys.argv[1] == "--inside":
        internal_client(sys.argv[2])
    else:
        check(*sys.argv[1:])
