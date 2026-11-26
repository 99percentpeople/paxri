#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
host=$(rustc -vV | sed -n 's/^host: //p')
target=${CARGO_BUILD_TARGET:-$host}
case "$target" in
  x86_64-unknown-linux-gnu) platform=linux-amd64-glibc ;;
  aarch64-unknown-linux-gnu) platform=linux-arm64-glibc ;;
  aarch64-unknown-linux-musl) platform=linux-arm64-musl ;;
  x86_64-apple-darwin) platform=macos-amd64 ;;
  aarch64-apple-darwin) platform=macos-arm64 ;;
  *) echo "Unsupported distribution target: $target" >&2; exit 1 ;;
esac
cargo --config packaging/distribution.toml build --locked --profile distribution \
    -p paxri-cli -p paxri-gateway -p paxri-gui
bins="${CARGO_TARGET_DIR:-target}${CARGO_BUILD_TARGET:+/$target}/distribution"
if [[ $platform == linux-arm64-musl ]]; then
    for bin in paxri paxri-gateway paxri-gui; do
        elf=$(LC_ALL=C readelf -lW "$bins/$bin")
        if [[ $elf != *'/lib/ld-musl-aarch64.so.1]'* ]]; then
            echo "Expected the ARM64 musl loader in $bin" >&2
            exit 1
        fi
    done
fi
if [[ $target == "$host" ]]; then
    "$bins/paxri" --version
    "$bins/paxri-gateway" --help >/dev/null
    "$bins/paxri-gui" --help >/dev/null
fi
dist=dist/native
mkdir -p "$dist"
work=$(mktemp -d "$dist/.native.XXXXXX")
trap 'rm -rf -- "$work"' EXIT
name=paxri-$platform
root=$work/$name
mkdir -p "$root/bin"
if [[ $platform == macos-* ]]; then
    app="$root/paxri.app/Contents"
    install -d "$app/Resources/licenses"
    install -m 0644 crates/paxri-ui/fonts/OFL.txt "$app/Resources/licenses/terminal-symbols-ofl.txt"
    mkdir -p "$app/MacOS"
    version=$("$bins/paxri" --version | awk '{print $2}')
    # Apple's bundle version fields take the numeric part of the Cargo version.
    version=${version%%[-+]*}
    cat > "$app/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>paxri</string>
<!-- Existing macOS installations use this bundle identifier. -->
<key>CFBundleIdentifier</key><string>org.peerhatch.desktop</string>
<key>CFBundleExecutable</key><string>paxri-gui</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundleVersion</key><string>$version</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
    for bin in paxri paxri-gateway paxri-gui; do
        install -m 0755 "$bins/$bin" "$app/MacOS/$bin"
        codesign --force --sign - "$app/MacOS/$bin"
        ln -s "../paxri.app/Contents/MacOS/$bin" "$root/bin/$bin"
    done
    codesign --force --sign - "$root/paxri.app"
    codesign --verify --deep --strict "$root/paxri.app"
else
    install -d "$root/licenses"
    install -m 0644 crates/paxri-ui/fonts/OFL.txt "$root/licenses/terminal-symbols-ofl.txt"
    for bin in paxri paxri-gateway paxri-gui; do install -m 0755 "$bins/$bin" "$root/bin/$bin"; done
    cp packaging/linux/{install-systemd.sh,paxri-gateway.service} "$root/"
fi
install -d "$root/docs"
install -m 0644 README.md docs/installation.md "$root/docs/"
tar -cf - -C "$work" "$name" | gzip -9n > "$work/$name.tar.gz"
mv "$work/$name.tar.gz" "$dist/$name.tar.gz"
echo "$dist/$name.tar.gz"
