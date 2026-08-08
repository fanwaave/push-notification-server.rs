# syntax=docker/dockerfile:1.7

FROM rust:1.88-bookworm@sha256:af306cfa71d987911a781c37b59d7d67d934f49684058f96cf72079c3626bfe0 AS builder

WORKDIR /workspace
COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/workspace/target,sharing=locked \
    cargo build --locked --release \
    && install -D -m 0755 \
      target/release/push-notification-server \
      /out/push-notification-server

FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241

RUN groupadd --system --gid 10001 app \
    && useradd --system --uid 10001 --gid 10001 --no-create-home \
      --shell /usr/sbin/nologin app \
    && apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

LABEL org.opencontainers.image.source="https://github.com/fanwaave/push-notification-server.rs" \
      org.opencontainers.image.description="Provider-neutral Rust push and contact notification delivery service"

COPY --from=builder /out/push-notification-server /usr/local/bin/push-notification-server

ENV HOST=0.0.0.0 \
    PORT=8121 \
    RUST_LOG=push_notification_server=info

USER 10001:10001
EXPOSE 8121
ENTRYPOINT ["/usr/local/bin/push-notification-server"]
