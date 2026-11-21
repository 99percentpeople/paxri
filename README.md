# PeerHatch

PeerHatch is a prototype for opening shells, copying files, and forwarding TCP ports between paired computers across networks. Its Rust CLI and GPUI desktop app use the same background gateway and TOML configuration.

The gateway establishes an authenticated iroh connection to each peer. Terminal sessions, file transfers, and forwards each open their own stream on that connection. Peers try direct IPv4 and IPv6 paths and can use an encrypted relay when direct UDP is unavailable.

## Try the prototype

Sign in to GitHub, open a successful **Prototype builds** run on `main`, and download the artifact for your OS from the **Artifacts** section. Builds are available for Windows x64, Linux AMD64 and ARM64, and macOS Intel and Apple Silicon. Extract the archive inside the artifact and keep the three programs together. The CLI is `bin/peerhatch` on Unix and `peerhatch.exe` on Windows. [Download a prototype build](https://github.com/99percentpeople/peerhatch/actions/workflows/prototype.yml)

For a headless Linux node, run these commands in the directory containing [compose.yaml](compose.yaml).

```sh
docker compose up -d --wait
docker compose exec gateway peerhatch pair code
```

Compose pulls `ghcr.io/99percentpeople/peerhatch:dev`. You can also pin an image by commit or load it offline. [Installation](docs/installation.md)

Linux archives come in glibc builds for AMD64 and ARM64 and a musl build for ARM64. All three include the CLI, gateway, and GUI. On an ARM64 router you can extract just the CLI and gateway from the musl archive. [Headless installation](docs/installation.md#headless-arm64-musl-installation)

## Pair two computers

Open **PeerHatch** on macOS or run `peerhatch-gui` on either desktop. **Home** shows the local pairing code and accepts a code from another device. Enter the first computer’s code on the second, then approve the request on the first. Share the code with the person who controls the other device. Approval grants shell and file access as the gateway account. TCP targets and listeners have separate permissions.

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
| Either computer | `peerhatch status` | Lists peers, short IDs, streams, and forwarding rules |

The pairing code survives restarts. `peerhatch pair refresh` replaces it and dismisses requests made with the old code. Peer commands accept the displayed device name, the full public ID, or a unique hexadecimal prefix of at least four characters.

## Use a paired device

The examples use a device named `home`. Substitute the name or short ID printed by `peerhatch status`.

```sh
peerhatch status home --check
peerhatch shell home
peerhatch exec home -- vim "notes from today.txt"
peerhatch exec home -c 'uname -s && uptime'
peerhatch cp build.zip home:Downloads/
peerhatch cp -rp home:projects/demo ./backups/
```

**Terminal.** The gateway starts a native PTY under its own account. Windows prefers PowerShell 7 and falls back to Windows PowerShell 5.1. On Unix it uses the account’s shell. `exec` preserves terminal input, resize events, and the remote exit status. Captured output can contain both output streams and terminal escape sequences. A redirected session starts at 80 columns by 24 rows. An unprivileged gateway can host terminal and file sessions.

**Files.** `peerhatch cp` transfers in either direction. `peer:path` is relative to the gateway user’s home directory unless the path is absolute. Use `-r` for directories, `-p` for timestamps and supported Unix permission bits, `-n` to skip existing files, `-v` for per-file reporting, and `-q` for errors only. Multiple sources require an existing destination directory. An interrupted file keeps its previous contents. Completed files in the same transfer remain on disk.

**TCP forwarding.** On the machine running a web server on port 3000, set the explicit TCP allowlist for the paired device named `laptop`. On the laptop, create the listener:

```sh
# On home
peerhatch port-access set laptop --connect 127.0.0.1:3000

# On laptop
peerhatch forward add home-web --peer home --listen 8080 --target 3000
peerhatch forward list
```

Open `http://127.0.0.1:8080` on the laptop. For a listener on the other computer, authorize `listen` on that computer and pass `--remote` when creating the rule. A named service can also be opened directly on stdin/stdout with `peerhatch connect home dev-web`. Examples for both ends are in [home.toml](examples/home.toml) and [laptop.toml](examples/laptop.toml).

**Desktop.** Open a paired device to see the active path, terminal sessions, and port forwarding. Use **+** to open another terminal. Tabs can be renamed, reordered, and detached. Settings includes application appearance, terminal colors, cursor, and gateway startup. `peerhatch-gui` opens Home without arguments and accepts the same commands as `peerhatch` when launched from a terminal.

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
