FROM rust:1.98.1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY packaging/distribution.toml packaging/distribution.toml
COPY compat/ compat/
COPY crates/ crates/
COPY apps/ apps/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo --config packaging/distribution.toml build --locked --profile distribution \
        -p peerhatch-gateway -p peerhatch-cli \
    && install -D -t /out target/distribution/peerhatch-gateway target/distribution/peerhatch

FROM debian:bookworm-slim
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
RUN groupadd --gid 10001 peerhatch \
    && useradd --uid 10001 --gid peerhatch --no-create-home \
        --home-dir /var/lib/peerhatch --shell /bin/bash peerhatch \
    && install -d -m 0700 -o peerhatch -g peerhatch /var/lib/peerhatch /run/peerhatch
COPY --from=build /out/ /usr/local/bin/
COPY --chmod=0755 packaging/linux/start-gateway /usr/local/libexec/peerhatch/start-gateway
ENV HOME=/var/lib/peerhatch PEERHATCH_IPC=/run/peerhatch/gateway.sock
USER peerhatch:peerhatch
WORKDIR /var/lib/peerhatch
HEALTHCHECK --interval=30s --start-period=20s --timeout=5s CMD peerhatch status >/dev/null
ENTRYPOINT ["/usr/local/libexec/peerhatch/start-gateway", "/var/lib/peerhatch"]
