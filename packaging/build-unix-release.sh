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
    -p peerhatch-cli -p peerhatch-gateway -p peerhatch-gui
bins="${CARGO_TARGET_DIR:-target}${CARGO_BUILD_TARGET:+/$target}/distribution"
if [[ $platform == linux-arm64-musl ]]; then
    for bin in peerhatch peerhatch-gateway peerhatch-gui; do
        elf=$(LC_ALL=C readelf -lW "$bins/$bin")
        if [[ $elf != *'/lib/ld-musl-aarch64.so.1]'* ]]; then
            echo "Expected the ARM64 musl loader in $bin" >&2
            exit 1
        fi
    done
fi
if [[ $target == "$host" ]]; then
    "$bins/peerhatch" --version
    "$bins/peerhatch-gateway" --help >/dev/null
    "$bins/peerhatch-gui" --help >/dev/null
fi
mkdir -p target/dist
work=$(mktemp -d target/dist/.native.XXXXXX)
trap 'rm -rf -- "$work"' EXIT
name=peerhatch-$platform
root=$work/$name
mkdir -p "$root/bin"
if [[ $platform == macos-* ]]; then
    app="$root/PeerHatch.app/Contents"
    mkdir -p "$app/MacOS"
    version=$("$bins/peerhatch" --version | awk '{print $2}')
    # Apple's bundle version fields take the numeric part of the Cargo version.
    version=${version%%[-+]*}
    cat > "$app/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>PeerHatch</string>
<key>CFBundleIdentifier</key><string>org.peerhatch.desktop</string>
<key>CFBundleExecutable</key><string>peerhatch-gui</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundleVersion</key><string>$version</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
    for bin in peerhatch peerhatch-gateway peerhatch-gui; do
        install -m 0755 "$bins/$bin" "$app/MacOS/$bin"
        codesign --force --sign - "$app/MacOS/$bin"
        ln -s "../PeerHatch.app/Contents/MacOS/$bin" "$root/bin/$bin"
    done
    codesign --force --sign - "$root/PeerHatch.app"
    codesign --verify --deep --strict "$root/PeerHatch.app"
else
    for bin in peerhatch peerhatch-gateway peerhatch-gui; do install -m 0755 "$bins/$bin" "$root/bin/$bin"; done
    cp packaging/linux/{start-gateway,install-systemd.sh,peerhatch-gateway.service} "$root/"
fi
cp README.md docs/installation.md "$root/"
tar -cf - -C "$work" "$name" | gzip -9n > "$work/$name.tar.gz"
mv "$work/$name.tar.gz" "target/dist/$name.tar.gz"
