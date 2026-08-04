#!/bin/sh
# go-dev provisioning: the go toolchain comes from the base image; add git
# plus gopls and delve. Idempotent — guarded so re-runs are no-ops.
set -eu

if ! command -v git >/dev/null 2>&1; then
  apt-get update -qq
  apt-get install -y -qq git
fi

export GOBIN=/usr/local/bin

command -v gopls >/dev/null 2>&1 || go install golang.org/x/tools/gopls@latest
command -v dlv >/dev/null 2>&1 || go install github.com/go-delve/delve/cmd/dlv@latest

echo "go-dev ready: $(go version); $(git --version)"
