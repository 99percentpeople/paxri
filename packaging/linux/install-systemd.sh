#!/bin/sh
set -eu

if [ "$#" -gt 1 ]; then
    echo "Usage: $0 [release-bin-directory]" >&2
    exit 2
fi
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
bin_dir=${1:-"$here/../../target/release"}
for binary in paxri paxri-gateway; do
    if [ ! -x "$bin_dir/$binary" ]; then
        echo "Missing $bin_dir/$binary. Build with cargo build --locked --release -p paxri-gateway -p paxri-cli first." >&2
        exit 1
    fi
done
if [ "$(id -u)" != 0 ]; then
    echo "Run this installer as root to register the system service." >&2
    exit 1
fi

# Upgrade the existing system unit in place so it keeps its account and device key.
account=paxri
if [ ! -e /etc/systemd/system/paxri-gateway.service ] && [ -f /etc/systemd/system/peerhatch-gateway.service ]; then
    account=peerhatch
fi
unit="$account-gateway.service"
getent group "$account" >/dev/null || groupadd --system "$account"
getent passwd "$account" >/dev/null || useradd --system --gid "$account" --no-create-home \
    --home-dir "/var/lib/$account" --shell /bin/bash "$account"
install -d -m 0700 -o "$account" -g "$account" "/etc/$account"
install -D -m 0755 -t /usr/local/bin "$bin_dir/paxri" "$bin_dir/paxri-gateway"
install -D -m 0644 /dev/stdin "/usr/local/lib/tmpfiles.d/$account.conf" <<EOF
f /etc/$account/node.toml :0600 :$account :$account - version = 1\n
EOF
service="$here/paxri-gateway.service"
if [ "$account" = peerhatch ]; then
    legacy_unit=$(mktemp)
    trap 'rm -f "$legacy_unit"' EXIT
    sed -e 's|=paxri$|=peerhatch|' -e 's|/etc/paxri/|/etc/peerhatch/|g' \
        -e 's|/var/lib/paxri$|/var/lib/peerhatch|' -e 's|/run/paxri/|/run/peerhatch/|' \
        -e 's|tmpfiles.d/paxri.conf|tmpfiles.d/peerhatch.conf|' "$service" > "$legacy_unit"
    service=$legacy_unit
fi
install -D -m 0644 "$service" "/etc/systemd/system/$unit"
systemctl daemon-reload
systemctl enable "$unit"
systemctl restart "$unit"
