#!/usr/bin/env bash
# Build the lilbox-box image and load it into microsandbox.
#
#   docker build   ->   docker save (OCI tar)   ->   lilbox image load
#
# microsandbox has no native image build; it consumes OCI images. We build with
# Docker and import the result straight into microsandbox's image cache under the
# tag `lilbox-box`, which `lilbox new --image lilbox-box` then resolves locally --
# no registry required.
#
# Usage:
#   images/lilbox-box/build.sh            # build + load as `lilbox-box`
#   TAG=lilbox-box:dev images/lilbox-box/build.sh
set -euo pipefail

TAG="${TAG:-lilbox-box}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LILBOX="${LILBOX:-$(command -v lilbox || echo "$HOME/.local/bin/lilbox")}"

command -v docker >/dev/null 2>&1 || {
  echo "build.sh: docker not found (needed to build the OCI image)" >&2
  exit 1
}
[ -x "$LILBOX" ] || { echo "build.sh: lilbox not found at '$LILBOX'" >&2; exit 1; }

echo "==> docker build -t $TAG $HERE"
docker build -t "$TAG" "$HERE"

echo "==> importing into microsandbox as '$TAG'"
ARCHIVE="$(mktemp "${TMPDIR:-/tmp}/lilbox-image.XXXXXX.tar")"
trap 'rm -f "$ARCHIVE"' EXIT
docker save -o "$ARCHIVE" "$TAG"
"$LILBOX" image load "$ARCHIVE" --tag "$TAG"

echo "==> done."
echo "    boot it with:  lilbox new --image $TAG"
