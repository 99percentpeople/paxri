# NAT traversal and connectivity

paxri authenticates a device by its public key. An endpoint ticket provides its identity, relay hints, and IP candidates. When the default public infrastructure is enabled, the gateway can also look up the peer’s current address. A relay gives the two endpoints an initial path. iroh checks direct IPv4 and IPv6 UDP paths while the same encrypted connection carries traffic.

## Observe the actual path

Select a peer in the Devices sidebar or run `paxri status PEER`. **Direct** shows the peer address used by the connection. **Relay** identifies the relay carrying traffic. **Selecting path** means path selection is in progress. The displayed RTT is the QUIC estimate. `--json` includes per-connection details and counts of active streams. Gateways announce themselves silently to paired peers at startup, on network changes, and when returning to the Android app, keeping shared connections available between sessions. When a previously live connection drops, the gateway retries briefly with short backoff (`2s`, `5s`, `15s`) and then stops background polling, waiting passively for the peer to reconnect when it comes back online or for an explicit reconnect or stream request. The desktop receives connection changes from its local gateway. Disconnected terminals keep their last output until you close them. Use `paxri status PEER --check` to explicitly contact a peer for diagnostics.

The device page refreshes **Upload**, **Download**, and **Latency** about once per second for the displayed device. Each chart scrolls through the last minute of readings. The first reading appears as a dot halfway up the chart. As readings arrive, the chart connects them with a line and leaves a gap when they are more than five seconds apart. When a point scrolls past the left edge, the visible part of its line continues scrolling until the next point reaches the edge. Rates use the change in the local gateway’s QUIC byte counters over the actual time between samples, including control traffic and retransmissions. Hover over either rate to see the total sent or received on the current connection.

`paxri status PEER` reads a single snapshot from the local gateway. **SENT (B)** and **RECEIVED (B)** are cumulative byte counts for the current connection. In `--json`, use `connections[].traffic.sent_bytes`, `traffic.received_bytes` and `rtt_ms` on the same connection. To calculate a rate, divide the change in a counter by the elapsed seconds between your own queries. Start a new baseline when `traffic.connected_at_ms` changes. A gateway that does not report counters omits `traffic`; the table displays `-`.

To measure application echo time between two nodes, use `paxri probe listen` and `paxri probe connect`. Pass `--relay-only` when measuring relay performance. Use `--no-relay` to test an IP-only route with a known peer ticket. Set `RUST_LOG=iroh::net_report=debug,iroh::socket::remote_map=debug` for detailed native diagnostics.

## Public relay and UDP discovery

With the default public relays, each gateway measures three authenticated ping/pong exchanges to each reachable candidate and ranks them by median RTT. It periodically rechecks the ranking, keeps the current home when latencies are close, and checks sooner on disconnect or network change. A peer that is already online can exchange fresh tickets over its shared connection. A configured `native.relay_url` selects a private relay and limits discovery to that relay.

Public UDP address discovery uses the **same QUIC socket** as gateway traffic. IPv4 and IPv6 observations are grouped separately by the actual IP address of the observer. Even if the lowest-latency data relay cannot answer UDP discovery, other observers can return candidates.

| Gateway log | Interpretation |
| --- | --- |
| `mapping=consistent` | At least two different observer IPs saw the same source IP and port |
| `mapping=varies` | Observers saw different public mappings |
| `mapping=unknown` | Fewer than two independent observations were available for this family |

A consistent mapping still needs a successful peer-to-peer check. UDP filtering, carrier NAT, VPN routing, and the peer’s network determine whether an address is actually reachable.

## Multiple IPv4 exits and roaming

Operating-system routing selects the default UDP exit. If a physical LAN router supports mapping and that path should carry direct traffic, enable this on that device:

```toml
[native]
lan_port_mapping = true
```

The IPv4 QUIC socket stays bound to a wildcard address. The gateway chooses an active physical source, verifies the on-link router, and requests a finite NAT-PMP lease for the actual socket port. It renews at half the granted lifetime. After a network change it reselects the interface and router, withdraws an obsolete address, and announces the replacement. A router that comes online later is retried. If no physical NAT-PMP router responds, the normal OS-selected PCP, UPnP, and NAT-PMP mapper can be used. IPv6 paths and the relay remain available. A stream can survive a move when QUIC recovers a usable path before the connection times out. [NAT-PMP](https://www.rfc-editor.org/rfc/rfc6886.html)

A machine with a reachable public UDP address can accept direct traffic itself. To forward encrypted traffic between two *other* peers, run a separate iroh relay with a publicly reachable HTTPS endpoint. Creating a port mapping alone does not make a gateway a relay. The default public-relay selection can fail over between its candidates. An explicitly configured private relay is a single discovery and fallback endpoint. Established direct paths can continue during a relay outage.

## Reproduce the Linux network experiments

Use a kernel that permits unprivileged network namespaces. Install iproute2, nftables, util-linux, OpenSSL, Python 3.8+, and an `iroh-relay` 1.4.1 binary. Build `paxri` and `paxri-gateway` with `cargo build --locked`. Override `PAXRI_BIN` and `IROH_RELAY_BIN` to select specific binaries.

```sh
scripts/check-nat.sh cone cone
scripts/check-nat.sh symmetric cone
scripts/check-nat.sh cone symmetric
scripts/check-nat.sh symmetric symmetric
scripts/check-nat.sh --dual-stack symmetric symmetric
scripts/check-nat.sh --ipv6-only
scripts/check-nat.sh --gateway
scripts/check-nat.sh --roam
```

Each client gets its own isolated LAN and router. The routers’ external addresses are on a documentation network. A private HTTPS relay with a temporary test CA also provides UDP address discovery. Calibration sends packets from one socket to distinct destination addresses and ports, then checks which third-party senders can reach the mapping.

| Network or event | Observed result |
| --- | --- |
| Cone/cone, symmetric/cone, cone/symmetric IPv4 | Each combination reached a direct path during 20 echoes on one stream |
| Symmetric/symmetric IPv4, disjoint port pools and strict filtering | All 20 echoes used the relay |
| Dual stack with two symmetric IPv4 routers | IPv6 direct traffic succeeded through stateful IPv6 firewalls |
| Peer UDP initially blocked, then allowed | In a 25-echo run the first three used the relay and the remaining 22 used direct UDP |
| A direct path was blocked during transfer | The same stream completed. Its first relay response after the loss took 14.16 seconds in one run |
| TCP forward forced through a private relay | Two gateways transferred a 256 KiB random file and verified its contents |
| Physical interface and NAT-PMP router replaced | The same open bidirectional stream went direct through the second router. The ticket dropped the old mapping |

The IPv6-only fixture uses a relay hostname with an AAAA record and a matching certificate. Give self-hosted relays a DNS name and matching TLS certificate. Configure IPv6 TCP targets using bracket notation such as `[::1]:3000`.

On a Windows x64 machine paired with a Linux ARM64 device, a listener pinned to Ethernet accepted traffic on that IPv4 address. A wildcard IPv4 listener accepted loopback, Ethernet, and virtual-interface traffic. A separate IPv6 listener accepted IPv6, and both families could bind the same numeric port. The Windows inbound firewall affected new inbound TCP connections. SSH over an explicitly forwarded SSH service and paxri’s own native PTY are separate uses of the transport.

## Measure performance on your network

Report cold connection time, time to direct, selected path, application throughput, RTT, and interruptions. Measure both directions. QUIC flow control and loss recovery preserve stream ordering, and a slow or lossy network can still increase latency. A 100 Mbit/s path with an 80 ms RTT has about 1 MB of data in flight at full throughput. [QUIC flow control](https://www.rfc-editor.org/rfc/rfc9000.html#section-4) · [Loss recovery](https://www.rfc-editor.org/rfc/rfc9002.html)

For repeatable comparisons, start with a LAN baseline, then repeat from different ISPs, mobile networks, and with direct UDP blocked. Test concurrent file and interactive streams, half-close, and recovery from both a single-path loss and a complete disconnect. The namespace results describe these fixtures. Estimating a real-world direct-connection rate requires independent connections on representative networks.
