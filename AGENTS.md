# AGENTS.md

Guidance for coding agents working in this repository. Read [README.md](README.md) for what paxri does and [docs/architecture.md](docs/architecture.md) for how it fits together.

## Project at a glance

paxri pairs computers and opens shells, copies files, and forwards TCP ports between them over authenticated iroh connections. On desktop and servers, every device runs one **gateway** (`paxri-gateway`), and the CLI (`paxri`) and GPUI desktop app (`paxri-gui`) talk to it over local IPC (a Unix domain socket, or a named pipe on Windows); settings live in one TOML file (`node.toml`) and the private key in a separate file. The Android client embeds the iroh transport in-process without a separate gateway daemon and stores its UI settings in its private `files/gui.toml`.

## Repository layout

| Path | Contents |
| --- | --- |
| `apps/paxri-gateway` | Gateway daemon: pairing, access checks, PTY hosting, forwarding listeners, settings, IPC server |
| `apps/paxri-cli` | `paxri` binary. Its command definitions are shared with the GUI |
| `apps/paxri-gui` | Desktop GPUI app: dock and tabs, pages (`src/pages`), settings, terminal views |
| `crates/paxri-config` | TOML schema, editing (`file.rs`), GUI and gateway preferences, shortcuts |
| `crates/paxri-control` | IPC framing, the `Command` enum and the `Status` struct |
| `crates/paxri-core` | Transport-independent sessions, streams, forwarding |
| `crates/paxri-native` | iroh identities, discovery, relays, direct paths |
| `crates/paxri-terminal`, `crates/paxri-files` | PTY and file-transfer protocols |
| `crates/paxri-ui` | GPUI components, terminal emulator and renderer, settings catalog. Shared by desktop **and** Android |
| `compat/{gpui-pre*,iroh}` | Vendored, patched crates wired in through `[patch.crates-io]`. Each package has a `LOCAL_CHANGES.md` |
| `mobile/android` | Android client: shared GPUI core, `gpui-mobile` platform host, Java glue, and Android-specific patches in `patches/` |
| `scripts/` | Integration checks (`check-gateway.py`, `check-nat*.{py,sh}`) and Windows dev sync |
| `packaging/` | Release archives, Windows portable ZIP, systemd and launchd units, Docker entrypoint, rolling-release publishing |
| `docs/` | User and developer documentation |
| `examples/` | Example `node.toml` files. Validate them with `paxri config check` |

## Build and test

The toolchain is Rust 1.98.1 (edition 2024, resolver 3), the same version CI and the Dockerfile use. Always pass `--locked`.

```sh
cargo build --locked                       # headless CLI + gateway (default members)
cargo build --locked -p paxri-gui          # desktop app (needs Fontconfig/FreeType/XKB/Wayland/X11/Vulkan dev libs on Linux)
cargo test --locked -p paxri-config -p paxri-core -p paxri-files -p paxri-native -p paxri-terminal --lib   # CI unit tests
cargo test --locked -p paxri-config --tests   # config & example integration tests (skipped by --lib)
cargo test --locked -p paxri-ui --lib      # shared terminal and UI unit tests
python3 scripts/check-gateway.py --file-copy   # integration checks; need target/debug binaries
python3 scripts/check-gateway.py --pipe-exec
target/debug/paxri config check examples/home.toml
```

Other `check-gateway.py` modes: `--pairing-only`, `--unpair`, `--native-terminal`, `--session-tools`, `--forwarding`, `--lifecycle`. Run the ones that cover your change.

Android shares the root `Cargo.lock` but is not a default workspace member. From the repository root, run `python3 mobile/android/build.py` (needs JDK 17, SDK 35, NDK, and `cargo-ndk` or an explicit Android linker). The script selects `-p paxri-android` for `aarch64-linux-android` and uses `android-dev` by default or `android-release` with `--unsigned-release`. Desktop and Android build `crates/paxri-ui` against the same `gpui-pre` core version pinned in the root manifest. Keep `cfg` for actual platform behaviors, and make sure both clients compile.

## Conventions

- **Formatting.** The tree is not `rustfmt`-clean. Do **not** run `cargo fmt` on the whole workspace, because it rewrites hundreds of unrelated files. Match the surrounding style and keep diffs limited to the code you change.
- **Wire and config compatibility.** Peers can run different builds. New `Status` fields and config keys use `#[serde(default)]`. Gateways advertise capabilities, so check them before relying on a new peer feature (as `exec --pipe` and unpair confirmation do). Config structs use `deny_unknown_fields`, so a new key must be added to the schema in `crates/paxri-config`.
- **Config edits** go through `crates/paxri-config/src/file.rs` (`set_gateway`, `set_gui`, …) with `toml_edit`. These functions preserve user comments and write atomically. Don't serialize the whole file again.
- **New preferences.** Declare a `SettingSpec` in `crates/paxri-ui/src/settings/catalog.rs`, update the config schema (`crates/paxri-config/src/gui.rs` or `startup.rs`) and `file.rs` with round-trip tests (including `crates/paxri-config/tests/gui.rs` for GUI settings), add a `GuiChange`/`GatewayChange` variant handled in the gateway's `Command` handler, and wire shared GUI preferences in `mobile/android/src/settings.rs` (`files/gui.toml`). The settings-editor paragraphs under "Desktop workspace" in `docs/architecture.md` describe how this fits together.
- **Vendored crates.** If you change `compat/*` or `mobile/android/patches/*`, update the matching `LOCAL_CHANGES.md` or `patches/README.md`. Android patches must also be listed in `PATCH_SERIES` in `mobile/android/build.py`.
- **Platforms.** Code runs on Linux, macOS, Windows (ConPTY, named pipes, services) and Android. Put platform-specific code behind `cfg` and keep the other targets compiling. For shortcuts, use `Modifiers::secondary_key()` (Ctrl, or Cmd on macOS). In Windows releases, keep `conpty.dll` and `OpenConsole.exe` in the same directory as `paxri-gateway.exe`.
- **Security.** Pairing grants shell, file and TCP access as the gateway account. Keep access checks in the gateway, not in the clients. Terminal links are restricted to `http`, `https` and `mailto` (see `sanitize_uri`). Don't weaken this kind of filtering.
- **Commits.** Use `type(scope): summary`, for example `feat(ui,gateway): …` or `fix(android,terminal): …`. Common scopes are `ui`, `gateway`, `terminal`, `android`, `packaging`, `build`. The body is a short bullet list.
- **Secrets.** Never commit `node.toml`, `*.key`, `*.p12`/`*.pfx` or `.env*`. They are gitignored. `target/` and `dist/` are build outputs.

## Documentation

User-visible changes must update the docs in the same change. That means `README.md` for features and UI, `docs/installation.md` for packaging and install, `docs/native-protocol.md` for config and wire format, `docs/connectivity.md` for networking, and `docs/builds.md` for CI and artifacts. Match the existing style: plain English in short declarative sentences, UI labels in **bold**, and keys, paths and commands in `code`. Release archives place `README.md` and `installation.md` at the archive root and rewrite their Markdown links for standalone reading (see `packaging/build-unix-release.sh` and `packaging/windows/build-portable.py`).
