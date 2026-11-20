# Build and distribute a prototype

Each build is identified by its Git commit. Cargo uses `0.0.0-dev` as the workspace package version. The packaging scripts use Cargo's optimized `--release` profile.

## Run the workflow

The regular CI workflow checks pushes to `main` and pull requests. When you want packages to share, open **Actions → Prototype builds → Run workflow**, choose a branch, and start the build. From a checkout you can run the same workflow with the GitHub CLI. [Prototype builds](https://github.com/99percentpeople/peerhatch/actions/workflows/prototype.yml)

```sh
gh workflow run prototype.yml --ref main
gh run list --workflow prototype.yml --branch main
```

## Download a build

Open a successful run and download the artifact for your OS and CPU. The `peerhatch-build-info` artifact contains `SHA256SUMS`, `compose.yaml`, and a `build.env` file that pins the gateway image to that commit. All files used together should come from the same run.

For example, from the repository checkout, replace `RUN_ID` with the run ID from the list above. These commands download and verify the Linux ARM64 archive.

```sh
gh run download RUN_ID -n peerhatch-linux-arm64 -D download
gh run download RUN_ID -n peerhatch-build-info -D download
(cd download && sha256sum -c --ignore-missing SHA256SUMS)
```

## Docker images

Every run uploads offline Docker images for Linux AMD64 and ARM64. A successful run on `main` also publishes a multi-platform image to GHCR. Use `ghcr.io/99percentpeople/peerhatch:dev` for the most recently packaged main build. Use `sha-<full Git commit>` or the run's `build.env` to select a particular build. [Installation and offline loading](installation.md)
