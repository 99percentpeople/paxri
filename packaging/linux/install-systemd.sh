#!/bin/sh
set -eu

if [ "$#" -gt 1 ]; then
    echo "Usage: $0 [release-bin-directory]" >&2
    exit 2
fi
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
bin_dir=${1:-"$here/../../target/release"}
for binary in tunnel tunnel-gateway; do
    if [ ! -x "$bin_dir/$binary" ]; then
        echo "Missing $bin_dir/$binary. Build with cargo build --locked --release -p tunnel-gateway -p tunnel-cli first." >&2
        exit 1
    fi
done
if [ "$(id -u)" != 0 ]; then
    echo "Run this installer as root to register the system service." >&2
    exit 1
fi

getent group p2p-tunnel >/dev/null || groupadd --system p2p-tunnel
getent passwd p2p-tunnel >/dev/null || useradd --system --gid p2p-tunnel --no-create-home \
    --home-dir /var/lib/p2p-tunnel --shell /bin/bash p2p-tunnel
install -d -m 0700 -o p2p-tunnel -g p2p-tunnel /etc/p2p-tunnel
install -D -m 0755 -t /usr/local/bin "$bin_dir/tunnel" "$bin_dir/tunnel-gateway"
install -D -m 0755 "$here/start-gateway" /usr/local/libexec/p2p-tunnel/start-gateway
install -D -m 0644 "$here/p2p-tunnel-gateway.service" /etc/systemd/system/p2p-tunnel-gateway.service
systemctl daemon-reload
systemctl enable p2p-tunnel-gateway.service
systemctl restart p2p-tunnel-gateway.service
