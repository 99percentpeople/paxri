#!/usr/bin/env bash
# Run the real tunnel CLI and a private relay inside a disposable Linux test network.
set -euo pipefail
family=ipv4
verify_gateway=0
if [[ ${1:-} == --gateway && $# == 1 ]]; then
    verify_gateway=1
    set -- symmetric symmetric
fi
if [[ ${1:-} == --ipv6-only && $# == 1 ]]; then
    family=ipv6
    set -- none none
else
    if [[ ${1:-} == --dual-stack ]]; then family=dual; shift; fi
    if [[ $# != 2 || ! $1 =~ ^(cone|symmetric)$ || ! $2 =~ ^(cone|symmetric)$ ]]; then
        echo "Usage: $0 [--dual-stack] {cone|symmetric} {cone|symmetric} | --ipv6-only | --gateway" >&2
        exit 2
    fi
fi
root=$(cd "$(dirname "$0")/.." && pwd)
tunnel=$(realpath "${TUNNEL_BIN:-$root/target/debug/tunnel}")
relay=$(realpath "${IROH_RELAY_BIN:-$(command -v iroh-relay)}")
[[ -x "$tunnel" && -x "$relay" ]]
tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT

timeout 110s unshare --user --map-root-user --mount --net --pid --fork --kill-child --mount-proc \
    bash -seu -o pipefail -- "$tmp" "$tunnel" "$relay" "$root" "$family" "$@" "$verify_gateway" <<'LAB'
cd "$1"
tunnel=$2 relay=$3 root=$4 family=$5 mode_a=$6 mode_b=$7
mount --make-rprivate /
mount -t tmpfs tmpfs /run
ip link set lo up
ip link add wan type bridge
ip link set wan up
if [[ $family != ipv6 ]]; then
    ip addr add 203.0.113.1/24 dev wan
    ip addr add 203.0.113.2/32 dev wan
    ip addr add 203.0.113.3/32 dev wan
fi
if [[ $family != ipv4 ]]; then
    sysctl -qw net.ipv6.conf.all.forwarding=1
    ip -6 addr add 2001:db8:100::1/64 dev wan nodad
fi

gateway() {
    local id=$1 mode=$2 public=$3 gw="gw-$1" node="node-$1"
    ip netns add "$gw"
    ip netns add "$node"
    ip link add "wire-$id" type veth peer name external netns "$gw"
    ip link set "wire-$id" master wan
    ip link set "wire-$id" up
    ip -n "$gw" link add internal type veth peer name eth0 netns "$node"
    ip -n "$gw" link set lo up
    ip -n "$gw" link set internal up
    ip -n "$gw" link set external up
    ip -n "$node" link set lo up
    ip -n "$node" link set eth0 up
    if [[ $family != ipv4 ]]; then
        ip netns exec "$gw" sysctl -qw net.ipv6.conf.all.forwarding=1
        ip -n "$gw" -6 addr add "2001:db8:100::$id/64" dev external nodad
        ip -n "$gw" -6 addr add "2001:db8:$id::1/64" dev internal nodad
        ip -n "$node" -6 addr add "2001:db8:$id::2/64" dev eth0 nodad
        ip -n "$node" -6 route add default via "2001:db8:$id::1"
        ip -n "$gw" -6 route add default via 2001:db8:100::1
        ip -6 route add "2001:db8:$id::/64" via "2001:db8:100::$id"
        ip netns exec "$gw" nft -f - <<'NFT'
table ip6 lab6 {
    chain forward_traffic { type filter hook forward priority filter; policy drop; iifname "internal" accept; ct state established,related accept; meta l4proto ipv6-icmp accept; }
}
NFT
    fi
    if [[ $family == ipv6 ]]; then return; fi
    ip -n "$gw" addr add "$public/24" dev external
    ip -n "$gw" addr add 10.77.0.1/24 dev internal
    ip -n "$node" addr add 10.77.0.2/24 dev eth0
    ip -n "$node" route add default via 10.77.0.1
    ip netns exec "$gw" sysctl -qw net.ipv4.ip_forward=1
    ip netns exec "$gw" nft -f - <<'NFT'
table ip lab {
    chain protect_input { type filter hook input priority filter; iifname "external" meta l4proto udp drop; }
    chain send { type nat hook postrouting priority srcnat; }
    chain forward_traffic { type filter hook forward priority filter; policy drop; iifname "internal" accept; ct state established,related accept; }
}
NFT
    if [[ $mode == cone ]]; then
        # One internal host. Outbound UDP opens the matching port for any external sender.
        ip netns exec "$gw" nft -f - <<NFT
add set ip lab active_ports { type inet_service; flags dynamic,timeout; timeout 120s; size 65535; }
add rule ip lab forward_traffic iifname "external" ct status dnat accept
add chain ip lab receive { type nat hook prerouting priority dstnat; }
add rule ip lab receive iifname "external" ip daddr $public udp dport @active_ports dnat to 10.77.0.2
add chain ip lab remember { type filter hook prerouting priority mangle; }
add rule ip lab remember iifname "internal" ip saddr 10.77.0.2 meta l4proto udp update @active_ports { udp sport timeout 120s }
NFT
    else
        # Separate, disjoint source-port pools for the destinations exercised in this lab.
        # WAN replies are accepted only when conntrack matches the external address and port.
        ip netns exec "$gw" nft -f - <<NFT
add rule ip lab send oifname "external" ip daddr 203.0.113.1 udp dport 7842 snat to $public:42000-42099 random
add rule ip lab send oifname "external" ip daddr 203.0.113.1 udp dport 4001 snat to $public:42100-42199 random
add rule ip lab send oifname "external" ip daddr 203.0.113.1 udp dport 4002 snat to $public:42200-42299 random
add rule ip lab send oifname "external" ip daddr 203.0.113.2 meta l4proto udp snat to $public:42300-42399 random
add rule ip lab send oifname "external" meta l4proto udp snat to $public:42400-42499 random
NFT
    fi
    ip netns exec "$gw" nft add rule ip lab send oifname external masquerade
}

gateway a "$mode_a" 203.0.113.10
gateway b "$mode_b" 203.0.113.20
if [[ $family != ipv6 ]]; then
    python3 "$root/scripts/check-nat-mapping.py" node-a "$mode_a" 203.0.113.10
    python3 "$root/scripts/check-nat-mapping.py" node-b "$mode_b" 203.0.113.20
fi
relay_ip=203.0.113.1
relay_host=$relay_ip
relay_url_host=$relay_ip
relay_san="IP:$relay_ip"
if [[ $family == ipv6 ]]; then
    relay_ip=2001:db8:100::1
    relay_host="[$relay_ip]"
    relay_url_host=relay.v6.test
    relay_san="DNS:$relay_url_host"
    printf '::1 localhost\n%s %s\n' "$relay_ip" "$relay_url_host" > hosts
    mount --bind "$PWD/hosts" /etc/hosts
fi

openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 \
    -subj /CN=nat-lab-ca -keyout ca.key -out ca.pem >/dev/null 2>&1
openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
    -subj /CN=nat-lab-relay -keyout relay.key -out relay.csr >/dev/null 2>&1
echo "subjectAltName=$relay_san" > relay.ext
openssl x509 -req -in relay.csr -CA ca.pem -CAkey ca.key -CAcreateserial -days 1 \
    -extfile relay.ext -out relay.pem >/dev/null 2>&1
export SSL_CERT_FILE="$PWD/ca.pem"
cat > relay.toml <<EOF
http_bind_addr = "$relay_host:80"
enable_quic_addr_discovery = true
[tls]
https_bind_addr = "$relay_host:443"
quic_bind_addr = "$relay_host:7842"
cert_mode = "Manual"
manual_cert_path = "$PWD/relay.pem"
manual_key_path = "$PWD/relay.key"
EOF
"$relay" --config-path relay.toml >&2 &
relay_pid=$!
for _ in {1..100}; do
    kill -0 "$relay_pid"
    if ss -lntH 'sport = :443' | grep -Fq "$relay_host:443"; then break; fi
    sleep 0.1
done

a=$("$tunnel" identity a.key)
b=$("$tunnel" identity b.key)
for node in a b; do
    peer=$a
    if [[ $node == a ]]; then peer=$b; fi
    printf 'version=1\n[native]\nrelay_url="https://%s"\n[peers.other]\nendpoint_id="%s"\n' "$relay_url_host" "$peer" > "$node.toml"
done
if [[ $8 == 1 ]]; then
    gateway_bin=$(dirname "$tunnel")/tunnel-gateway
    [[ -x "$gateway_bin" ]]
    # Require traffic to traverse the relay even if new direct candidates appear.
    ip netns exec gw-a nft insert rule ip lab forward_traffic ip daddr 203.0.113.20 meta l4proto udp drop
    ip netns exec gw-b nft insert rule ip lab forward_traffic ip daddr 203.0.113.10 meta l4proto udp drop
    cat >> b.toml <<'EOF'
[services.fixture]
target = "127.0.0.1:9001"
allow_peers = ["other"]
EOF
    cat >> a.toml <<'EOF'
[forwards.fixture]
listen = "127.0.0.1:9000"
peer = "other"
service = "fixture"
EOF
    mkdir public
    head -c 262144 /dev/urandom > public/fixture.bin
    ip netns exec node-b python3 -m http.server 9001 --bind 127.0.0.1 --directory public >&2 &
    for node in b a; do
        ip netns exec "node-$node" "$gateway_bin" --config "$node.toml" --key "$node.key" --ipc "$PWD/$node.sock" >&2 &
    done
    for _ in {1..80}; do
        if ip netns exec node-a curl -fsS --noproxy '*' --max-time 5 http://127.0.0.1:9000/fixture.bin -o got.bin &&
            cmp -s public/fixture.bin got.bin; then
            echo "PASS: two gateways transferred 256 KiB via a private HTTPS relay behind two symmetric NATs"
            exit 0
        fi
        sleep 0.25
    done
    exit 1
fi
ip netns exec node-b "$tunnel" probe listen --key b.key --config b.toml --peer other >ticket.log &
listener=$!
for _ in {1..150}; do
    kill -0 "$listener"
    if grep -q '^ticket=.' ticket.log 2>/dev/null; then break; fi
    sleep 0.1
done
grep -q '^ticket=.' ticket.log
connect_args=()
if [[ $family == ipv4 ]]; then
    connect_args=(--ticket "$(sed -n 's/^ticket=//p' ticket.log)")
fi
if [[ $family == ipv6 ]]; then
    # Keep QAD and HTTPS reachable while the first samples can only use the relay.
    ip netns exec gw-a nft -f - <<'NFT'
add chain ip6 lab6 block_peer { type filter hook forward priority -5; }
add rule ip6 lab6 block_peer ip6 daddr 2001:db8:b::2 meta l4proto udp drop
add rule ip6 lab6 block_peer ip6 saddr 2001:db8:b::2 meta l4proto udp drop
NFT
fi
ip netns exec node-a "$tunnel" probe connect --key a.key --config a.toml --peer other \
    "${connect_args[@]}" --count 20 | while IFS= read -r sample; do
        echo "$sample"
        if [[ $family == ipv6 && $sample == "seq=3 "* ]]; then
            ip netns exec gw-a nft flush chain ip6 lab6 block_peer
        fi
    done | tee samples.log
awk -v family="$family" -v topology="$mode_a/$mode_b" '
    /^seq=/ {
        n++
        if ($3 == "path=relay") relay++
        if ($3 == "path=direct" && $4 ~ /^addr=ip:203\.0\.113\.20:/) v4++
        if ($3 == "path=direct" && $4 ~ /^addr=ip:\[2001:db8:b::2\]:/) v6++
    }
    END {
        if (family == "ipv4") printf "%s probes=%d direct=%d relay=%d\n", topology, n, v4, relay
        else printf "%s %s probes=%d direct_v4=%d direct_v6=%d relay=%d\n", family, topology, n, v4, v6, relay
        if (family == "ipv6") required = relay >= 3 && v6 > 0
        else if (family == "dual" && topology == "symmetric/symmetric") required = v6 > 0
        else required = topology == "symmetric/symmetric" || v4 + v6 > 0
        exit !(n == 20 && n == v4 + v6 + relay && required)
    }
' samples.log
LAB
