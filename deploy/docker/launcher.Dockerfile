# syntax=docker/dockerfile:1.7
# Optional worker launcher: Docker socket or namespaced Kubernetes workload RBAC.
FROM rust:1.100-bookworm@sha256:ade39a710ef85f3506cffcfec3e8a24ba090badef1dff2fb798f71d878833fea AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY services ./services
RUN cargo build --release --locked -p orbit-launcher

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc
ARG DOCKER_VERSION=27.3.1
ARG DOCKER_SHA256=9b4f6fe406e50f9085ee474c451e2bb5adb119a03591f467922d3b4e2ddf31d3
ARG KUBECTL_VERSION=v1.31.4
ARG KUBECTL_SHA256=298e19e9c6c17199011404278f0ff8168a7eca4217edad9097af577023a5620f
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && curl -fsSL "https://download.docker.com/linux/static/stable/x86_64/docker-${DOCKER_VERSION}.tgz" \
       -o /tmp/docker.tgz \
    && printf '%s  /tmp/docker.tgz\n' "${DOCKER_SHA256}" | sha256sum -c - \
    && tar -xzf /tmp/docker.tgz --strip-components=1 -C /usr/local/bin docker/docker \
    && curl -fsSL "https://dl.k8s.io/release/${KUBECTL_VERSION}/bin/linux/amd64/kubectl" \
       -o /tmp/kubectl \
    && printf '%s  /tmp/kubectl\n' "${KUBECTL_SHA256}" | sha256sum -c - \
    && install -m 0755 /tmp/kubectl /usr/local/bin/kubectl \
    && rm -f /tmp/docker.tgz /tmp/kubectl \
    && groupadd --gid 10001 orbit \
    && useradd --uid 10001 --gid 10001 --create-home --home-dir /home/orbit --shell /usr/sbin/nologin orbit
COPY --from=builder /app/target/release/orbit-launcher /usr/local/bin/orbit-launcher
USER 10001:10001
EXPOSE 8090
ENV ORBIT_LAUNCHER_ADDR=0.0.0.0:8090
ENTRYPOINT ["/usr/local/bin/orbit-launcher"]
