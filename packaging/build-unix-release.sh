#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
case "$(uname -s)/$(uname -m)" in
  Linux/x86_64) platform=linux-amd64 ;;
  Linux/aarch64) platform=linux-arm64 ;;
  Darwin/x86_64) platform=macos-amd64 ;;
  Darwin/arm64) platform=macos-arm64 ;;
  *) echo 'Unsupported release platform' >&2; exit 1 ;;
esac
cargo build --locked --release -p peerhatch-cli -p peerhatch-gateway -p peerhatch-gui
bins="${CARGO_TARGET_DIR:-target}/release"
"$bins/peerhatch" --version
"$bins/peerhatch-gateway" --help >/dev/null
"$bins/peerhatch-gui" --help >/dev/null
mkdir -p target/dist
work=$(mktemp -d)
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
tar -czf "target/dist/$name.tar.gz" -C "$work" "$name"
