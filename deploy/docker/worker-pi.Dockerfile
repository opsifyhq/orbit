# syntax=docker/dockerfile:1.7
# Pi-enabled execution worker. Pi is a Node package; the Rust worker binaries come from
# the base worker image. Auth is mounted externally (PI_CODING_AGENT_DIR).
ARG WORKER_BASE=orbit-worker:latest
FROM node:26.13-bookworm-slim@sha256:5df79989ed685e09f0a98019db4cebfb22fb9df1a53c0161664a011f7ff6b0f1 AS node
FROM ${WORKER_BASE}

USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git libatomic1 \
    && rm -rf /var/lib/apt/lists/*

# Opt-in CLI escape hatches (worker-profile allowGlab/allowGh). Pinned for reproducibility.
ARG GH_VERSION=2.105.0
ARG GH_SHA256=79e7ab5db738c48777b696e374f8b428f0647debfb424f44d2f426370407fe2f
ARG GLAB_VERSION=1.131.0
ARG GLAB_SHA256=580dbca43835b47267e512476a442f6b2e8785a82b26ccacee22bd7010662b2d
RUN curl -fsSL --retry 3 \
       "https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_amd64.tar.gz" \
       -o /tmp/gh.tar.gz \
    && printf '%s  /tmp/gh.tar.gz\n' "${GH_SHA256}" | sha256sum -c - \
    && tar -xzf /tmp/gh.tar.gz -C /tmp \
    && install -m 0755 "/tmp/gh_${GH_VERSION}_linux_amd64/bin/gh" /usr/local/bin/gh \
    && curl -fsSL --retry 3 \
       "https://gitlab.com/gitlab-org/cli/-/releases/v${GLAB_VERSION}/downloads/glab_${GLAB_VERSION}_linux_amd64.tar.gz" \
       -o /tmp/glab.tar.gz \
    && printf '%s  /tmp/glab.tar.gz\n' "${GLAB_SHA256}" | sha256sum -c - \
    && tar -xzf /tmp/glab.tar.gz -C /tmp \
    && install -m 0755 /tmp/bin/glab /usr/local/bin/glab \
    && rm -f /tmp/gh.tar.gz /tmp/glab.tar.gz \
    && gh --version && glab --version

COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=node /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -sf /usr/local/lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
    && ln -sf /usr/local/lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx \
    && npm install -g @earendil-works/pi-coding-agent@1.5.2 \
    && npm cache clean --force \
    && pi --version

RUN mkdir -p /home/orbit/.pi/agent \
    && chown -R 10001:10001 /home/orbit

ENV HOME=/home/orbit \
    PI_CODING_AGENT_DIR=/home/orbit/.pi/agent \
    ORBIT_WORKER_HARNESSES=pi

USER 10001:10001
ENTRYPOINT ["/usr/local/bin/orbit-worker"]
