FROM rust:1.98.1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY compat/ compat/
COPY crates/ crates/
COPY apps/ apps/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --locked --release -p tunnel-gateway -p tunnel-cli \
    && install -D -s -t /out target/release/tunnel-gateway target/release/tunnel

FROM debian:bookworm-slim
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
RUN groupadd --gid 10001 p2p-tunnel \
    && useradd --uid 10001 --gid p2p-tunnel --no-create-home \
        --home-dir /var/lib/p2p-tunnel --shell /bin/bash p2p-tunnel \
    && install -d -m 0700 -o p2p-tunnel -g p2p-tunnel /var/lib/p2p-tunnel /run/p2p-tunnel
COPY --from=build /out/ /usr/local/bin/
COPY --chmod=0755 packaging/linux/start-gateway /usr/local/libexec/p2p-tunnel/start-gateway
ENV HOME=/var/lib/p2p-tunnel TUNNEL_IPC=/run/p2p-tunnel/gateway.sock
USER p2p-tunnel:p2p-tunnel
WORKDIR /var/lib/p2p-tunnel
HEALTHCHECK --interval=30s --start-period=20s --timeout=5s CMD tunnel status >/dev/null
ENTRYPOINT ["/usr/local/libexec/p2p-tunnel/start-gateway", "/var/lib/p2p-tunnel"]
