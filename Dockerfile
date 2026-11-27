FROM rust:1.98.1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY packaging/distribution.toml packaging/distribution.toml
COPY compat/ compat/
COPY crates/ crates/
COPY apps/ apps/
COPY mobile/android/Cargo.toml mobile/android/Cargo.toml
COPY mobile/android/src/ mobile/android/src/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo --config packaging/distribution.toml build --locked --profile distribution \
        -p paxri-gateway -p paxri-cli \
    && install -D -t /out target/distribution/paxri-gateway target/distribution/paxri

# CI overrides this context with the binaries in the matching Linux release.
FROM scratch AS paxri-binaries
COPY --from=build /out/ /

FROM debian:bookworm-slim AS certificates
RUN apt-get update -q \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

FROM debian:bookworm-slim
COPY --from=certificates /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
RUN groupadd --gid 10001 paxri \
    && useradd --uid 10001 --gid paxri --no-create-home \
        --home-dir /var/lib/paxri --shell /bin/bash paxri \
    && install -d -m 0700 -o paxri -g paxri /var/lib/paxri /run/paxri
COPY --from=paxri-binaries / /usr/local/bin/
COPY --chmod=0755 packaging/docker/entrypoint.sh /usr/local/libexec/paxri/docker-entrypoint
ENV HOME=/var/lib/paxri LANG=C.UTF-8 PAXRI_IPC=/run/paxri/gateway.sock
USER paxri:paxri
WORKDIR /var/lib/paxri
HEALTHCHECK --interval=30s --start-period=20s --timeout=5s CMD paxri status >/dev/null
ENTRYPOINT ["/usr/local/libexec/paxri/docker-entrypoint"]
