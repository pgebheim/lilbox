#!/bin/sh
# fullstack-web provisioning: node/npm come from the base image (Debian, not
# alpine, so apt is available). Enable pnpm/yarn via corepack, add the
# Python backend toolchain, and install uv. Idempotent — guarded so re-runs
# are no-ops.
set -eu

corepack enable 2>/dev/null || true

if ! command -v git >/dev/null 2>&1 || ! command -v python3 >/dev/null 2>&1; then
  apt-get update -qq
  apt-get install -y -qq git python3 python3-venv curl
fi

if ! command -v uv >/dev/null 2>&1; then
  curl -LsSf https://astral.sh/uv/install.sh | env UV_INSTALL_DIR=/usr/local/bin sh
fi

echo "fullstack-web ready: node $(node --version); $(git --version); $(python3 --version); uv $(uv --version)"
