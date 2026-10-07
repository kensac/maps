# syntax=docker/dockerfile:1

# Build. Needs Rust 1.88+ (slice::as_chunks).
FROM rust:1-slim-bookworm AS builder
WORKDIR /app
ENV CARGO_TERM_COLOR=never

COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Cache the registry and target dir across builds.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    cargo build --release --locked --bin maps \
 && cp target/release/maps /usr/local/bin/maps \
 && strip --strip-debug /usr/local/bin/maps

# Runtime: glibc only, non-root. Probe GET /health externally.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /usr/local/bin/maps /usr/local/bin/maps

USER nonroot:nonroot
EXPOSE 8080
VOLUME ["/data"]
ENTRYPOINT ["/usr/local/bin/maps"]
CMD ["serve", "/data/map.osm.pbf", "--addr", "0.0.0.0:8080"]
