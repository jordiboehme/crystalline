# Fluid, the browser UI for Crystalline: a Node stage that builds the static
# bundle and an nginx stage that serves it and forwards the JSON API to a
# Crystalline daemon.
#
# The build context is this folder, not the repository root, because nothing
# outside it is needed:
#
#   docker build -f fluid/Dockerfile -t ghcr.io/jordiboehme/crystalline-fluid fluid/
#
# Fluid is a static bundle. It holds no state, talks to exactly one upstream
# and is safe to run in as many replicas as a deployment wants.

FROM node:26-alpine AS build

WORKDIR /app

# Corepack is no longer part of the Node distribution (dropped for Node 25 and
# later), so the node image no longer ships it. Installing it from npm keeps
# the pnpm version pinned in exactly one place - the "packageManager" field in
# package.json - which is what corepack reads to decide what to fetch. The
# prompt it would otherwise raise before downloading has no answer in a build.
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
RUN npm install --global corepack@0.35.0 && corepack enable pnpm

# The manifest and the lockfile on their own first, so the install layer is
# rebuilt only when the dependencies actually change and not on every edit to
# a source file.
COPY package.json pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile

COPY . .
RUN pnpm build

FROM nginx:stable-alpine AS runtime

LABEL org.opencontainers.image.title="Crystalline Fluid" \
      org.opencontainers.image.description="The browser UI for a Crystalline instance" \
      org.opencontainers.image.source="https://github.com/jordiboehme/crystalline" \
      org.opencontainers.image.licenses="Apache-2.0"

# Where the Crystalline daemon answers, as host:port. The nginx image's own
# entrypoint runs envsubst over /etc/nginx/templates/*.template at every
# container start, so this value is baked into the served config then; see the
# template for what that means for a DNS change. The filter keeps envsubst to
# CRYSTALLINE_-prefixed names, so nginx's own $variables in the template reach
# nginx untouched.
ENV CRYSTALLINE_UPSTREAM=crystalline:7411 \
    NGINX_ENVSUBST_FILTER=^CRYSTALLINE_

# The name matters: rendered to /etc/nginx/conf.d/default.conf, it replaces
# the stock server block the image ships rather than being added beside it.
COPY nginx.conf.template /etc/nginx/templates/default.conf.template
COPY --from=build /app/dist /usr/share/nginx/html

EXPOSE 80

# The same surface a reader uses, probed from inside: the app's own index
# page. Shell form, since this image has one, and busybox wget is already
# present.
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD wget --spider -q http://127.0.0.1/ || exit 1
