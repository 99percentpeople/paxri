# paxri

paxri (Peer Access eXchange & Remote Interconnect) lets you open shells, copy files, and forward TCP ports between paired computers across networks. Its Rust CLI and GPUI desktop app use the same background gateway and TOML configuration.

The gateway establishes an authenticated iroh connection to each peer. Terminal sessions, file transfers, and forwards each open their own stream on that connection. Peers try direct IPv4 and IPv6 paths and can use an encrypted relay when direct UDP is unavailable.

## Get started

Download the archive for your OS from the **paxri rolling build** prerelease. Each successful `main` build updates the same page. Builds are available for Windows x64, Linux AMD64 and ARM64, and macOS Intel and Apple Silicon. Extract the archive and keep the three programs together. The CLI is `bin/paxri` on Unix and `paxri.exe` on Windows. This README and `installation.md` sit at the top of each archive. Third-party licenses are in `licenses/`, or `paxri.app/Contents/Resources/licenses/` on macOS. [Download the rolling build](../../releases/tag/rolling)

For a headless Linux node, run these commands in the directory containing [compose.yaml](compose.yaml).

```sh
docker compose up -d --wait
docker compose exec gateway paxri pair code
```

Compose pulls `ghcr.io/99percentpeople/paxri:dev`. You can also pin an image by commit or load it offline. [Installation](docs/installation.md)

Linux archives come in glibc builds for AMD64 and ARM64 and a musl build for ARM64. All three include the CLI, gateway, and GUI. On an ARM64 router you can extract just the CLI and gateway from the musl archive. [Headless installation](docs/installation.md#headless-arm64-musl-installation)

## Pair two computers

Open **paxri** on macOS or run `paxri-gui` on either desktop. In **Devices**, choose **Add Device** at the end of the device list to see the local pairing code or enter one from another device. Enter the first computer’s code on the second, then approve the request on the first. Share the code with the person who controls the other device. Approval grants shell and file access as the gateway account. Paired devices can connect to TCP addresses reachable by the gateway and bind TCP listeners with the gateway account’s privileges.

The CLI performs the same steps. On **both** computers, start the gateway if it is idle:

```sh
paxri gateway start
```

| Where | Command | Result |
| --- | --- | --- |
| First computer | `paxri pair code` | Prints the current invitation |
| First computer | `paxri pair qr` | Displays the current invitation as a QR code for Android |
| Second computer | `paxri pair join "PAIRING_CODE"` | Sends an approval request |
| First computer | `paxri pair status` | Lists the requesting device and its short ID |
| First computer | `paxri pair accept DEVICE_ID` | Saves the pairing on both devices (accepts a full ID or unique short prefix) |
| Either computer | `paxri pair remove DEVICE` | Revokes local access immediately and requests confirmation from the other gateway |
| Either computer | `paxri status` | Lists peers, short IDs, streams, and forwarding rules |

For a small terminal, save the QR image with `paxri pair qr --svg > pairing.svg` and open it in a browser. The pairing code survives restarts. `paxri pair refresh` replaces it and dismisses requests made with the old code. `paxri pair accept` and `paxri pair decline` accept a full public ID or a unique hexadecimal prefix of at least four characters. Peer commands accept the displayed device name, the full public ID, or a unique hexadecimal prefix of at least four characters.

To unpair in the desktop app, open **Devices**, select the device, and choose **Unpair device**. Unpairing works offline. It removes local aliases and forwarding rules for that identity and revokes its access to the gateway’s TCP targets and reverse listeners. An online peer removes the corresponding pairing and confirms. For an offline peer the gateway saves the notification and retries after reconnects or restarts. `paxri pair status` shows pending confirmations. Pairing again requires a new approval.

## Use a paired device

The examples use a device named `home`. Substitute the name or short ID printed by `paxri status`.

```sh
paxri status home --check
paxri shell home
paxri exec home -- vim "notes from today.txt"
paxri exec home -c 'uname -s && uptime'
paxri exec --pipe home -- cat image.png > copy.png
paxri cp build.zip home:Downloads/
paxri cp -rp home:projects/demo ./backups/
```

**Terminal.** The gateway starts a native PTY under its own account. Windows prefers PowerShell 7 and falls back to Windows PowerShell 5.1. On Unix it uses the account’s shell. `exec` preserves terminal input, resize events, and the remote exit status. Captured output can contain both output streams and terminal escape sequences. A redirected session starts at 80 columns by 24 rows. An unprivileged gateway can host terminal and file sessions.

Open **Settings → Terminal controls** on desktop or Android to view or change the hardware keyboard shortcuts for copy, paste, zoom in, zoom out, and reset zoom. Select the current binding and press the desired keys together to save it. Esc cancels recording. The menu beside each binding can restore its platform default or disable the shortcut. **Copy** copies the local selection. With no selection, the key follows normal terminal input handling. The same page controls whether programs in the active terminal can write to this device’s clipboard. On Android, **Choose keys on screen** in the binding menu lets you select modifiers and a key by touch. **Terminal keys** opens the on-screen key editor.

With the terminal focused, the default zoom shortcuts are **Ctrl++** or **Ctrl+=** to zoom in, **Ctrl+-** to zoom out, and **Ctrl+0** to return to the configured size. On macOS, Command also works. Pinch with two fingers on a touchscreen to zoom the terminal. Text, column width, and line height scale together, including custom line heights. The remote shell receives the new row and column counts. Window titles, tabs and the rest of the interface use the system display scale. Android also respects the system text size.

**Links and bell.** Hold Ctrl (Command on macOS) and click a URL or OSC 8 hyperlink in a desktop terminal to open it. On Android, tap a link to open or copy it. Only `http`, `https` and `mailto` links open. When a program rings the terminal bell, the desktop plays the system alert sound and briefly shows a bell icon on the tab. Android plays the notification sound. Turn either feature off with **Clickable links** or **Terminal bell** in **Settings → Terminal controls** (`clickable_links` and `terminal_bell` in `[gui.controls]`).

Inside a remote terminal, `paxri status` queries the gateway hosting that terminal. From the initiating computer, `paxri exec home -- paxri status` does the same. Keep the CLI beside the gateway executable. To add commands for this device, put executables in `tools/bin/` beside its `node.toml`. [Session tools and examples](docs/installation.md#remote-terminal-tools)

Use `exec --pipe` for scripts and binary tools. It streams stdin, stdout and stderr separately, forwards stdin EOF, and returns the remote exit code. Both gateways must support this mode. With a screenshot tool installed on the peer, an application can read its PNG directly from stdout. Use a binary-safe caller such as Python’s `subprocess.run(..., capture_output=True)`.

**Files.** `paxri cp` transfers in either direction. `peer:path` is relative to the gateway user’s home directory unless the path is absolute. Use `-r` for directories, `-p` for timestamps and supported Unix permission bits, `-n` to skip existing files, `-v` for per-file reporting, and `-q` for errors only. Multiple sources require an existing destination directory. An interrupted file keeps its previous contents. Completed files in the same transfer remain on disk.

**Port forwarding.** In **Devices → Port forwarding**, choose **Add rule**, select a paired device, optionally set a **Rule name**, and enter the listening port and target. A bare listening port binds IPv4 loopback, and a bare target port connects to IPv4 loopback with IPv6 loopback fallback. **Forward** opens a new listening port on this computer to reach an existing service on the peer. **Reverse** opens a new listening port on the peer to reach an existing service on this computer.

```sh
paxri forward add web --peer home --listen 8080 --target 3000
paxri forward add preview --peer home --remote --listen 9000 --target 5000
paxri forward edit web --rename app --peer home --listen 8080 --target 3000
paxri forward list
```

Open `http://127.0.0.1:8080` on this computer to reach port 3000 on `home`. Both devices can edit, rename, pause, resume or remove their shared rule. The creating gateway stores it. From the other device, use the owner in the list, for example `paxri forward pause web --owner laptop`. Locally stored rules remain visible when the peer goes offline. Rules stored on the peer appear while it is connected. To open a TCP stream directly on stdin/stdout, use `paxri connect home 3000`. [Example configuration](examples/laptop.toml)

**Desktop.** Select a peer in **Devices** to see its connection, or open its actions menu and choose **Reconnect** to re-check availability and renegotiate a faster direct or relay path. Open **Sessions** in the sidebar to find terminals across paired devices, or open it from a device’s details to filter by that device. Search by terminal title, device, or session number and filter by state. **Port forwarding** lists rules for all devices. Filter by device, add a rule, or open a row’s menu to edit, pause, resume, or remove it. Use the floating terminal button in the lower-right corner to open another session. With multiple available peers, it opens a device picker. The **+** button after the tabs groups sessions by device. Choose **New** beside a device to start a shell, or choose a detached session to reopen it in a tab. Press Ctrl+F (Command+F on macOS) to jump to the search field in Devices, Sessions, Port forwarding or Settings. Esc clears it. Tabs can be renamed, reordered, and detached. Closing a tab or the last GUI window leaves its shell running on the peer gateway and preserves its title. Open **Sessions** to resume it, or choose **End session** from its row menu to stop it. The gateway also ends a session when the shell exits or when the initiating device stays disconnected longer than the hosting gateway’s **Session keep-alive**. Set it in **Settings → Gateway** to 5 minutes (the default), 30 minutes, 1 hour, 6 hours, 24 hours or Forever. It is saved as `session_keep_alive` in `[gateway]`, and changes apply to running sessions. Settings includes application appearance, terminal colors, cursor, and gateway startup. `paxri-gui` opens Devices without arguments and accepts the same commands as `paxri` when launched from a terminal.

## Android client

The ARM64 Android client uses GPUI for the device list and terminal, the Android input method for typing, and the desktop gateway's pairing and PTY protocols. It requires Android 10 or later and a Vulkan-capable GPU.

On the **Devices** home screen, search the device list or filter by operating system. Open **Sessions** from the navigation menu for the same session search, filters, and floating terminal button as on desktop. Use the session row menu to end or rename a session. **Rename session** is also available in the terminal menu. A manual name stays with that session across app restarts. Clear the name to follow the terminal title again.

While a paired device is connected, Android uses a connected-device foreground service so switching apps leaves the transport and open terminals running. The service allows five minutes for a lost connection to return. Removing the last pairing or dismissing the app from Recents stops the service.

Download `paxri-android-arm64.apk` from the rolling prerelease. Rolling builds use a fixed signing key so you can install updates over an existing fixed-signature build and keep your pairings and settings. Moving from an earlier development-signed APK requires a one-time uninstall, which clears its app data. Pull requests still produce a development-signed APK. [Rolling prerelease](../../releases/tag/rolling) · [Build runs](../../actions/workflows/builds.yml)

To build the development APK, install JDK 17, Python 3, Android SDK Platform 35, Build Tools 35.0.0 and the NDK (tested with r26b). Set `ANDROID_HOME` to the SDK directory and `ANDROID_NDK_HOME` to the NDK directory. From the repository root run:

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk --locked
python3 mobile/android/build.py
adb install -r dist/android/paxri-android-arm64.apk
```

On ARM64 Linux, the SDK tools must also run on the build host. If you already have an NDK cross toolchain, set `CC_aarch64_linux_android`, `CXX_aarch64_linux_android`, and `CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER` to its compiler drivers. With an explicit linker the packaging script invokes Cargo directly.

On the computer, open **Devices** and choose **Pair a device** (the floating **+** for your first pairing), or run `paxri pair qr`. In the Android device list, tap the floating **+** button at the bottom right. Choose **Scan QR code**, allow camera access, and point the camera at the code. To scan a saved screenshot, tap **Choose QR image**. Choose **Enter pairing code** to paste a text invitation from `paxri pair code`. Approve the phone’s request on the computer to complete pairing.

The device list shows connection state and the last observed change. Open a connected computer to see its direct or relay path, upload and download rates, and round-trip latency, or choose **Reconnect** from its top-bar menu to re-check reachability and renegotiate a faster path. Tap the floating terminal button at the bottom right to start a session, then tap the terminal to open the keyboard. Drag one finger to scroll or pinch with two fingers to zoom. The phone remembers the zoom level. The back arrow returns to the device’s session list and keeps the shell running. Reopen a session to continue, start another shell, or choose **End session** in the top bar to release it. Sessions last while the Android app process is running. On the Android terminal toolbar, hold an arrow key, Backspace, Delete, Page Up, Page Down, or Space to repeat it at the system keyboard rate. Release it or slide off the key to stop. Hold Ctrl, Alt or Shift while pressing another key, or tap a modifier to latch it for the next key. Tap a latched modifier again to cancel. Modifiers can be combined with each other, the software keyboard, and the terminal toolbar. The PTY resizes when the keyboard opens or closes. The app keeps the screen awake while it is in the foreground.

In **Settings → Terminal keys**, edit each row as a comma-separated list. The preview updates as you type. Add or remove individual keys in the text field. Use **Add row** for a new row or tap the trash icon beside a row to delete it. Use `Comma` for a literal comma. Key vibration, height steps and row removal apply immediately to open sessions. Text fields save when you finish editing or leave the page. **Restore defaults** restores the two default rows at 40 dp, with ↑ above ↓ and PgUp above PgDn. The Android app stores its settings in its private `files/node.toml` (migrated automatically from earlier `files/gui.toml` installs), with one inner array per row. For example:

```toml
[gui.keybar]
row_height = 40
haptics = "standard"
rows = [
  ["Esc", "Tab", "Ctrl", "Alt", "Home", "Up", "End", "PageUp"],
  ["Keys", "Shift", "Backspace", "Enter", "Left", "Down", "Right", "PageDown"],
]
```

To use a private relay on Android, open **Settings → Network** and set **Relay URL** to an `http` or `https` URL with a host. Leave it blank to use automatic public relay selection. The setting is saved as `relay_url` under `[native]` in `files/node.toml` and takes effect after restarting the app.

The terminal header shows the active path. Public relays use Android's certificate verifier. The APK includes the verifier's network security resource so Android can retrieve signed revocation lists from the certificate authorities listed by the verifier.

## Build and develop

CI builds use Rust 1.98.1. The default workspace build produces the headless CLI and gateway. The desktop and Android apps are separate packages that share one pinned GPUI core.

```sh
cargo build --locked
cargo build --locked -p paxri-gui
target/debug/paxri --help
target/debug/paxri config check examples/home.toml
```

Linux desktop builds require development libraries for Fontconfig, FreeType, XKB (including X11), Wayland, X11 and Vulkan. A graphical session and a working Vulkan driver are required to open the GUI. For the Windows desktop use MSVC Build Tools and the Windows SDK. macOS desktop builds use Xcode and its Metal tools. The CI workflow lists the Linux packages.

Run the checks that cover the change, for example `cargo test --locked -p paxri-files` or `python3 scripts/check-gateway.py --file-copy` after building the headless binaries. To keep a Windows checkout in sync from Linux, use `python3 scripts/sync-windows.py --host WINDOWS_SSH_HOST --watch --dev`. [Development and architecture](docs/architecture.md)

## Documentation

- [Installation and downloads](docs/installation.md)
- [Architecture and desktop development](docs/architecture.md)
- [NAT traversal, IPv6, relays, and reproducible network tests](docs/connectivity.md)
- [Configuration and native wire protocol](docs/native-protocol.md)
- [Build and distribute paxri](docs/builds.md)
- [Contributor and coding-agent guide](AGENTS.md)
