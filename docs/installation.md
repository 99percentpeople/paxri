# Install paxri

Download your system's package and `SHA256SUMS` from the **paxri rolling build** prerelease. The release notes identify the source commit. Save both files in the same directory. On Linux run `sha256sum -c --ignore-missing SHA256SUMS`. On Windows compare `Get-FileHash -Algorithm SHA256 FILE` with the entry for that file in `SHA256SUMS`. [Download the rolling build](../../../releases/tag/rolling)

| System | Package |
| --- | --- |
| Windows on Intel or AMD | `paxri-windows-x64.zip` |
| AMD64 Linux with glibc (e.g. Debian or Ubuntu) | `paxri-linux-amd64-glibc.tar.gz` |
| ARM64 Linux with glibc (e.g. 64-bit Raspberry Pi OS) | `paxri-linux-arm64-glibc.tar.gz` |
| ARM64 Linux with musl (e.g. Alpine or OpenWrt) | `paxri-linux-arm64-musl.tar.gz` |
| Apple Silicon | `paxri-macos-arm64.tar.gz` |
| Intel Mac | `paxri-macos-amd64.tar.gz` |
| ARM64 Android | `paxri.apk` |

On Windows, extract the whole ZIP into one directory. Keep `conpty.dll` and `OpenConsole.exe` beside the gateway to support mouse input and screen switching in remote terminal applications. Restart the gateway after updating those files.

Every Linux archive includes `bin/paxri`, `bin/paxri-gateway`, and `bin/paxri-gui`. The glibc builds target Debian 12 (glibc 2.36). The musl build targets Alpine 3.23. Opening the GUI requires a graphical session, a Vulkan driver, and the desktop runtime libraries, including Fontconfig and XKB.

## Headless ARM64 musl installation

All three programs in the ARM64 musl archive use the system loader at `/lib/ld-musl-aarch64.so.1`. The CLI and gateway also need `libgcc_s.so.1`, provided by `libgcc` on Alpine and `libgcc1` on OpenWrt 25.12. On a router, extract just these two programs to persistent storage. From an unprivileged account with a writable home directory, start the gateway and print the pairing code.

```sh
bundle=paxri-linux-arm64-musl
tar -xzf "$bundle.tar.gz" "$bundle/bin/paxri" "$bundle/bin/paxri-gateway"
"$bundle/bin/paxri" gateway start
"$bundle/bin/paxri" pair code
```

Keep the gateway account’s configuration directory on persistent storage so that its identity and pairings survive a reboot.

## Desktop and per-user gateway

Extract the complete archive. On Windows double-click `paxri-gui.exe`. On macOS open `paxri.app`. The macOS application is ad-hoc signed and distributed outside the App Store. On Linux run `bin/paxri-gui` in a graphical session. The GUI starts a per-user gateway if needed. Closing it leaves that gateway running. Keep the CLI, GUI, and gateway together so the clients can find the gateway executable.

For a headless per-user installation, run `paxri gateway start`. Use `gateway status`, `stop`, or `restart` to manage it. Commands below assume the directory containing the binaries is on PATH. In PowerShell you can use `.\paxri.exe` in the extracted directory. In a Unix archive you can use `./bin/paxri`.

The CLI and GUI share `node.toml` and a sibling `device.key`. The default directory is `$XDG_CONFIG_HOME/paxri` (usually `~/.config/paxri`) on Linux, `~/Library/Application Support/paxri` on macOS, and `%LOCALAPPDATA%\paxri` on Windows. On upgrade, paxri reuses an existing PeerHatch or P2P Tunnel profile, including its device key and pairings. To inspect or change startup policy use **Settings → Gateway → Startup behavior** or the CLI:

```sh
paxri gateway startup
paxri gateway startup gui
```

| Mode | Startup behavior |
| --- | --- |
| `manual` | Start from the CLI or the desktop |
| `gui` | Start when the desktop app opens, the default |
| `login` | Start when the current user signs in |
| `system` | Start at boot with the configured user identity and OS registration privileges |

In desktop settings, switches and selections save as soon as you change them. The device name and relay URL save when you press Enter or leave the field. Clear the device name to use the computer name, or clear the relay URL to restore automatic relay selection.

In **Settings → Network**, choose whether to allow UPnP, try a physical LAN router, or use a custom relay. Network changes take effect at the next gateway start. Restarting disconnects active sessions. **Open config file** opens the selected node’s TOML with the system default application for that file type, including when its gateway is stopped.

Windows supports the ordinary-user installation without an administrator account. Boot-time registration requires elevation, uses Task Scheduler and stores the startup helper under Program Files. A Windows S4U task has limited access to user-encrypted files and authenticated network shares. On Windows, `paxri gateway status` and **Settings → Runtime → Gateway details** show the running executable and check the actual Task Scheduler registration, even when the gateway is stopped. The report identifies disabled or missing tasks and startup paths pointing to another installation. A client opened from a new folder reuses the running gateway for the same profile. To switch the running executable, use `paxri gateway restart` from the desired folder. To update the login or boot task, select the startup mode again or run `paxri gateway startup login` or `paxri gateway startup system` there. On Linux the app uses desktop autostart or systemd, and on macOS it uses LaunchAgents or LaunchDaemons. A gateway running as root or an elevated administrator rejects remote PTY and file operations.

## Remote terminal tools

On Windows, the default PowerShell opened by paxri uses UTF-8 for console input, console output and text sent to native programs. This applies to interactive shells and `exec -c`, including `--pipe`. A program started explicitly with `exec -- PROGRAM` uses its own encoding. Binary pipe output passes through unchanged.

GUI terminals, `paxri shell`, and both forms of `paxri exec` share the host gateway’s session environment. The gateway adds the directory containing its companion CLI and then the profile’s `tools/bin/` directory to the child’s command search path. The system PATH and shell configuration files stay untouched. A gateway-only installation can still host shells and use tools already on its PATH.

- `PAXRI_IPC` identifies the host gateway’s Unix socket or Windows pipe, including when multiple gateways run on the same computer.
- `PAXRI_CALLER_ID` is the full paired device ID of the computer that opened this session. Inside a Unix terminal, `paxri cp ./build.log "${PAXRI_CALLER_ID}:Downloads/"` sends a file back to that computer. In PowerShell, use `paxri cp .\build.log "${env:PAXRI_CALLER_ID}:Downloads/"`.

To add a tool, place a native executable (or an executable script with a shebang on Unix) in `tools/bin/` next to the host’s active `node.toml`. Create this directory if needed. Open a new session to pick it up. For example, an independently installed `ph-inspect` or `ph-inspect.exe` can be called as `paxri exec home -- ph-inspect`. Arguments and the tool’s exit status follow the normal `exec` behavior.

An extension runs as the gateway account in its OS session. A desktop tool needs access to the intended graphical session and the platform’s screen capture or accessibility permissions. With a PNG-producing `ph-screenshot` installed, a caller can capture its output directly:

```python
import subprocess

png = subprocess.run(
    ["paxri", "exec", "--pipe", "home", "--", "ph-screenshot"],
    capture_output=True, check=True,
).stdout
```

The byte string can be passed directly to an image consumer. Diagnostics go to stderr. The same mode accepts a shell script through `-c`.

## Docker on Linux

The image includes the CLI and gateway. Use the repository's `compose.yaml` or extract it from the `paxri-build-info` artifact. Run Compose in that directory.

```sh
docker compose up -d --wait
docker compose exec gateway paxri status
docker compose exec gateway paxri pair code
docker compose logs -f gateway
```

The default tag is `dev`. To use the same commit as a particular desktop build, run `docker compose --env-file build.env up -d --wait` with the file from that run. You can also set `PAXRI_VERSION=sha-<full Git commit>` in the Compose project's `.env`. To update a running `dev` installation, run `docker compose pull` and `docker compose up -d --wait`. The registry serves AMD64 and ARM64 from the same tag.

Compose uses the host network and hostname on native Linux. A forward to `127.0.0.1` can reach the host’s loopback services. Remote terminals run the container’s shell as UID/GID `10001:10001`. The named volume at `/var/lib/paxri` holds the identity, peers, and configuration. Reuse the same Compose project name across upgrades. Removing the volume removes that device identity. To bring an existing node, stop its old gateway, mount the whole directory containing both `node.toml` and `device.key`, allow UID 10001 to write the directory, and keep the key private.

### Offline or local image

Download the Docker artifact for the target CPU and the `paxri-build-info` artifact from the same Actions run. Extract both into the same directory. For example, on ARM64 run:

```sh
docker load -i paxri-docker-linux-arm64.tar.gz
docker compose --env-file build.env up -d --pull never --wait
```

To build from source and use the same Compose file:

```sh
docker build -t ghcr.io/99percentpeople/paxri:local .
PAXRI_VERSION=local docker compose up -d --pull never --wait
```

### Manage the container from a host client

The simplest management command is `docker compose exec gateway paxri`. To use an installed CLI or GUI on native Linux, share a local socket directory with the gateway:

```sh
mkdir -p "$HOME/.local/share/paxri"
sudo install -d -o 10001 -g "$(id -g)" -m 2750 "$HOME/.local/share/paxri/docker-ipc"
```

Add a bind mount for `${HOME}/.local/share/paxri/docker-ipc:/run/paxri` to the gateway's Compose volumes. Recreate the container, then set `PAXRI_IPC="$HOME/.local/share/paxri/docker-ipc/gateway.sock"` in the host client environment. Members of the socket group can manage this gateway.

## Linux system service

From an extracted Linux archive run:

```sh
sudo ./install-systemd.sh ./bin
sudo -u paxri /usr/local/bin/paxri pair code
journalctl -u paxri-gateway -f
```

From a source checkout, build `cargo --config packaging/distribution.toml build --locked --profile distribution` and run `sudo ./packaging/linux/install-systemd.sh ./target/distribution`. The installer registers `paxri-gateway.service` under a dedicated `paxri` account. It uses `/etc/paxri` for the key and configuration, `/var/lib/paxri` as the home directory, and `/run/paxri/gateway.sock` for local management. Add authorized operators to the `paxri` group and sign in again. To disable boot startup and stop the service, run `sudo systemctl disable --now paxri-gateway`.

When upgrading an existing `peerhatch-gateway.service` with this installer, use the existing service and account names. The installer updates the binaries in place and keeps the service’s configuration and device key.

To run a separate node for your user, give the CLI its own configuration and socket.

```sh
state_dir="$HOME/.local/share/paxri/node"
install -d -m 0700 "$state_dir"
./bin/paxri --ipc "$state_dir/gateway.sock" gateway start --config "$state_dir/node.toml"
./bin/paxri --ipc "$state_dir/gateway.sock" status
./bin/paxri --ipc "$state_dir/gateway.sock" gateway stop --config "$state_dir/node.toml"
```

Pass the same `--ipc` to the CLI. For macOS and Windows system-service templates and OS integration details, see [Architecture](architecture.md).
