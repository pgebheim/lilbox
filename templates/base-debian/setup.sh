#!/bin/sh
# base-debian provisioning: the devcontainers base already bakes in git,
# zsh+oh-my-zsh, common CLI utils, and a sudo-enabled non-root user, so the
# default setup is a thin toolbelt top-up. Intended as the base users fork
# for a bring-your-own-stack box. Idempotent — guarded so re-runs are no-ops.
set -eu

NEED=""
command -v rg >/dev/null 2>&1 || NEED="$NEED ripgrep"
command -v fdfind >/dev/null 2>&1 || command -v fd >/dev/null 2>&1 || NEED="$NEED fd-find"
command -v jq >/dev/null 2>&1 || NEED="$NEED jq"

if [ -n "$NEED" ]; then
  sudo apt-get update -qq
  sudo apt-get install -y -qq $NEED
fi

echo "base-debian ready: $(git --version)"
