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

FROM gcr.io/distroless/static-debian13:nonroot

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
