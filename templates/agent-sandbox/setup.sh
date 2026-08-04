#!/bin/sh
# agent-sandbox provisioning: start from a slim Debian and provision the
# toolbelt an agent expects — git, curl, python3, ripgrep, jq, uv, and
# Node. Idempotent — each step guarded so re-runs are no-ops.
set -eu

if ! command -v git >/dev/null 2>&1 || ! command -v curl >/dev/null 2>&1 \
  || ! command -v python3 >/dev/null 2>&1 || ! command -v rg >/dev/null 2>&1 \
  || ! command -v jq >/dev/null 2>&1; then
  apt-get update -qq
  apt-get install -y -qq git curl ca-certificates python3 python3-venv ripgrep jq
fi

if ! command -v uv >/dev/null 2>&1; then
  curl -LsSf https://astral.sh/uv/install.sh | env UV_INSTALL_DIR=/usr/local/bin sh
fi

if ! command -v node >/dev/null 2>&1; then
  curl -fsSL https://deb.nodesource.com/setup_22.x | bash -
  apt-get install -y -qq nodejs
fi

echo "agent-sandbox ready: $(git --version); $(python3 --version); node $(node --version)"
