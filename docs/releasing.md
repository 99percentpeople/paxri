# Build and publish a release

PeerHatch takes its version from `[workspace.package].version` in the root `Cargo.toml`. The [release workflow](../.github/workflows/release.yml) builds optimized binaries on native runners and exports Docker images on Linux AMD64 and ARM64.

## Preview builds

Run **Actions → Release → Run workflow** on a branch to build the packages and download them from the workflow’s **Artifacts** section. Alternatively use `gh workflow run release.yml --ref main`. Branch previews produce the same types of archives as a version tag.

## Publish a version

1. Set the workspace version, update `Cargo.lock`, and run the affected checks.
2. Commit and push the intended source to `main`.
3. Create an annotated tag matching the manifest version and push the tag. For example:

```sh
git tag -a v0.1.0 -m "PeerHatch v0.1.0"
git push origin v0.1.0
```

The workflow attaches the Windows ZIP, four Linux/macOS archives, two Docker image archives, `compose.yaml`, and `SHA256SUMS` to the GitHub Release. The per-architecture Docker jobs smoke-test the CLI and running gateway before uploading their artifacts. The final publish job combines both image architectures into `ghcr.io/OWNER/peerhatch:VERSION` and `:latest`. It uses the repository’s `GITHUB_TOKEN` with job-level package and release write permissions.

For a new GHCR package, its owner controls the package visibility independently of the repository. The package settings page offers **Change visibility**. [GitHub package visibility](https://docs.github.com/en/packages/learn-github-packages/configuring-a-packages-access-control-and-visibility)

To check a published release, inspect `gh release view v0.1.0`, use `docker buildx imagetools inspect ghcr.io/99percentpeople/peerhatch:0.1.0`, and compare the downloaded assets against `SHA256SUMS`. Artifacts from Actions are available for the configured retention period. The release assets are the versioned download links.
