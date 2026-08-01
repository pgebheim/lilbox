#!/usr/bin/env sh
# Symlink the vm CLI into ~/.local/bin.
set -eu

SRC="$(cd "$(dirname "$0")" && pwd)/bin/vm"
DEST_DIR="${HOME}/.local/bin"
DEST="${DEST_DIR}/vm"

chmod +x "$SRC"
mkdir -p "$DEST_DIR"
ln -sf "$SRC" "$DEST"
echo "linked $DEST -> $SRC"

if ! command -v msb >/dev/null 2>&1 && [ ! -x "${HOME}/.local/bin/msb" ]; then
    echo "warning: microsandbox (msb) not found."
    echo "  install it: curl -fsSL https://install.microsandbox.dev | sh"
fi

case ":${PATH}:" in
    *":${DEST_DIR}:"*) ;;
    *) echo "note: ${DEST_DIR} is not on your PATH — add it to use 'vm' directly." ;;
esac

echo "done. try: vm doctor"
