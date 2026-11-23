FROM rust:1.98.1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY packaging/distribution.toml packaging/distribution.toml
COPY compat/ compat/
COPY crates/ crates/
COPY apps/ apps/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo --config packaging/distribution.toml build --locked --profile distribution \
        -p paxri-gateway -p paxri-cli \
    && install -D -t /out target/distribution/paxri-gateway target/distribution/paxri

FROM debian:bookworm-slim
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
RUN groupadd --gid 10001 paxri \
    && useradd --uid 10001 --gid paxri --no-create-home \
        --home-dir /var/lib/paxri --shell /bin/bash paxri \
    && install -d -m 0700 -o paxri -g paxri /var/lib/paxri /run/paxri
COPY --from=build /out/ /usr/local/bin/
COPY --chmod=0755 packaging/docker/entrypoint.sh /usr/local/libexec/paxri/docker-entrypoint
ENV HOME=/var/lib/paxri LANG=C.UTF-8 PAXRI_IPC=/run/paxri/gateway.sock
USER paxri:paxri
WORKDIR /var/lib/paxri
HEALTHCHECK --interval=30s --start-period=20s --timeout=5s CMD paxri status >/dev/null
ENTRYPOINT ["/usr/local/libexec/paxri/docker-entrypoint"]
