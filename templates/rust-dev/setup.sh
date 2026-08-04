#!/bin/sh
# rust-dev provisioning: cargo/rustc come from the base image; add git + the
# build deps needed for common crates, plus clippy/rustfmt/rust-analyzer.
# Idempotent — guarded so re-runs are no-ops.
set -eu

if ! command -v git >/dev/null 2>&1; then
  apt-get update -qq
  apt-get install -y -qq git pkg-config libssl-dev
fi

rustup component add clippy rustfmt 2>/dev/null || true
command -v rust-analyzer >/dev/null 2>&1 || rustup component add rust-analyzer 2>/dev/null || true

echo "rust-dev ready: $(rustc --version); $(cargo --version); $(git --version)"
