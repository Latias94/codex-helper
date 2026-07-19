# syntax=docker/dockerfile:1

ARG CARGO_CHEF_IMAGE=lukemathwalker/cargo-chef:0.1.77-rust-1.95@sha256:00c3c07c51d092325df88f0df2d626cd4302e12933f179ba154509cc314d6c2a
ARG DEBIAN_MIRROR=http://deb.debian.org/debian
ARG DEBIAN_SECURITY_MIRROR=http://deb.debian.org/debian-security

FROM ${CARGO_CHEF_IMAGE} AS chef
WORKDIR /workspace
ARG DEBIAN_MIRROR
ARG DEBIAN_SECURITY_MIRROR
ENV CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse

RUN set -eux; \
    sed -i \
        -e "s|^URIs: http://deb.debian.org/debian-security$|URIs: ${DEBIAN_SECURITY_MIRROR}|g" \
        -e "s|^URIs: http://deb.debian.org/debian$|URIs: ${DEBIAN_MIRROR}|g" \
        /etc/apt/sources.list.d/debian.sources; \
    rm -f /etc/apt/apt.conf.d/docker-clean; \
    apt_packages="ca-certificates libssl-dev pkg-config"; \
    for attempt in 1 2 3 4 5; do \
        apt-get -o Acquire::Retries=5 update \
        && apt-get -o Acquire::Retries=5 install -y --download-only --no-install-recommends \
            $apt_packages \
        && break; \
        if [ "$attempt" = "5" ]; then exit 1; fi; \
        rm -rf /var/lib/apt/lists/*; \
        sleep "$((attempt * 5))"; \
    done; \
    apt-get install -y --no-download --no-install-recommends $apt_packages; \
    rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/*.deb /var/cache/apt/archives/partial/*

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /workspace/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json -p codex-helper-server --bin codex-helper-server
COPY . .
RUN cargo build --locked --release -p codex-helper-server --bin codex-helper-server

FROM debian:trixie-slim AS runtime
WORKDIR /app
ARG DEBIAN_MIRROR
ARG DEBIAN_SECURITY_MIRROR

RUN set -eux; \
    sed -i \
        -e "s|^URIs: http://deb.debian.org/debian-security$|URIs: ${DEBIAN_SECURITY_MIRROR}|g" \
        -e "s|^URIs: http://deb.debian.org/debian$|URIs: ${DEBIAN_MIRROR}|g" \
        /etc/apt/sources.list.d/debian.sources; \
    rm -f /etc/apt/apt.conf.d/docker-clean; \
    apt_packages="ca-certificates curl libssl3t64 tini"; \
    for attempt in 1 2 3 4 5 6 7 8; do \
        apt-get -o Acquire::Retries=5 update \
        && apt-get -o Acquire::Retries=5 install -y --download-only --no-install-recommends \
            $apt_packages \
        && break; \
        if [ "$attempt" = "8" ]; then exit 1; fi; \
        rm -rf /var/lib/apt/lists/*; \
        sleep "$((attempt * 5))"; \
    done; \
    apt-get install -y --no-download --no-install-recommends $apt_packages; \
    rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/*.deb /var/cache/apt/archives/partial/*; \
    groupadd --system --gid 10001 codex-helper; \
    useradd --system --uid 10001 --gid codex-helper --home-dir /nonexistent --shell /usr/sbin/nologin codex-helper; \
    mkdir -p /config /data; \
    chown -R codex-helper:codex-helper /config /data

COPY --from=builder /workspace/target/release/codex-helper-server /usr/local/bin/codex-helper-server

USER codex-helper:codex-helper
ENV CODEX_HELPER_HOME=/data
ENV RUST_LOG=info
EXPOSE 3211 4211
VOLUME ["/config", "/data"]

ENTRYPOINT ["/usr/bin/tini", "--", "codex-helper-server"]
CMD ["--config", "/config/server.toml"]
