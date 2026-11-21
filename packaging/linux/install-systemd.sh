#!/bin/sh
set -eu

if [ "$#" -gt 1 ]; then
    echo "Usage: $0 [release-bin-directory]" >&2
    exit 2
fi
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
bin_dir=${1:-"$here/../../target/release"}
for binary in peerhatch peerhatch-gateway; do
    if [ ! -x "$bin_dir/$binary" ]; then
        echo "Missing $bin_dir/$binary. Build with cargo build --locked --release -p peerhatch-gateway -p peerhatch-cli first." >&2
        exit 1
    fi
done
if [ "$(id -u)" != 0 ]; then
    echo "Run this installer as root to register the system service." >&2
    exit 1
fi

getent group peerhatch >/dev/null || groupadd --system peerhatch
getent passwd peerhatch >/dev/null || useradd --system --gid peerhatch --no-create-home \
    --home-dir /var/lib/peerhatch --shell /bin/bash peerhatch
install -d -m 0700 -o peerhatch -g peerhatch /etc/peerhatch
install -D -m 0755 -t /usr/local/bin "$bin_dir/peerhatch" "$bin_dir/peerhatch-gateway"
install -D -m 0644 /dev/stdin /usr/local/lib/tmpfiles.d/peerhatch.conf <<'EOF'
f /etc/peerhatch/node.toml :0600 :peerhatch :peerhatch - version = 1\n
EOF
install -D -m 0644 "$here/peerhatch-gateway.service" /etc/systemd/system/peerhatch-gateway.service
systemctl daemon-reload
systemctl enable peerhatch-gateway.service
systemctl restart peerhatch-gateway.service
