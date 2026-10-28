# A loop runner image: the Crystalline binary plus Claude Code.
#
# It runs `crystalline reflect run` or `crystalline evolve run` against a
# team server in remote mode (CRYSTALLINE_REMOTE_URL and
# CRYSTALLINE_REMOTE_TOKEN). This is an example, not a published image. See
# docs/automation.md.
#
# The published Crystalline image is distroless and has no shell and no
# Node, so Claude Code cannot be installed on top of it. The crystalline
# binary in it is a static musl build, so it is copied into a Node base
# image instead.
#
# Build:
#   docker build -t crystalline-runner examples/kubernetes
#
# Pin both versions. The loop commands need Crystalline 0.29.0 or newer, and
# Claude Code 2.1.312 is the version the runner's flags were checked against.

ARG CRYSTALLINE_TAG=v0.29.0
FROM ghcr.io/jordiboehme/crystalline:${CRYSTALLINE_TAG} AS crystalline

FROM node:24-bookworm-slim

# The crystalline binary reads the system CA certificates for HTTPS, and the
# slim Node image does not ship them. Not pinned: the newest bundle is the
# one to have.
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

ARG CLAUDE_CODE_VERSION=2.1.312
RUN npm install -g "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
    && npm cache clean --force

COPY --from=crystalline /usr/local/bin/crystalline /usr/local/bin/crystalline
COPY --chmod=755 crystalline-loop.sh /usr/local/bin/crystalline-loop

# The same user and paths as the published image. A run writes its log and
# the remote client's cache under these paths, and Claude Code writes under
# HOME, so they must be writable (an emptyDir in the Kubernetes examples).
RUN groupadd --gid 65532 nonroot \
    && useradd --uid 65532 --gid 65532 --create-home --home-dir /home/nonroot nonroot \
    && mkdir -p /data \
    && chown 65532:65532 /data
# The version is pinned on purpose, and the root filesystem is read-only in
# the Kubernetes examples, so Claude Code must not try to update itself.
ENV DISABLE_AUTOUPDATER=1 \
    HOME=/home/nonroot \
    XDG_CONFIG_HOME=/data/config \
    XDG_STATE_HOME=/data/state \
    XDG_CACHE_HOME=/data/cache
USER 65532:65532
WORKDIR /home/nonroot

# `crystalline-loop reflect` runs `crystalline reflect run` once and turns
# its exit code 1 (nothing due, or already running) into 0, so a scheduler
# does not count a quiet check as a failed job.
CMD ["crystalline-loop", "reflect", "--runner", "k8s"]
