# Build and distribute a prototype

Each build is identified by its Git commit. Cargo uses `0.0.0-dev` as the workspace package version. Native archives and Docker images use the `distribution` Cargo profile. It optimizes for size, uses full link-time optimization with one codegen unit per crate, and strips symbols. For a local native archive on Linux or macOS, run `bash packaging/build-unix-release.sh`. [Profile settings](../packaging/distribution.toml)

## Run the workflow

The regular CI workflow checks pushes to `main` and pull requests. When you want packages to share, open **Actions → Prototype builds → Run workflow**, choose a branch, and start the build. From a checkout you can run the same workflow with the GitHub CLI. [Prototype builds](https://github.com/99percentpeople/peerhatch/actions/workflows/prototype.yml)

```sh
gh workflow run prototype.yml --ref main
gh run list --workflow prototype.yml --branch main
```

## Download a build

Open a successful run and download the artifact for your OS, CPU, and, on Linux, libc. Linux publishes `peerhatch-linux-amd64-glibc`, `peerhatch-linux-arm64-glibc`, and `peerhatch-linux-arm64-musl`. Each contains the CLI, gateway, and GUI under `bin/`, dynamically linked to the selected libc. The `peerhatch-build-info` artifact contains `SHA256SUMS`, `compose.yaml`, and a `build.env` file that pins the gateway image to that commit. All files used together should come from the same run.

For example, from the repository checkout, replace `RUN_ID` with the run ID from the list above. These commands download and verify the ARM64 musl archive.

```sh
gh run download RUN_ID -n peerhatch-linux-arm64-musl -D download
gh run download RUN_ID -n peerhatch-build-info -D download
(cd download && sha256sum -c --ignore-missing SHA256SUMS)
```

## Docker images

Every run uploads offline Docker images for Linux AMD64 and ARM64. A successful run on `main` also publishes a multi-platform image to GHCR. Use `ghcr.io/99percentpeople/peerhatch:dev` for the most recently packaged main build. Use `sha-<full Git commit>` or the run's `build.env` to select a particular build. [Installation and offline loading](installation.md)
