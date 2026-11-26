#!/bin/sh
set -eu

if [ "$#" -gt 1 ]; then
    echo "Usage: $0 [release-bin-directory]" >&2
    exit 2
fi
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
bin_dir=${1:-"$here/bin"}
if [ "$#" -eq 0 ] && [ ! -d "$bin_dir" ] && [ -f "$here/../../Cargo.toml" ]; then
    bin_dir="$here/../../target/release"
fi
for binary in paxri paxri-gateway; do
    if [ ! -x "$bin_dir/$binary" ]; then
        echo "Missing executable $bin_dir/$binary. Pass the directory containing paxri and paxri-gateway." >&2
        exit 1
    fi
done
if [ "$(id -u)" != 0 ]; then
    echo "Run this installer as root to register the system service." >&2
    exit 1
fi

account=paxri
unit="$account-gateway.service"
getent group "$account" >/dev/null || groupadd --system "$account"
getent passwd "$account" >/dev/null || useradd --system --gid "$account" --no-create-home \
    --home-dir "/var/lib/$account" --shell /bin/bash "$account"
install -d -m 0700 -o "$account" -g "$account" "/etc/$account"
install -D -m 0755 -t /usr/local/bin "$bin_dir/paxri" "$bin_dir/paxri-gateway"
install -D -m 0644 /dev/stdin "/usr/local/lib/tmpfiles.d/$account.conf" <<EOF
f /etc/$account/node.toml :0600 :$account :$account - version = 1\n
EOF
install -D -m 0644 "$here/paxri-gateway.service" "/etc/systemd/system/$unit"
systemctl daemon-reload
systemctl enable "$unit"
systemctl restart "$unit"
