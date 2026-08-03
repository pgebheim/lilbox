#!/usr/bin/env bash
# Build the lilexe-box image and load it into microsandbox.
#
#   docker build   ->   docker save (OCI tar)   ->   msb load -t lilexe-box
#
# microsandbox has no native image build; it consumes OCI images. We build with
# Docker and import the result straight into msb's local image cache under the
# tag `lilexe-box`, which `vm new --image lilexe-box` then resolves locally --
# no registry required.
#
# Usage:
#   images/lilexe-box/build.sh            # build + load as `lilexe-box`
#   TAG=lilexe-box:dev images/lilexe-box/build.sh
set -euo pipefail

TAG="${TAG:-lilexe-box}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MSB="${MSB:-$(command -v msb || echo "$HOME/.local/bin/msb")}"

command -v docker >/dev/null 2>&1 || {
  echo "build.sh: docker not found (needed to build the OCI image)" >&2
  exit 1
}
[ -x "$MSB" ] || { echo "build.sh: msb not found at '$MSB'" >&2; exit 1; }

echo "==> docker build -t $TAG $HERE"
docker build -t "$TAG" "$HERE"

echo "==> importing into microsandbox as '$TAG'"
# Stream the image from docker straight into msb load -- no tar file on disk.
docker save "$TAG" | "$MSB" load -t "$TAG"

echo "==> done."
echo "    boot it with:  vm new --image $TAG"
