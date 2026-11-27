# syntax=docker/dockerfile:1.7
# ORBIT control plane (orbit-api). Multi-stage, non-root, reproducible with --locked.
FROM rust:1.100-bookworm@sha256:ade39a710ef85f3506cffcfec3e8a24ba090badef1dff2fb798f71d878833fea AS builder
WORKDIR /app

# Dependency layer: manifests first for better build caching.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/orbit-domain/Cargo.toml crates/orbit-domain/Cargo.toml
COPY crates/orbit-contracts/Cargo.toml crates/orbit-contracts/Cargo.toml
COPY crates/orbit-store/Cargo.toml crates/orbit-store/Cargo.toml
COPY crates/orbit-events/Cargo.toml crates/orbit-events/Cargo.toml
COPY crates/orbit-security/Cargo.toml crates/orbit-security/Cargo.toml
COPY crates/orbit-harness/Cargo.toml crates/orbit-harness/Cargo.toml
COPY crates/orbit-kubernetes/Cargo.toml crates/orbit-kubernetes/Cargo.toml
COPY crates/orbit-s3/Cargo.toml crates/orbit-s3/Cargo.toml
COPY crates/orbit-exchange/Cargo.toml crates/orbit-exchange/Cargo.toml
COPY crates/orbit-api/Cargo.toml crates/orbit-api/Cargo.toml
COPY services/api/Cargo.toml services/api/Cargo.toml
COPY services/worker/Cargo.toml services/worker/Cargo.toml
COPY services/launcher/Cargo.toml services/launcher/Cargo.toml
COPY services/license-cli/Cargo.toml services/license-cli/Cargo.toml
COPY services/mcp/Cargo.toml services/mcp/Cargo.toml
RUN mkdir -p crates/orbit-domain/src crates/orbit-contracts/src crates/orbit-store/src \
      crates/orbit-events/src crates/orbit-security/src crates/orbit-harness/src \
      crates/orbit-kubernetes/src crates/orbit-s3/src crates/orbit-exchange/src crates/orbit-api/src services/api/src services/worker/src services/launcher/src \
      services/license-cli/src services/mcp/src \
    && for d in crates/orbit-domain crates/orbit-contracts crates/orbit-store crates/orbit-events \
              crates/orbit-security crates/orbit-harness crates/orbit-kubernetes crates/orbit-s3 crates/orbit-exchange crates/orbit-api; do echo "" > "$d/src/lib.rs"; done \
    && echo "fn main() {}" > services/api/src/main.rs \
    && echo "fn main() {}" > services/worker/src/main.rs \
    && echo "fn main() {}" > services/launcher/src/main.rs \
    && echo "fn main() {}" > services/license-cli/src/main.rs \
    && echo "fn main() {}" > services/mcp/src/main.rs \
    && cargo fetch --locked

COPY crates ./crates
# Documentation is embedded into orbit-api at compile time (crates/orbit-api/src/docs.rs).
COPY docs ./docs
COPY services ./services
RUN cargo build --release --locked -p orbit-api-server --bin orbit-api

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc AS harness
RUN apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 orbit \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin orbit

COPY --from=builder /app/target/release/orbit-api /usr/local/bin/orbit-api
USER 10001:10001
EXPOSE 8080
ENV ORBIT_HTTP_ADDR=0.0.0.0:8080
HEALTHCHECK --interval=15s --timeout=3s --start-period=10s --retries=3 \
  CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1
ENTRYPOINT ["/usr/local/bin/orbit-api"]
