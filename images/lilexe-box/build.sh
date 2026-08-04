#!/usr/bin/env bash
# Build the lilexe-box image and load it into microsandbox.
#
#   docker build   ->   docker save (OCI tar)   ->   vm image load
#
# microsandbox has no native image build; it consumes OCI images. We build with
# Docker and import the result straight into microsandbox's image cache under the
# tag `lilexe-box`, which `vm new --image lilexe-box` then resolves locally --
# no registry required.
#
# Usage:
#   images/lilexe-box/build.sh            # build + load as `lilexe-box`
#   TAG=lilexe-box:dev images/lilexe-box/build.sh
set -euo pipefail

TAG="${TAG:-lilexe-box}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VM="${VM:-$(command -v vm || echo "$HOME/.local/bin/vm")}"

command -v docker >/dev/null 2>&1 || {
  echo "build.sh: docker not found (needed to build the OCI image)" >&2
  exit 1
}
[ -x "$VM" ] || { echo "build.sh: vm not found at '$VM'" >&2; exit 1; }

echo "==> docker build -t $TAG $HERE"
docker build -t "$TAG" "$HERE"

echo "==> importing into microsandbox as '$TAG'"
ARCHIVE="$(mktemp "${TMPDIR:-/tmp}/lilexe-image.XXXXXX.tar")"
trap 'rm -f "$ARCHIVE"' EXIT
docker save -o "$ARCHIVE" "$TAG"
"$VM" image load "$ARCHIVE" --tag "$TAG"

echo "==> done."
echo "    boot it with:  vm new --image $TAG"
