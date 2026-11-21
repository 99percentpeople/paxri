# Install the PeerHatch prototype

Download your system's archive and `SHA256SUMS` from the **PeerHatch prototype** prerelease. The release notes identify the source commit. Save both files in the same directory. On Linux run `sha256sum -c --ignore-missing SHA256SUMS`. On Windows compare `Get-FileHash -Algorithm SHA256 FILE` with the entry for that file in `SHA256SUMS`. [Prototype downloads](https://github.com/99percentpeople/peerhatch/releases/tag/prototype)

| System | Archive |
| --- | --- |
| Windows on Intel or AMD | `peerhatch-windows-x64.zip` |
| AMD64 Linux with glibc (e.g. Debian or Ubuntu) | `peerhatch-linux-amd64-glibc.tar.gz` |
| ARM64 Linux with glibc (e.g. 64-bit Raspberry Pi OS) | `peerhatch-linux-arm64-glibc.tar.gz` |
| ARM64 Linux with musl (e.g. Alpine or OpenWrt) | `peerhatch-linux-arm64-musl.tar.gz` |
| Apple Silicon | `peerhatch-macos-arm64.tar.gz` |
| Intel Mac | `peerhatch-macos-amd64.tar.gz` |

Every Linux archive includes `bin/peerhatch`, `bin/peerhatch-gateway`, and `bin/peerhatch-gui`. The glibc builds target Debian 12 (glibc 2.36). The musl build targets Alpine 3.23. Opening the GUI requires a graphical session, a Vulkan driver, and the desktop runtime libraries, including Fontconfig and XKB.

## Headless ARM64 musl installation

All three programs in the ARM64 musl archive use the system loader at `/lib/ld-musl-aarch64.so.1`. The CLI and gateway also need `libgcc_s.so.1`, provided by `libgcc` on Alpine and `libgcc1` on OpenWrt 25.12. On a router, extract just these two programs to persistent storage. From an unprivileged account with a writable home directory, start the gateway and print the pairing code.

```sh
bundle=peerhatch-linux-arm64-musl
tar -xzf "$bundle.tar.gz" "$bundle/bin/peerhatch" "$bundle/bin/peerhatch-gateway"
"$bundle/bin/peerhatch" gateway start
"$bundle/bin/peerhatch" pair code
```

Keep the gateway account’s configuration directory on persistent storage so that its identity and pairings survive a reboot.

## Desktop and per-user gateway

Extract the complete archive. On Windows double-click `peerhatch-gui.exe`. On macOS open `PeerHatch.app`. The macOS application is ad-hoc signed and distributed outside the App Store. On Linux run `bin/peerhatch-gui` in a graphical session. The GUI starts a per-user gateway if needed. Closing it leaves that gateway running. Keep the CLI, GUI, and gateway together so the clients can find the gateway executable.

For a headless per-user installation, run `peerhatch gateway start`. Use `gateway status`, `stop`, or `restart` to manage it. Commands below assume the directory containing the binaries is on PATH. In PowerShell you can use `.\peerhatch.exe` in the extracted directory. In a Unix archive you can use `./bin/peerhatch`.

The CLI and GUI share `node.toml` and a sibling `device.key`. The default directory is `$XDG_CONFIG_HOME/peerhatch` (usually `~/.config/peerhatch`) on Linux, `~/Library/Application Support/peerhatch` on macOS, and `%LOCALAPPDATA%\PeerHatch` on Windows. An existing development node can continue to use its previous configuration directory and identity. To inspect or change startup policy use **Settings → Gateway** or the CLI:

```sh
peerhatch gateway startup
peerhatch gateway startup gui
```

| Mode | Startup behavior |
| --- | --- |
| `manual` | Start from the CLI or the desktop |
| `gui` | Start when the desktop app opens, the default |
| `login` | Start when the current user signs in |
| `system` | Start at boot with the configured user identity and OS registration privileges |

Windows supports the ordinary-user installation without an administrator account. Boot-time registration requires elevation, uses Task Scheduler and stores the startup helper under Program Files. A Windows S4U task has limited access to user-encrypted files and authenticated network shares. Re-register startup if you move the binaries. On Linux the app uses desktop autostart or systemd, and on macOS it uses LaunchAgents or LaunchDaemons. A gateway running as root or an elevated administrator rejects remote PTY and file operations.

## Docker on Linux

The image includes the CLI and gateway. Use the repository's `compose.yaml` or extract it from the `peerhatch-build-info` artifact. Run Compose in that directory.

```sh
docker compose up -d --wait
docker compose exec gateway peerhatch status
docker compose exec gateway peerhatch pair code
docker compose logs -f gateway
```

The default tag is `dev`. To use the same commit as a particular desktop build, run `docker compose --env-file build.env up -d --wait` with the file from that run. You can also set `PEERHATCH_VERSION=sha-<full Git commit>` in the Compose project's `.env`. To update a running `dev` installation, run `docker compose pull` and `docker compose up -d --wait`. The registry serves AMD64 and ARM64 from the same tag.

Compose uses the host network and hostname on native Linux. A forward to `127.0.0.1` can reach the host’s loopback services. Remote terminals run the container’s shell as UID/GID `10001:10001`. The named volume at `/var/lib/peerhatch` holds the identity, peers, and configuration. Reuse the same Compose project name across upgrades. Removing the volume removes that device identity. To bring an existing node, stop its old gateway, mount the whole directory containing both `node.toml` and `device.key`, allow UID 10001 to write the directory, and keep the key private.

### Offline or local image

Download the Docker artifact for the target CPU and the `peerhatch-build-info` artifact from the same Actions run. Extract both into the same directory. For example, on ARM64 run:

```sh
docker load -i peerhatch-docker-linux-arm64.tar.gz
docker compose --env-file build.env up -d --pull never --wait
```

To build from source and use the same Compose file:

```sh
docker build -t ghcr.io/99percentpeople/peerhatch:local .
PEERHATCH_VERSION=local docker compose up -d --pull never --wait
```

### Manage the container from a host client

The simplest management command is `docker compose exec gateway peerhatch`. To use an installed CLI or GUI on native Linux, share a local socket directory with the gateway:

```sh
mkdir -p "$HOME/.local/share/peerhatch"
sudo install -d -o 10001 -g "$(id -g)" -m 2750 "$HOME/.local/share/peerhatch/docker-ipc"
```

Add a bind mount for `${HOME}/.local/share/peerhatch/docker-ipc:/run/peerhatch` to the gateway's Compose volumes. Recreate the container, then set `PEERHATCH_IPC="$HOME/.local/share/peerhatch/docker-ipc/gateway.sock"` in the host client environment. Members of the socket group can manage this gateway.

## Linux system service

From an extracted Linux archive run:

```sh
sudo ./install-systemd.sh ./bin
sudo -u peerhatch /usr/local/bin/peerhatch pair code
journalctl -u peerhatch-gateway -f
```

From a source checkout, build `cargo --config packaging/distribution.toml build --locked --profile distribution` and run `sudo ./packaging/linux/install-systemd.sh ./target/distribution`. The installer registers `peerhatch-gateway.service` under a dedicated `peerhatch` account. It uses `/etc/peerhatch` for the key and configuration, `/var/lib/peerhatch` as the home directory, and `/run/peerhatch/gateway.sock` for local management. Add authorized operators to the `peerhatch` group and sign in again. To disable boot startup and stop the service, run `sudo systemctl disable --now peerhatch-gateway`.

You can also run a separate foreground node from the archive:

```sh
state_dir="$HOME/.local/share/peerhatch/node"
install -d -m 0700 "$state_dir"
PATH="$PWD/bin:$PATH" ./start-gateway "$state_dir" --ipc "$state_dir/gateway.sock"
```

Pass the same `--ipc` to the CLI. For macOS and Windows system-service templates and OS integration details, see [Architecture](architecture.md).
