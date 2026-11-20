# Native protocol and configuration

PeerHatch gateways share an authenticated connection using the iroh QUIC ALPN `p2p-tunnel/session/1`. The protocol identifiers and pairing-code prefix are stable across the PeerHatch rename. Each application operation opens an independent reliable, ordered, bidirectional stream.

## Node configuration

Each computer owns one version 1 TOML file. For a working pair of service and listener configurations, start with [home.toml](../examples/home.toml) and [laptop.toml](../examples/laptop.toml). Replace the example Ed25519 public keys with your devices’ keys or pair interactively. `peerhatch config check FILE` validates a file locally.

| Key | Meaning |
| --- | --- |
| `version` | Integer `1` |
| `device_name` | Display name, defaulting to the computer name |
| `pairing_code` | Persistent invitation maintained by the gateway |
| `gateway.startup` | `manual`, `gui`, `login`, or `system`. Use the GUI or CLI to also update OS registration |
| `gui.appearance` | `system` (default), `light`, or `dark` |
| `gui.terminal.scheme` | `follow-app` (default), `light`, `dark`, or `midnight` |
| `gui.terminal.cursor_shape`, `blink_cursor` | `underline` (default), `bar`, or `block`. Blinking defaults to false |
| `native.relay_url` | Optional HTTP(S) relay. Use HTTPS for a public deployment |
| `native.lan_port_mapping` | Opt in to physical IPv4 interface selection and NAT-PMP lease renewal |
| `peers.<alias>.endpoint_id`, `ticket` | 64 hex digits identifying the device and its optional addressing ticket |
| `peers.<alias>.platform` | Reported OS, including `windows`, `linux`, `macos`, or `web`. Unrecognized values display as unknown |
| `peers.<alias>.last_status_change_ms` | Last recorded pairing or connection transition, used by the device list |
| `peers.<alias>.ports.connect`, `ports.listen` | TCP destinations the peer can reach here and addresses it can listen on here. Both lists start empty |
| `services.<name>.target`, `allow_peers` | Local TCP destination and the aliases allowed to open it |
| `forwards.<name>.listen`, `direction` | Listen address and `local` (default) or `remote` |
| `forwards.<name>.peer` | Paired device that owns the other end |
| `forwards.<name>.service` or `target` | Named service for a local listener or explicit TCP destination. Choose one |

A listen address can be a loopback address, a particular interface, `0.0.0.0` or `[::]`. Port zero asks the OS for an available port. A bare port in the CLI uses IPv4 loopback. The running gateway reports the actual bound address.

## Pairing and addressing

Pairing uses the separate ALPN `p2p-tunnel/pair/1`. The code `p2p1.<endpoint ticket>.<token>` contains the issuer’s addressing ticket and a random 128-bit invitation. Each request has a fresh random request ID. Each side sends a UTF-8 JSON message of up to 16 KiB followed by FIN. The receiver verifies the ticket’s key against the authenticated peer before asking the local user to approve.

The issuer saves the pairing before answering with its name and ticket. The requester verifies the responder’s key before persisting it. Existing keys retain their aliases and permissions. Name collisions get a numeric suffix. Refreshing a code invalidates requests made with its old value. New peers can use PTY and files and begin with empty TCP allowlists.

On an established connection, each peer can open a unidirectional stream to announce updated addressing tickets. Version byte `1` precedes the sequence of UTF-8 tickets, each with a big-endian `u16` length of at most 8192 bytes. The receiver verifies the device ID before updating its cache.

## Shared connection and stream types

The peer with the lexicographically smaller public key serializes connection selection. On the first bidirectional control stream it sends a presence byte, a 16-byte identifier if there is an existing connection, and a fresh 16-byte candidate identifier, then finishes sending. The other peer replies `1` if both still hold the same connection, or installs the candidate and replies `0`. Simultaneous dials converge. Each subsequent bidirectional stream opens with a type byte.

| Byte | Operation |
| --- | --- |
| 1 | Named service, explicit TCP target, or device information |
| 2 | Interactive PTY |
| 3 | Execute a program or shell command in a PTY |
| 4 | Request a TCP listener on the peer |
| 5 | TCP connection accepted by that listener |
| 6 | File transfer |

## TCP opening, permissions, and EOF

On a type 1 stream the initiator immediately sends a big-endian `u32` byte length and 1–4096 bytes of UTF-8 JSON. For example, `{"type":"OPEN","service":"dev-web"}` opens a named service. `{"type":"OPEN_TCP","target":"127.0.0.1:3000"}` opens a target authorized in `ports.connect` or explicitly published to this peer. Device-information requests are also authenticated.

The host looks up the authenticated device and opens the approved TCP destination. It responds with `{"type":"OK"}` or `{"type":"ERROR","code":"DENIED"}` using the same framing. Other errors are `TARGET_REFUSED`, `TARGET_TIMEOUT`, `TARGET_UNREACHABLE`, and `INVALID_REQUEST`. Unknown and unauthorized services both return `DENIED`. After `OK`, exchange raw bytes. TCP EOF finishes only that sending direction. Preserve buffered bytes following the handshake and allow the reverse direction to complete.

A type 4 request carries length-prefixed JSON with a `listen` address and a random 16-byte `id`. After checking `ports.listen`, the host replies with `bound` or `error` and keeps the control stream open. Each accepted TCP socket starts a type 5 stream with the raw 16-byte ID. The initiator checks the peer and active ID before connecting its local target. Closing the control stream releases the port. Removing authorization also releases the listener. Existing streams can finish. Persisted rules retry after a disconnect.

## Native PTY

For types 2 and 3 the initiator sends rows and columns as two big-endian `u16` values, each between 1 and 1024. Type 3 also carries a big-endian `u16` length and 1–32768 bytes of UTF-8 JSON. `{"kind":"program","program":"vim","args":["notes.txt"]}` preserves argument boundaries. `{"kind":"shell","script":"uptime"}` invokes the gateway account’s shell.

The host checks pairing, account privileges, and the 16-session quota. Its one-byte status is ready (0), denied (1), unavailable (2), busy (3), or privileged account (4). After ready, frames use a type byte and big-endian fields:

| Type | Direction | Payload |
| --- | --- | --- |
| 1 | Either | `u16` byte count (1–32768) and raw PTY data |
| 2 | Client to host | `u16` rows and `u16` columns |
| 3 | Host to client | `u32` child exit code after the final output |

UTF-8 characters and escape sequences may span frames. The CLI uses the host terminal. The GUI emulator handles cursor position, alternate screen, SGR colors, application mouse reporting, bracketed paste, focus events, terminal queries, and palette or cursor changes. OSC 0/2 can update a tab title until the user renames it. Closing the client, losing the session, or unpairing the peer releases the PTY.

## File transfer

Type 6 streams require a paired identity and run as the gateway account. A big-endian `u32` precedes each frame. Its high bit selects binary data or JSON control, and the low 31 bits give a length of 1–65536. A `get` or `put` request carries a path and the `recursive`, `preserve`, and `no_clobber` flags. `put` can require an existing directory. The recipient replies with `ready` or `error`.

Senders bracket directory entries with `directory` and `end_directory`. A `file` header declares name and size, followed by data frames and `end_file`. A receiver checks the size, stages the file alongside its destination, commits it atomically, and acknowledges with `done`. No-clobber mode can reply `skipped` if a file exists or another writer creates it first. A final `done` confirms the root entry. Preserved metadata includes timestamps and ordinary Unix permissions where supported. Received child names are validated, links and special files are rejected, and recursive depth is limited to 128.

## Diagnostic echo

`p2p-tunnel/probe/1` uses its own authenticated bidirectional stream. The caller sends a big-endian `u16` count, then successive `u16` sequence numbers starting at 1. The listener checks the configured peer and echoes each sequence. The probe exercises path selection and measures application-level RTT. [Network experiments](connectivity.md)
