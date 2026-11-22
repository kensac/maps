# syntax=docker/dockerfile:1@sha256:572cebbc2b15e6f84c077742952674889cb63cd5e9b80abdffcbe587b6f19183

# Build. Needs Rust 1.88+ (slice::as_chunks).
FROM rust:1-slim-bookworm@sha256:7c7d4eb4d05b7781187a327809466cfddf29753f6defdc3d19a0e291951b7a37 AS builder
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
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f
COPY --from=builder /usr/local/bin/maps /usr/local/bin/maps

USER nonroot:nonroot
EXPOSE 8080
VOLUME ["/data"]
ENTRYPOINT ["/usr/local/bin/maps"]
CMD ["serve", "/data/map.osm.pbf", "--addr", "0.0.0.0:8080"]
