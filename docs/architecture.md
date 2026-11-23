# Architecture

paxri runs one gateway per device. The gateway owns its private key, listens on configured addresses, and keeps peer connections alive independently of the CLI and GUI. Both clients send requests to the same local management endpoint. Quitting the desktop leaves forwarding rules active.

```text
CLI or GPUI → local IPC → gateway A ⇄ authenticated peer session ⇄ gateway B
                                                                    ├─ native PTY
                                                                    ├─ file transfer
                                                                    └─ local TCP service
```

## Workspace

| Package | Responsibility |
| --- | --- |
| `apps/paxri-gateway` | Pairing approvals, listeners, access checks, child processes, persistent settings, and IPC |
| `apps/paxri-cli` | The `paxri` binary, command definitions shared with the GUI, gateway management, and terminal I/O |
| `crates/paxri-ui` | Shared GPUI controls, device and session views, terminal emulator, ANSI painting, cursor and key encoding |
| `mobile/android` | Android GPUI host, pairing and transport, touch navigation and system keyboard |
| `apps/paxri-gui` | GPUI application, device management, dockable terminal sessions, and shared CLI dispatch |
| `crates/paxri-core` | Transport-independent session and stream types, forwarding, half-close, and reverse listeners |
| `crates/paxri-native` | iroh identities, peer discovery, relay selection, direct paths, and connection reuse |
| `crates/paxri-control` | Local IPC framing and gateway status |
| `crates/paxri-config` | TOML, aliases and short IDs, GUI preferences, and startup settings |
| `crates/paxri-terminal`, `crates/paxri-files` | PTY and file-transfer framing and implementation |

The native adapter multiplexes reliable ordered streams over a shared iroh connection. A future transport such as WebRTC can implement the same session and stream interfaces, including independent write-side EOF. A browser client and automatic relay promotion are future work. [Protocol](native-protocol.md)

## Gateway lifecycle

Linux and macOS use Unix domain sockets. Windows uses a local named pipe. `--ipc` or `PAXRI_IPC` selects an explicit endpoint. Access to the local management endpoint grants control over this node. Config updates are serialized, and the config file is replaced atomically. The private key is a separate file held by the gateway.

- **Per user.** A desktop can start its sibling gateway, attach to a running gateway, or manage startup policy. The login mode starts it when the user signs in. The CLI can do this on headless systems.
- **Linux.** A systemd installation uses a service account and a group-controlled socket. Docker runs its own non-root gateway. [Installation](installation.md)
- **macOS.** User startup uses a LaunchAgent. A system LaunchDaemon can run as a designated user. The packaged template expects a dedicated account and a writable data and socket directory. [LaunchDaemon template](../packaging/macos/org.paxri.gateway.plist)
- **Windows.** Boot or login startup uses Task Scheduler with an ordinary user identity. Administrators can also register `paxri-gateway.exe --windows-service --config FILE --key FILE` as the `paxriGateway` service and grant its service account access to those files. Keep the desktop in the interactive login session.

## Forwarding and session lifetime

A local forward accepts a local TCP socket and asks the peer to connect to the target TCP address. A remote forward asks the peer to bind a listener, then each accepted connection gets its own stream back to the initiating gateway. Both directions support multiple clients. A rule can bind to a single interface or a wildcard address.

`paxri connect PEER HOST:PORT` opens a TCP stream directly without a local TCP port. After the local IPC handshake, a four-byte big-endian length wraps chunks of up to 32 KiB on IPC. A zero-length frame signals write-side EOF. The native network stream carries raw TCP bytes after its opening handshake. A failure on one logical stream does not terminate the other streams on that peer session.

## Desktop workspace

The Devices workspace combines a searchable paired-device sidebar, pairing requests, and connection details. Its Sessions view lists open terminal views and the initiating device’s shells on connected peer gateways. Desktop and Android share the search controls, session states, and logic for merging local views with gateway entries. Its Port forwarding table lists locally stored rules and rules involving this device on connected peers. Either endpoint can manage a rule. The creating gateway stores and applies changes, including pause and resume, and checks the requesting device against the rule’s peer identity. The GUI subscribes to peer connection changes on the local management socket. New terminal sessions require an online shell host. A disconnected terminal keeps its output until its tab is closed. The gateway checks advertised shell and command-execution support for CLI requests. Desktop peers that predate capability exchange retain their existing shell entry points. All windows share one management controller. Pairing, rule creation and editing, and removal confirmation open in borderless native child windows owned by the initiating workspace. Each dialog uses a fixed size for its form, a draggable title with a close button, and a fixed action bar below its scrollable content. Pairing is modeless so the workspace stays usable while waiting for a peer. Forwarding rule forms and removal confirmations are modal. Their owner shows a dimmed backdrop and accepts input again when the dialog closes. The title-bar close button and the footer dismiss button perform the same action.

Each native window has a dock with horizontal or vertical tabs. Devices and Settings share the same resizable navigation column. It sits beside the page with top tabs and below the tab list with left tabs. Dragging the divider adjusts the width down to a minimum of 176 logical pixels. The divider preserves the chosen width when switching pages in the same window. Tabs can be reordered, detached, joined to another window, or renamed inline. A terminal tab keeps its emulator state across moves. Closing the tab detaches from the PTY. The peer gateway keeps the shell available in Sessions. The terminal uses the configured cursor shape during typing, selection, and IME composition. It honors remote cursor movement, visibility, blinking, and title changes, uses the configured ANSI palette, and supports mouse selection, scrollback, IME composition, and keyboard input.

In a terminal, drag to select text and right-click to copy it and clear the selection. Right-click with no selection to paste. When a terminal program captures the mouse, hold Shift while selecting and right-clicking to use the local clipboard.

Both clients build `crates/paxri-ui` against their platform’s GPUI version. It owns the palettes, buttons, device rows and headings, session list, VT state and terminal canvas. The desktop app adds its dock, settings, form controls and window focus handling in `apps/paxri-gui/src/components`. The Android host owns navigation, native input and transport. Returning to a device keeps each session’s PTY and emulator alive until the user closes it, the pairing is revoked, or the app process exits.

The settings editor has one navigation page per top-level TOML table. Gateway shows the current status, start and stop controls, restart, and the `gateway.startup` preference. The same page includes diagnostics and the device ID. Network maps to `[native]`. Interface contains Appearance for `[gui]` and Terminal for `[gui.terminal]` under separate headings. This device contains root keys such as `device_name`. Configuration rows show their full TOML paths. Ctrl+F (Command+F on macOS) searches names, paths, descriptions, and aliases across groups.

In **Settings → Interface → Terminal**, a live preview uses the same text grid as an open session. The font picker lists monospaced families installed on the computer running the GUI. In `[gui.terminal]`, `font_family = ""` uses the platform default, `line_height` sets the row height (16–32 logical pixels, default 18), and `letter_spacing` adds to the font’s natural cell width (−1 to 4 logical pixels, default 0). Open terminals recalculate their grid and send the new row and column counts to the remote PTY.

The mapping lives in `apps/paxri-gui/src/pages/settings_catalog.rs`. To add a preference, declare a `SettingSpec` with its local key, English title, description, and search aliases. Pass it and its control to `SettingsGroup::setting`. The editor derives the displayed path, navigation, and search results from these declarations. Use `Ui::setting_choice` for typed options, `Ui::setting_toggle` for booleans, or `TextSetting` for a string with an editable draft and Save/Cancel actions. The page binds changes to the existing gateway or GUI command.

## Windows development sync

From a Linux checkout, `python3 scripts/sync-windows.py --host WINDOWS_SSH_HOST --watch --dev` synchronizes source to `~/paxri` on a Windows machine over SSH. Both hosts need Python and a working Rust toolchain. The updater builds both checkouts, installs successful builds, and restarts running development binaries. Gateway restart closes its active streams. The Windows desktop restarts in the signed-in user’s session. Set `PAXRI_DEV_BUILD_HOST=windows` to sync and build on Windows only.

The watcher reuses Cargo’s dev build cache in `target` and updates the executables in `target/debug`. To also run the watched GUI from an existing Windows portable folder, put that folder’s absolute path on one line in `target/dev-watch/gui-directory.txt` in the **Windows checkout** (UTF-8). On the next development update, the watcher copies the incrementally built GUI there and restarts it if it is running. The portable CLI and gateway keep their installed builds. Remove the file to stop updating the extra GUI copy.

To run the development watcher as a user service, use a checkout at `~/paxri`, put `PAXRI_DEV_WINDOWS_HOST=your-host` in `~/.config/paxri/dev-watch.env`, then link and enable [paxri-dev-watch.service](../packaging/linux/paxri-dev-watch.service). The watcher matches running Linux application units named `paxri-gateway.service` and `paxri-gui.service`.
