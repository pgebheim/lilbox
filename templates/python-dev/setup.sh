#!/bin/sh
# python-dev provisioning: git + uv. Idempotent — guarded so re-runs are no-ops.
set -eu

# git + curl (curl is needed to fetch uv); the python image is Debian-based.
if ! command -v git >/dev/null 2>&1 || ! command -v curl >/dev/null 2>&1; then
  apt-get update -qq
  apt-get install -y -qq git curl
fi

# uv into /usr/local/bin so it's on PATH for `lilbox exec` / `lilbox ssh`.
if ! command -v uv >/dev/null 2>&1; then
  curl -LsSf https://astral.sh/uv/install.sh | env UV_INSTALL_DIR=/usr/local/bin sh
fi

echo "python-dev ready: $(git --version); uv $(uv --version)"
