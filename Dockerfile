# syntax=docker/dockerfile:1

# ---- build ---------------------------------------------------------------
# The crate needs Rust >= 1.88 (slice::as_chunks); rust:1 tracks latest stable.
FROM rust:1-slim-bookworm AS builder
WORKDIR /app

# rust-toolchain.toml (channel = "stable") is deliberately NOT copied: it makes
# rustup re-sync and download the stable channel inside the build. The image's
# preinstalled toolchain already is stable.
ENV CARGO_TERM_COLOR=never

COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Cache mounts keep the registry and incremental target dir across builds.
# The target dir is a cache mount, so copy the binary out before the step ends.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    cargo build --release --locked --bin maps \
 && cp target/release/maps /usr/local/bin/maps \
 && strip --strip-debug /usr/local/bin/maps

# ---- runtime -------------------------------------------------------------
# distroless/cc ships glibc + libgcc and nothing else (no shell, no curl), and
# runs as uid 65532 ("nonroot"). There is no in-image HEALTHCHECK: probe
# GET /health (returns "ok") from your orchestrator instead, e.g. a Kubernetes
# readiness/liveness httpGet probe on port 8080.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /usr/local/bin/maps /usr/local/bin/maps

USER nonroot:nonroot
EXPOSE 8080
# Mount the extract read-only at /data/map.osm.pbf (or override CMD).
VOLUME ["/data"]
ENTRYPOINT ["/usr/local/bin/maps"]
CMD ["serve", "/data/map.osm.pbf", "--addr", "0.0.0.0:8080"]
