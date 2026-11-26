#!/bin/bash
# Make the README pictures (docs/screenshot-dark.png, docs/screenshot-light.png,
# docs/demo.gif) in a container, from a made-up home folder, so no real file
# names end up in them. Needs docker (Docker Desktop works:
# DOCKER_CONTEXT=desktop-linux).
#
#   tests/demo/run.sh            build the current tree and take the pictures
#   OUT=/some/dir tests/demo/run.sh
set -euo pipefail
REPO=$(cd "$(dirname "$0")/../.." && pwd)
OUT=${OUT:-$REPO/docs}
mkdir -p "$OUT"

tar -C "$REPO" --exclude=./target --exclude=./gui/build --exclude=./gui/tests/build --exclude=./.git --exclude=./dist/out \
    --exclude=./docs -cf - . | docker build -q -t trawlr-demo -f tests/demo/Dockerfile -
# --privileged: fanotify. /home on a volume: fanotify does not report changes
# made inside the container's own overlay filesystem.
name=trawlr-demo-$$
trap 'docker rm -fv "$name" >/dev/null 2>&1 || true' EXIT
docker run --name "$name" --privileged --mount type=volume,dst=/home trawlr-demo sh /src/tests/demo/inside.sh
docker cp "$name:/out/." "$OUT"
