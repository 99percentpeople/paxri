#!/bin/sh
# Run inside the release matrix's native Debian or Alpine Rust image.
set -eu
if [ -f /etc/alpine-release ]; then
    apk add --no-cache bash build-base clang cmake pkgconf fontconfig-dev freetype-dev \
        libxkbcommon-dev wayland-dev vulkan-loader-dev libx11-dev libxi-dev
else
    apt-get update -q
    apt-get install -y --no-install-recommends clang cmake pkg-config libfontconfig1-dev \
        libfreetype-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
        libvulkan-dev libx11-xcb-dev libxi-dev
fi
exec bash packaging/build-unix-release.sh
