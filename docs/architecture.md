# Architecture

PeerHatch runs one gateway per device. The gateway owns its private key, listens on configured addresses, and keeps peer connections alive independently of the CLI and GUI. Both clients send requests to the same local management endpoint. Quitting the desktop leaves forwarding rules active.

```text
CLI or GPUI → local IPC → gateway A ⇄ authenticated peer session ⇄ gateway B
                                                                    ├─ native PTY
                                                                    ├─ file transfer
                                                                    └─ local TCP service
```

## Workspace

| Package | Responsibility |
| --- | --- |
| `apps/peerhatch-gateway` | Pairing approvals, listeners, access checks, child processes, persistent settings, and IPC |
| `apps/peerhatch-cli` | The `peerhatch` binary, command definitions shared with the GUI, gateway management, and terminal I/O |
| `apps/peerhatch-gui` | GPUI application, device management, dockable terminal sessions, and shared CLI dispatch |
| `crates/peerhatch-core` | Transport-independent session and stream types, forwarding, half-close, and reverse listeners |
| `crates/peerhatch-native` | iroh identities, peer discovery, relay selection, direct paths, and connection reuse |
| `crates/peerhatch-control` | Local IPC framing and gateway status |
| `crates/peerhatch-config` | TOML, aliases and short IDs, GUI preferences, and startup settings |
| `crates/peerhatch-terminal`, `crates/peerhatch-files` | PTY and file-transfer framing and implementation |

The native adapter multiplexes reliable ordered streams over a shared iroh connection. A future transport such as WebRTC can implement the same session and stream interfaces, including independent write-side EOF. A browser client and automatic relay promotion are future work. [Protocol](native-protocol.md)

## Gateway lifecycle

Linux and macOS use Unix domain sockets. Windows uses a local named pipe. `--ipc` or `PEERHATCH_IPC` selects an explicit endpoint. Access to the local management endpoint grants control over this node. Config updates are serialized, and the config file is replaced atomically. The private key is a separate file held by the gateway.

- **Per user.** A desktop can start its sibling gateway, attach to a running gateway, or manage startup policy. The login mode starts it when the user signs in. The CLI can do this on headless systems.
- **Linux.** A systemd installation uses a service account and a group-controlled socket. Docker runs its own non-root gateway. [Installation](installation.md)
- **macOS.** User startup uses a LaunchAgent. A system LaunchDaemon can run as a designated user. The packaged template expects a dedicated account and a writable data and socket directory. [LaunchDaemon template](../packaging/macos/org.peerhatch.gateway.plist)
- **Windows.** Boot or login startup uses Task Scheduler with an ordinary user identity. Administrators can also register `peerhatch-gateway.exe --windows-service --config FILE --key FILE` as the `PeerHatchGateway` service and grant its service account access to those files. Keep the desktop in the interactive login session.

## Forwarding and session lifetime

A local forward accepts a local TCP socket and asks the peer to open a named service or an authorized destination. A remote forward asks the peer to bind an authorized listener, then each accepted connection gets its own stream back to the initiating gateway. Both directions support multiple clients. A rule can bind to a single interface or a wildcard address.

`peerhatch connect PEER SERVICE` opens a service directly without a local TCP port. After the local IPC handshake, a four-byte big-endian length wraps chunks of up to 32 KiB on IPC. A zero-length frame signals write-side EOF. The native network stream carries raw TCP bytes after its opening handshake. A failure on one logical stream does not terminate the other streams on that peer session.

## Desktop workspace

Home displays the local gateway and pairing code, incoming requests, and the searchable paired-device list. A device page owns terminal sessions and independent TCP access and forwarding settings. The GUI subscribes to peer connection changes on the local management socket. New session and reconnect actions are available while a peer has an authenticated shared connection. The gateway applies the same check to CLI shell and exec requests. All windows share one management controller.

Each native window has a dock with horizontal or vertical tabs. The sidebar can collapse to icons or be resized. Tabs can be reordered, detached, joined to another window, or renamed inline. A terminal tab owns the PTY and emulator state across moves. Closing the tab releases the session. The terminal honors remote cursor and title changes, uses the configured ANSI palette, and supports mouse selection, scrollback, IME composition, and keyboard input.

In a terminal, drag to select text and right-click to copy it and clear the selection. Right-click with no selection to paste. When a terminal program captures the mouse, hold Shift while selecting and right-clicking to use the local clipboard.

The shared component library lives under `apps/peerhatch-gui/src/components`. Buttons, icon buttons, dropdown triggers and content, select, input, textarea, scrollbars, focus handling, and dock components use semantic colors and spacing from `src/theme`. Form controls and terminal views manage their own focus and scrolling.

## Windows development sync

From a Linux checkout, `python3 scripts/sync-windows.py --host WINDOWS_SSH_HOST --watch --dev` synchronizes source to `~/peerhatch` on a Windows machine over SSH. Both hosts need Python and a working Rust toolchain. The updater builds both checkouts, installs successful builds, and restarts running development binaries. Gateway restart closes its active streams. The Windows desktop restarts in the signed-in user’s session. Set `PEERHATCH_DEV_BUILD_HOST=windows` to sync and build on Windows only.

To run the development watcher as a user service, use a checkout at `~/peerhatch`, put `PEERHATCH_DEV_WINDOWS_HOST=your-host` in `~/.config/peerhatch/dev-watch.env`, then link and enable [peerhatch-dev-watch.service](../packaging/linux/peerhatch-dev-watch.service). The watcher matches running Linux application units named `peerhatch-gateway.service` and `peerhatch-gui.service`.
