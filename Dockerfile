# Crystalline OCI image.
#
# Base: gcr.io/distroless/static-debian13:nonroot, not `scratch`. reqwest's
# TLS stack (rustls-platform-verifier) reads system CA certificates at
# runtime - needed for the one-time embedding model download and for any
# OpenAI-compatible remote provider - and distroless/static-debian13 ships
# that CA bundle plus a numeric nonroot user (65532:65532) and a real
# /home/nonroot; scratch ships neither and HTTPS would fail. See
# research/distribution.md for the full comparison.
#
# Multi-arch build context layout (produced by the `images` job in
# .github/workflows/release.yml before it calls buildx): the two musl
# release binaries are extracted and staged at
#   dist/linux-amd64/crystalline
#   dist/linux-arm64/crystalline
# relative to the build context root. Buildx sets TARGETARCH per platform
# (amd64, arm64) when building `platforms: linux/amd64,linux/arm64`, so the
# COPY below picks the matching staged binary automatically.
#
# Two named stages, one image family: `runtime` is the slim image
# (`crystalline:latest`) and `runtime-with-model` layers the pre-fetched
# embedding model on top of it for a `-with-model` tagged variant.
# `runtime-with-model` is the last stage in the file, so a bare `docker
# build` with no `--target` would produce it, not the slim image - callers
# that want `runtime` (including the release workflow's slim build-push
# step) must pass `--target runtime` explicitly. The `runtime` stage's own
# content is unchanged by this split.

FROM gcr.io/distroless/static-debian13:nonroot AS runtime

ARG TARGETARCH
COPY --chown=nonroot:nonroot dist/linux-${TARGETARCH}/crystalline /usr/local/bin/crystalline

# All XDG paths land under /data so one volume covers config, the disposable
# search index and the model cache. Engrams themselves are never under
# /data - they live in whatever domain paths are bind-mounted separately
# (see examples/docker/compose.yaml), since files on disk are the only
# durable state.
ENV HOME=/home/nonroot \
    XDG_CONFIG_HOME=/data/config \
    XDG_STATE_HOME=/data/state \
    XDG_CACHE_HOME=/data/cache

VOLUME /data
EXPOSE 7411

USER nonroot

ENTRYPOINT ["/usr/local/bin/crystalline"]
CMD ["serve", "--http", "0.0.0.0:7411"]

# The `-with-model` variant: the embedding model pre-fetched at build time so
# semantic search works from the first daemon start, with no runtime egress.
#
# The model is copied to /opt/crystalline/models, not under /data, on
# purpose: /data is the VOLUME above, so anything baked there would be
# shadowed by whatever bind mount or named volume a caller attaches at
# runtime (a fresh named volume only inherits image content on its very
# first use, then a bind mount or a reused volume shadows it forever after).
# /opt/crystalline/models sits outside that volume so the baked files always
# win, and CRYSTALLINE_MODELS_DIR (crates/core/src/config.rs) points
# Crystalline at them directly instead of the default XDG cache path.
#
# Expected staging layout (produced by the `images` job before this build,
# same context root as the binaries above): the release workflow prefetches
# the model once with the staged amd64 binary and stages the resulting
# Hugging Face cache directory at
#   dist/model/models--BAAI--bge-small-en-v1.5
# which is the on-disk layout hf-hub itself uses (blobs, snapshots and refs
# subdirectories, with snapshot files as relative symlinks into blobs) - the
# whole directory is copied verbatim so those symlinks keep resolving.
FROM runtime AS runtime-with-model

COPY --chown=nonroot:nonroot dist/model/models--BAAI--bge-small-en-v1.5 /opt/crystalline/models/models--BAAI--bge-small-en-v1.5
ENV CRYSTALLINE_MODELS_DIR=/opt/crystalline/models
