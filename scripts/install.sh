#!/bin/sh
# Installs the latest qdrant-data-mcp release. VERSION, INSTALL_DIR, and QDRANT_MCP_REPO override the defaults.
set -eu

REPO="${QDRANT_MCP_REPO:-qdrant-labs/qdrant-data-mcp}"
VERSION="${VERSION:-latest}"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
BIN="qdrant-data-mcp"

case "$(uname -s)" in
  Linux) os=unknown-linux-gnu ;;
  Darwin) os=apple-darwin ;;
  *) echo "error: unsupported OS $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64|amd64) arch=x86_64 ;;
  aarch64|arm64) arch=aarch64 ;;
  *) echo "error: unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac

asset="$BIN-$arch-$os.gz"
if [ "$VERSION" = "latest" ]; then
  url="https://github.com/$REPO/releases/latest/download/$asset"
else
  url="https://github.com/$REPO/releases/download/$VERSION/$asset"
fi

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
curl -fsSL -o "$tmp" "$url"

expected="$(curl -fsSL "${url%/*}/checksums.txt" | awk -v f="$asset" '$2 == f { print $1 }')"
actual="$(sha256sum "$tmp" 2>/dev/null || shasum -a 256 "$tmp")"
if [ -z "$expected" ] || [ "${actual%% *}" != "$expected" ]; then
  echo "error: checksum verification failed for $asset" >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
gunzip -c "$tmp" > "$INSTALL_DIR/$BIN"
chmod +x "$INSTALL_DIR/$BIN"
echo "Installed $INSTALL_DIR/$BIN"
