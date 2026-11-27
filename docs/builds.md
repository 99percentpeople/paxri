# Build and distribute paxri

Each build is identified by its Git commit. Cargo uses `0.0.0-dev` as the workspace package version. Native archives and Docker images use the `distribution` Cargo profile. It optimizes for size, uses full link-time optimization with one codegen unit per crate, and strips symbols. [Profile settings](../packaging/distribution.toml)

## Build outputs

| Directory | Contents | Build entry point |
| --- | --- | --- |
| `dist/native/` | Linux and macOS tarballs, Windows ZIP | `bash packaging/build-unix-release.sh` on Linux or macOS, `python packaging/windows/build-portable.py` on Windows |
| `dist/android/` | Signed ARM64 `paxri-android-arm64.apk` | `python3 mobile/android/build.py` for a development build, **Build** on main for release signing |
| `dist/docker/` | Offline image tarballs for Linux AMD64 and ARM64 | **Build** workflow |
| `dist/release/` | All nine packages alongside `SHA256SUMS`, `compose.yaml`, and `build.env` | **Build** workflow after all platform jobs succeed |

Desktop and Android share the workspace-pinned `gpui-pre` core. Linux desktop builds use the wgpu renderer with the X11 and Wayland backends. Windows uses DirectX and macOS uses Metal. Android keeps its `gpui-mobile` backend. [Shared GPUI patches](../compat/gpui-pre/LOCAL_CHANGES.md)

Native CI jobs use `target/ci-native/` on every platform, with separate caches per matrix target. Local native packaging accepts `CARGO_TARGET_DIR`. Cargo output and Android build intermediates live under `target/` by default. The Android builder keeps its development keystore and the output of `--unsigned-release` in `target/android/`.

## Run the workflow

Every push to `main` runs the native tests and builds the desktop archives, Android APK, and Docker images in one workflow. After all jobs succeed, the `rolling` prerelease publishes the packages, their checksums, and the Compose files for the same commit. Pull requests run the native tests and build the Android APK. [Rolling build](../../../releases/tag/rolling)

Different commits can build concurrently. Rebuilds of the same commit on the same ref queue together because they share image tags. Each successful main build publishes a multi-platform image tagged with its commit. Updates to `rolling` and the `dev` image run one at a time. A build that finishes after a newer commit has been published keeps its commit image and skips those updates.

The workflow manages all attachments on the `rolling` prerelease. It verifies every current file against its SHA-256 digest, updates the release, then deletes attachments absent from the new build. Retrying a run resumes an interrupted cleanup.

You can also start **Build** manually. A manual run on another branch uploads the packages as Actions artifacts. [Build runs](../../../actions/workflows/builds.yml)

Both paths collect the platform artifacts once. Collection checks the filenames before generating checksums. When adding a target, update the build matrix and the expected package list together. [Package collection](../.github/actions/collect-build/action.yml)

```sh
gh workflow run builds.yml --ref main
gh run list --workflow builds.yml --branch main
```

## Android APK

Android shares the root `Cargo.lock` with the desktop and headless packages. The APK builder selects `paxri-android` and uses the `android-dev` Cargo profile by default. Rolling APKs use `android-release`, a size-optimized profile for the ARM64 native library.

Before setting up the toolchain, Android CI restores the AAR and JAR cache, checks the files against the SHA-256 digests in [jvm-dependencies.json](../mobile/android/jvm-dependencies.json), and downloads any missing or invalid files. On a cache miss, CI saves the verified files before building the APK. The cache key uses the manifest, so unrelated code or Cargo dependency changes do not invalidate it. The cache does not include build outputs or signing keys.

Run `python3 mobile/android/build.py --fetch-dependencies` to prepare these files locally without the Android SDK or Rust. Update the verifier AAR in the manifest if `rustls-platform-verifier-android` changes in `Cargo.lock`.

The desktop and Android terminals embed the same Cascadia Mono PL subset. Ordinary text uses the selected or system monospace font. Box drawing, blocks, Braille and common prompt symbols use the bundled face. The renderer fits box drawing, block elements and the four basic Powerline separators to the terminal grid at the selected line height and zoom. Other scripts and emoji use the installed system fonts.

To regenerate the subset, run `python3 crates/paxri-ui/fonts/subset.py /path/to/CascadiaMonoPL-Regular.ttf` with fontTools and the static Regular face from Cascadia Code v2407.24. The APK and desktop packages include the SIL Open Font License. [Upstream font release](https://github.com/microsoft/cascadia-code/releases/tag/v2407.24)

The `android-signing` GitHub Actions environment allows only the `main` branch. It stores `ANDROID_KEYSTORE_BASE64` (the base64-encoded PKCS#12 keystore with alias `paxri-release`) and `ANDROID_KEYSTORE_PASSWORD`. Main builds sign the APK and check its certificate SHA-256 against the public fingerprint in the workflow before uploading it. PRs and manual runs on other branches use a development key. To restore release signing in another repository, configure the same secrets from the original keystore and password. [Build workflow](../.github/workflows/builds.yml)

## Download a build

Download the package for your OS and CPU from the rolling prerelease or an Actions run. Actions serves the original `.zip`, `.tar.gz`, or `.apk` file directly. The `paxri-build-info` artifact groups `SHA256SUMS`, `compose.yaml`, and the `build.env` file that pins the gateway image to the same commit.

For example, to download and verify the current ARM64 musl package from the repository checkout:

```sh
gh release download rolling --pattern paxri-linux-arm64-musl.tar.gz --pattern SHA256SUMS --dir download
(cd download && sha256sum -c --ignore-missing SHA256SUMS)
```

## Docker images

Pushes to `main` and manual runs upload offline Docker images for Linux AMD64 and ARM64. A successful run on `main` also publishes a multi-platform image to GHCR. Use `ghcr.io/99percentpeople/paxri:dev` for the most recently packaged main build. Use `sha-<full Git commit>` or the run's `build.env` to select a particular build. [Installation and offline loading](installation.md)
