#!/bin/sh
# Symlink claude-proxy into ~/.local/bin (override with BINDIR=...).
set -eu

SRC=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/claude-proxy
DEST=${BINDIR:-$HOME/.local/bin}

[ -x "$SRC" ] || { echo "install.sh: $SRC is not executable" >&2; exit 1; }
mkdir -p "$DEST"
ln -sf "$SRC" "$DEST/claude-proxy"
echo "linked $DEST/claude-proxy -> $SRC"

case ":$PATH:" in
  *":$DEST:"*) ;;
  *) echo "note: $DEST is not on your PATH" >&2 ;;
esac
