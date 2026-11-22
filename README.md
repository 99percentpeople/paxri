# PeerHatch

PeerHatch is a prototype for opening shells, copying files, and forwarding TCP ports between paired computers across networks. Its Rust CLI and GPUI desktop app use the same background gateway and TOML configuration.

The gateway establishes an authenticated iroh connection to each peer. Terminal sessions, file transfers, and forwards each open their own stream on that connection. Peers try direct IPv4 and IPv6 paths and can use an encrypted relay when direct UDP is unavailable.

## Try the prototype

Download the archive for your OS from the rolling **PeerHatch prototype** prerelease. Each successful `main` build updates the same page. Builds are available for Windows x64, Linux AMD64 and ARM64, and macOS Intel and Apple Silicon. Extract the archive and keep the three programs together. The CLI is `bin/peerhatch` on Unix and `peerhatch.exe` on Windows. [Download the prototype](https://github.com/99percentpeople/peerhatch/releases/tag/prototype)

For a headless Linux node, run these commands in the directory containing [compose.yaml](compose.yaml).

```sh
docker compose up -d --wait
docker compose exec gateway peerhatch pair code
```

Compose pulls `ghcr.io/99percentpeople/peerhatch:dev`. You can also pin an image by commit or load it offline. [Installation](docs/installation.md)

Linux archives come in glibc builds for AMD64 and ARM64 and a musl build for ARM64. All three include the CLI, gateway, and GUI. On an ARM64 router you can extract just the CLI and gateway from the musl archive. [Headless installation](docs/installation.md#headless-arm64-musl-installation)

## Pair two computers

Open **PeerHatch** on macOS or run `peerhatch-gui` on either desktop. In **Devices**, choose **Add Device** at the end of the device list to see the local pairing code or enter one from another device. Enter the first computer’s code on the second, then approve the request on the first. Share the code with the person who controls the other device. Approval grants shell and file access as the gateway account. Paired devices can connect to TCP addresses reachable by the gateway and bind TCP listeners with the gateway account’s privileges.

The CLI performs the same steps. On **both** computers, start the gateway if it is idle:

```sh
peerhatch gateway start
```

| Where | Command | Result |
| --- | --- | --- |
| First computer | `peerhatch pair code` | Prints the current invitation |
| Second computer | `peerhatch pair join "PAIRING_CODE"` | Sends an approval request |
| First computer | `peerhatch pair status` | Lists the requesting device and its ID |
| First computer | `peerhatch pair accept DEVICE_ID` | Saves the pairing on both devices |
| Either computer | `peerhatch pair remove DEVICE` | Revokes local access immediately and requests confirmation from the other gateway |
| Either computer | `peerhatch status` | Lists peers, short IDs, streams, and forwarding rules |

The pairing code survives restarts. `peerhatch pair refresh` replaces it and dismisses requests made with the old code. Peer commands accept the displayed device name, the full public ID, or a unique hexadecimal prefix of at least four characters.

To unpair in the desktop app, open **Devices**, select the device, and choose **Unpair device**. Unpairing works offline. It removes local aliases and forwarding rules for that identity and revokes its access to the gateway’s TCP targets and reverse listeners. An online peer removes the corresponding pairing and confirms. For an offline peer the gateway saves the notification and retries after reconnects or restarts. Both gateways need unpair support for automatic confirmation. `peerhatch pair status` shows pending confirmations. Pairing again requires a new approval.

## Use a paired device

The examples use a device named `home`. Substitute the name or short ID printed by `peerhatch status`.

```sh
peerhatch status home --check
peerhatch shell home
peerhatch exec home -- vim "notes from today.txt"
peerhatch exec home -c 'uname -s && uptime'
peerhatch exec --pipe home -- cat image.png > copy.png
peerhatch cp build.zip home:Downloads/
peerhatch cp -rp home:projects/demo ./backups/
```

**Terminal.** The gateway starts a native PTY under its own account. Windows prefers PowerShell 7 and falls back to Windows PowerShell 5.1. On Unix it uses the account’s shell. `exec` preserves terminal input, resize events, and the remote exit status. Captured output can contain both output streams and terminal escape sequences. A redirected session starts at 80 columns by 24 rows. An unprivileged gateway can host terminal and file sessions.

Inside a remote terminal, `peerhatch status` queries the gateway hosting that terminal. From the initiating computer, `peerhatch exec home -- peerhatch status` does the same. Keep the CLI beside the gateway executable. To add commands for this device, put executables in `tools/bin/` beside its `node.toml`. [Session tools and examples](docs/installation.md#remote-terminal-tools)

Use `exec --pipe` for scripts and binary tools. It streams stdin, stdout and stderr separately, forwards stdin EOF, and returns the remote exit code. Both gateways must support this mode. With a screenshot tool installed on the peer, an application can read its PNG directly from stdout. Use a binary-safe caller such as Python’s `subprocess.run(..., capture_output=True)`.

**Files.** `peerhatch cp` transfers in either direction. `peer:path` is relative to the gateway user’s home directory unless the path is absolute. Use `-r` for directories, `-p` for timestamps and supported Unix permission bits, `-n` to skip existing files, `-v` for per-file reporting, and `-q` for errors only. Multiple sources require an existing destination directory. An interrupted file keeps its previous contents. Completed files in the same transfer remain on disk.

**Port forwarding.** In **Devices → Port forwarding**, choose **Add rule**, select a paired device, and enter the listening port and target. A bare port uses IPv4 loopback. **Forward** listens on this computer and connects to the target on the peer. **Reverse** listens on the peer and connects to the target on this computer.

```sh
peerhatch forward add web --peer home --listen 8080 --target 3000
peerhatch forward add preview --peer home --remote --listen 9000 --target 5000
peerhatch forward list
```

Open `http://127.0.0.1:8080` on this computer to reach port 3000 on `home`. Both devices can edit, pause, resume or remove their shared rule. The creating gateway stores it. From the other device, use the owner in the list, for example `peerhatch forward pause web --owner laptop`. Locally stored rules remain visible when the peer goes offline. Rules stored on the peer appear while it is connected. To open a TCP stream directly on stdin/stdout, use `peerhatch connect home 3000`. [Example configuration](examples/laptop.toml)

**Desktop.** Select a peer in the **Devices** sidebar to see its connection and terminal sessions. **Port forwarding** lists rules for all devices. Filter by device, add a rule, or open a row’s menu to edit, pause, resume, or remove it. Use **+** to open another terminal. Tabs can be renamed, reordered, and detached. Settings includes application appearance, terminal colors, cursor, and gateway startup. `peerhatch-gui` opens Devices without arguments and accepts the same commands as `peerhatch` when launched from a terminal.

## Build and develop

The prototype builds use Rust 1.98.1. The default workspace build produces the headless CLI and gateway. The desktop app is a separate package.

```sh
cargo build --locked
cargo build --locked -p peerhatch-gui
target/debug/peerhatch --help
target/debug/peerhatch config check examples/home.toml
```

Linux desktop builds require development libraries for Fontconfig, FreeType, XKB (including X11), Wayland, X11 and Vulkan. A graphical session and a working Vulkan driver are required to open the GUI. For the Windows desktop use MSVC Build Tools and the Windows SDK. macOS desktop builds use Xcode and its Metal tools. The CI workflow lists the Linux packages.

Run the checks that cover the change, for example `cargo test --locked -p peerhatch-files` or `python3 scripts/check-gateway.py --file-copy` after building the headless binaries. To keep a Windows checkout in sync from Linux, use `python3 scripts/sync-windows.py --host WINDOWS_SSH_HOST --watch --dev`. [Development and architecture](docs/architecture.md)

## Documentation

- [Installation and prototype downloads](docs/installation.md)
- [Architecture and desktop development](docs/architecture.md)
- [NAT traversal, IPv6, relays, and reproducible network tests](docs/connectivity.md)
- [Configuration and native wire protocol](docs/native-protocol.md)
- [Build and distribute a prototype](docs/prototype-builds.md)
