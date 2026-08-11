#!/bin/sh
# Install zed-crypt into ~/.local/bin.

set -eu

TARGET="${PREFIX:-$HOME/.local/bin}"
SRC=$(cd "$(dirname "$0")" && pwd)/zed-crypt

[ -f "$SRC" ] || { echo "zed-crypt not found next to install.sh" >&2; exit 1; }

mkdir -p "$TARGET"
install -m 755 "$SRC" "$TARGET/zed-crypt"
echo "installed $TARGET/zed-crypt"

have_backend=0
for b in gpg age sops; do
  command -v "$b" >/dev/null 2>&1 && { echo "backend available: $b"; have_backend=1; }
done
[ "$have_backend" -eq 1 ] || echo "warning: no backend installed (need gpg, age, or sops)" >&2

command -v zed >/dev/null 2>&1 || echo "warning: zed not on PATH (set ZED_CRYPT_EDITOR for another editor)" >&2

case ":$PATH:" in
  *":$TARGET:"*) ;;
  *) echo "note: $TARGET is not on your PATH" >&2 ;;
esac
