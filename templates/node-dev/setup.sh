#!/bin/sh
# node-dev provisioning. node + npm come from the base image; add git.
# Idempotent — guarded so re-runs are no-ops. (node:*-alpine → apk.)
set -eu

if ! command -v git >/dev/null 2>&1; then
  apk add --no-cache git
fi

echo "node-dev ready: node $(node --version); npm $(npm --version); $(git --version)"
