#!/usr/bin/env sh
# Build and install the native vm binary into ~/.local/bin.
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
DEST_DIR="${HOME}/.local/bin"

cd "$ROOT"
cargo build --release --locked
mkdir -p "$DEST_DIR"
install -m 0755 target/release/vm "$DEST_DIR/vm"
echo "installed $DEST_DIR/vm"

case ":${PATH}:" in
    *":${DEST_DIR}:"*) ;;
    *) echo "note: ${DEST_DIR} is not on your PATH; add it to use 'vm' directly." ;;
esac

echo "done. try: vm doctor"
