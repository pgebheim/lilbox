#!/usr/bin/env sh
# Build and install the native lilbox binary into ~/.local/bin.
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
DEST_DIR="${HOME}/.local/bin"

cd "$ROOT"
cargo build --release --locked
mkdir -p "$DEST_DIR"
install -m 0755 target/release/lilbox "$DEST_DIR/lilbox"
echo "installed $DEST_DIR/lilbox"

case ":${PATH}:" in
    *":${DEST_DIR}:"*) ;;
    *) echo "note: ${DEST_DIR} is not on your PATH; add it to use 'lilbox' directly." ;;
esac

echo "done. try: lilbox doctor"
