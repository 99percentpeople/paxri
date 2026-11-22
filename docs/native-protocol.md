# Native protocol and configuration

PeerHatch gateways share an authenticated connection using the iroh QUIC ALPN `p2p-tunnel/session/1`. The protocol identifiers and pairing-code prefix are stable across the PeerHatch rename. Each application operation opens an independent reliable, ordered, bidirectional stream.

## Node configuration

Each computer owns one version 1 TOML file. For a working pair of device configurations, start with [home.toml](../examples/home.toml) and [laptop.toml](../examples/laptop.toml). Replace the example Ed25519 public keys with your devices’ keys or pair interactively. `peerhatch config check FILE` validates a file locally.

| Key | Meaning |
| --- | --- |
| `version` | Integer `1` |
| `device_name` | Display name, defaulting to the computer name |
| `pairing_code` | Persistent invitation maintained by the gateway |
| `peers.<alias>.pairing_id` | Approval request ID for pairings established by two gateways with unpair support |
| `pending_unpairs.<device-id>` | Gateway-managed notice awaiting acknowledgement, with the peer ticket and pairing generation |
| `gateway.startup` | `manual`, `gui`, `login`, or `system`. Use the GUI or CLI to also update OS registration |
| `gui.appearance` | `system` (default), `light`, or `dark` |
| `gui.tab_placement` | Position of the workspace tab bar in all windows, `top` (default) or `left` |
| `gui.terminal.scheme` | `follow-app` (default), `light`, `dark`, or `midnight` |
| `gui.terminal.cursor_shape`, `blink_cursor` | `underline` (default), `bar`, or `block`. Blinking defaults to false |
| `native.relay_url` | Optional HTTP(S) relay. Use HTTPS for a public deployment |
| `native.upnp` | Allow UPnP discovery and UDP mappings on the default router (default `true`). PCP and NAT-PMP remain enabled |
| `native.lan_port_mapping` | Opt in to physical IPv4 interface selection and NAT-PMP lease renewal |
| `peers.<alias>.endpoint_id`, `ticket` | 64 hex digits identifying the device and its optional addressing ticket |
| `peers.<alias>.platform` | Reported OS, including `android`, `windows`, `linux`, `macos`, or `web`. Unrecognized values display as unknown |
| `peers.<alias>.last_status_change_ms` | Last recorded pairing or connection transition, used by the device list |
| `forwards.<name>.listen`, `direction` | Listen address and `local` (default) or `remote` |
| `forwards.<name>.peer` | Paired device that owns the other end |
| `forwards.<name>.target` | Destination TCP host and port, or a loopback port |

A listener can bind to loopback, a particular interface, `0.0.0.0` or `[::]`. Port zero asks the OS for an available port. A bare port in the GUI or CLI uses IPv4 loopback.

Both endpoints can manage a forwarding rule. The creating gateway stores it in `[forwards.<name>]`. Use `peerhatch forward list` to see the storage owner, direction from this device, actual bound port, target and state. For an older named forwarding rule, replace its `service` key with the target TCP address before starting the gateway. The retired `[services]` and `[port_access]` tables are ignored.

## Pairing and addressing

Pairing uses the separate ALPN `p2p-tunnel/pair/1`. The code `p2p1.<endpoint ticket>.<token>` contains the issuer’s addressing ticket and a random 128-bit invitation. Each request has a fresh random request ID. Each side sends a UTF-8 JSON message of up to 16 KiB followed by FIN. The receiver verifies the ticket’s key against the authenticated peer before asking the local user to approve.

The issuer saves the pairing before answering with its name and ticket. The requester verifies the responder’s key before persisting it. Existing keys retain their aliases. Name collisions get a numeric suffix. Refreshing a code invalidates requests made with its old value. Pairing contacts can include the platform and a `capabilities` object. Android uses the system device name for pairing, falling back to the model and then `Android`. The Android client advertises `shell: false` and `exec: false`. Gateways report shell hosting and command execution for their own account.

On an established connection, each peer can open a unidirectional stream to announce updated addressing tickets. Version byte `1` precedes the sequence of UTF-8 tickets, each with a big-endian `u16` length of at most 8192 bytes. The receiver verifies the device ID before updating its cache.

Unpairing commits a local revocation and its pending notification in one settings write, closes the shared session, and releases the affected listeners. The gateway sends `{"unpair":{"pairing_id":"…"}}` on the independent pairing protocol so confirmation can finish after application streams have closed. The recipient can revoke only the authenticated caller and acknowledges after its settings are saved. Already-absent peers acknowledge idempotently. Re-approval establishes a new request ID and cancels old outgoing notices. A recipient retains its newer pairing if it receives an earlier generation. Offline notices survive restarts and are retried using the saved peer address.

A gateway announces its availability to paired peers by establishing the shared session when it starts, learns a new pairing or address, or regains local network or relay connectivity. Each side observes admission and closure on the same connection. A lost connection triggers a reconnect attempt. An offline peer can announce itself when it comes back. Announcements use at most four concurrent dials and open no terminal or application stream.

## Shared connection and stream types

The peer with the lexicographically smaller public key serializes connection selection. On the first bidirectional control stream it sends a presence byte, a 16-byte identifier if there is an existing connection, and a fresh 16-byte candidate identifier. The other peer replies `1` if both still hold the same connection, or installs the candidate and replies `0`. Simultaneous dials converge. After the acknowledgement, both endpoints send their current platform and capabilities on the selected admission stream and push a replacement snapshot whenever they change. Android snapshots also carry the current system device name. The device list displays it alongside the saved alias when they differ. Each snapshot is UTF-8 JSON with a big-endian `u16` length and a 4096-byte limit. Reconnection exchanges fresh snapshots. Older peers finish the admission stream after the acknowledgement and continue using the same application streams. Each subsequent bidirectional stream opens with a type byte.

| Byte | Operation |
| --- | --- |
| 1 | Explicit TCP target or device information |
| 2 | Interactive PTY |
| 3 | Execute a program or shell command in a PTY |
| 4 | Request a TCP listener on the peer |
| 5 | TCP connection accepted by that listener |
| 6 | File transfer |
| 7 | List or manage this pairing’s forwarding rules |
| 8 | Execute a program or shell command with binary pipes |

## TCP forwarding

On a type 1 stream the initiator sends a big-endian `u32` byte length and 1–4096 bytes of UTF-8 JSON. For example, `{"type":"OPEN_TCP","target":"127.0.0.1:3000"}` connects to port 3000 on the receiving gateway’s loopback interface. The receiver checks pairing, validates the target and connects using its operating-system account. It responds with `{"type":"OK"}` or an error code such as `DENIED`, `TARGET_REFUSED`, `TARGET_TIMEOUT`, `TARGET_UNREACHABLE`, or `INVALID_REQUEST`. After `OK`, the stream carries raw TCP bytes. EOF finishes only that sending direction.

Type 7 uses length-prefixed JSON (up to 1 MiB). A request contains an `op` of `list`, `edit`, `set_enabled`, or `remove`. The receiver derives the caller from the authenticated session and only returns or changes its own rules for that identity. An edit supplies the listen address, target and direction from the caller’s perspective. It cannot change the paired device. The reply contains the receiver’s rules for the caller or an error. Each gateway queries connected peers when the forwarding list is requested.

A type 4 request carries length-prefixed JSON with a `listen` address and a random 16-byte `id`. After checking the authenticated pairing, the host replies with `bound` or `error` and keeps the control stream open. Each accepted TCP socket starts a type 5 stream with the raw 16-byte ID. The initiator checks the peer and active ID before connecting its local target. Closing the control stream releases the port. Unpairing also releases the listener. Existing streams can finish. Persisted rules retry after a disconnect.

## Native PTY

For types 2 and 3 the initiator sends rows and columns as two big-endian `u16` values, each between 1 and 1024. Type 3 also carries a big-endian `u16` length and 1–32768 bytes of UTF-8 JSON. `{"kind":"program","program":"vim","args":["notes.txt"]}` preserves argument boundaries. `{"kind":"shell","script":"uptime"}` invokes the gateway account’s shell.

The host checks pairing, account privileges, and the 16-session quota. Its one-byte status is ready (0), denied (1), unavailable (2), busy (3), or privileged account (4). After ready, frames use a type byte and big-endian fields:

| Type | Direction | Payload |
| --- | --- | --- |
| 1 | Either | `u16` byte count (1–32768) and raw PTY data |
| 2 | Client to host | `u16` rows and `u16` columns |
| 3 | Host to client | `u32` child exit code after the final output |

UTF-8 characters and escape sequences may span frames. The CLI uses the host terminal. The GUI emulator handles cursor position, alternate screen, SGR colors, application mouse reporting, bracketed paste, focus events, terminal queries, and palette or cursor changes. OSC 0/2 can update a tab title until the user renames it. Closing the client, losing the session, or unpairing the peer releases the PTY.

## Piped commands

Type 8 takes the same length-prefixed invocation as type 3 and omits the terminal size. Admission checks, the shared session quota and the one-byte launch reply match those of a native terminal. An unsupported stream type fails the request before execution.

After ready, the client sends a big-endian `u16` length followed by up to 32768 bytes of stdin. A zero length closes the child's stdin. The client keeps the transport open to receive results. The host prefixes each output chunk with 1 for stdout or 2 for stderr, then a big-endian `u16` length (1–32768) and the original bytes. After both pipes drain, the host sends 3 and the big-endian `u32` exit code. A Unix signal exit uses 128 plus the signal number.

## File transfer

Type 6 streams require a paired identity and run as the gateway account. A big-endian `u32` precedes each frame. Its high bit selects binary data or JSON control, and the low 31 bits give a length of 1–65536. A `get` or `put` request carries a path and the `recursive`, `preserve`, and `no_clobber` flags. `put` can require an existing directory. The recipient replies with `ready` or `error`.

Senders bracket directory entries with `directory` and `end_directory`. A `file` header declares name and size, followed by data frames and `end_file`. A receiver checks the size, stages the file alongside its destination, commits it atomically, and acknowledges with `done`. No-clobber mode can reply `skipped` if a file exists or another writer creates it first. A final `done` confirms the root entry. Preserved metadata includes timestamps and ordinary Unix permissions where supported. Received child names are validated, links and special files are rejected, and recursive depth is limited to 128.

## Diagnostic echo

`p2p-tunnel/probe/1` uses its own authenticated bidirectional stream. The caller sends a big-endian `u16` count, then successive `u16` sequence numbers starting at 1. The listener checks the configured peer and echoes each sequence. The probe exercises path selection and measures application-level RTT. [Network experiments](connectivity.md)
