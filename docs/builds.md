# Build and distribute paxri

Each build is identified by its Git commit. Cargo uses `0.0.0-dev` as the workspace package version. Native archives and Docker images use the `distribution` Cargo profile. It optimizes for size, uses full link-time optimization with one codegen unit per crate, and strips symbols. For a local native archive on Linux or macOS, run `bash packaging/build-unix-release.sh`. [Profile settings](../packaging/distribution.toml)

## Run the workflow

Every push to `main` runs the native tests and builds the desktop archives, Android APK, and Docker images in one workflow. After all jobs succeed, the `rolling` prerelease publishes the packages, their checksums, and the Compose files for the same commit. Pull requests run the native tests and build the Android APK. [Rolling build](../../../releases/tag/rolling)

You can also start **Build** manually. A manual run on another branch uploads the packages as Actions artifacts. [Build runs](../../../actions/workflows/builds.yml)

```sh
gh workflow run builds.yml --ref main
gh run list --workflow builds.yml --branch main
```

## Download a build

Download the package for your OS and CPU from the rolling prerelease or an Actions run. Actions serves the original `.zip`, `.tar.gz`, or `.apk` file directly. The `paxri-build-info` artifact groups `SHA256SUMS`, `compose.yaml`, and the `build.env` file that pins the gateway image to the same commit.

For example, to download and verify the current ARM64 musl package from the repository checkout:

```sh
gh release download rolling --pattern paxri-linux-arm64-musl.tar.gz --pattern SHA256SUMS --dir download
(cd download && sha256sum -c --ignore-missing SHA256SUMS)
```

## Docker images

Pushes to `main` and manual runs upload offline Docker images for Linux AMD64 and ARM64. A successful run on `main` also publishes a multi-platform image to GHCR. Use `ghcr.io/99percentpeople/paxri:dev` for the most recently packaged main build. Use `sha-<full Git commit>` or the run's `build.env` to select a particular build. [Installation and offline loading](installation.md)
