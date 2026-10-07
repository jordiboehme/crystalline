#!/bin/sh
# Install the two nginx snippets that only exist under a path prefix, before
# the image's own 20-envsubst-on-templates.sh renders every template. Nothing
# happens when CRYSTALLINE_BASE_PATH is empty, so the image serves at the root
# exactly as before. A value that cannot be a prefix stops the container with
# a sentence, rather than rendering a config that serves the wrong paths.
set -eu

base="${CRYSTALLINE_BASE_PATH:-}"
if [ -z "$base" ]; then
    exit 0
fi

if ! printf '%s\n' "$base" | grep -Eq '^(/[a-z0-9._-]+)+$' \
    || printf '%s\n' "$base" | grep -Eq '/\.\.?(/|$)'; then
    echo "CRYSTALLINE_BASE_PATH must look like /crystalline: one leading slash, no trailing slash, segments of lower-case letters, digits, '.', '_' and '-'; got '$base'" >&2
    exit 1
fi

mkdir -p /etc/nginx/templates/crystalline
cp /etc/nginx/crystalline-base-path/server.conf.template /etc/nginx/templates/crystalline/server.conf.template
cp /etc/nginx/crystalline-base-path/index.conf.template /etc/nginx/templates/crystalline/index.conf.template
