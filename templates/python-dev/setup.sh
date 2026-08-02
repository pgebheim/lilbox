#!/bin/sh
# python-dev provisioning. Idempotent: guarded so re-runs are no-ops.
# (uv and any extra tooling are layered on in #12.)
set -eu

if ! command -v git >/dev/null 2>&1; then
  apt-get update -qq
  apt-get install -y -qq git
fi

echo "python-dev ready: $(git --version)"
