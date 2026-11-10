#!/bin/sh
# Install AIVory Code on macOS or Linux without a package manager.
#
#   curl -fsSL https://github.com/aivorynet/aivory-code/releases/latest/download/install.sh | sh
#
# It downloads the archive of the latest release for this OS and CPU, puts `aivory` (the app)
# and `aivoryd` (the daemon) side by side in ~/.local/share/aivory-code, and links both into
# ~/.local/bin. Set AIVORY_VERSION=2026.1.0 for a fixed version, or AIVORY_PREFIX for another
# folder. Homebrew users: `brew install aivorynet/tap/aivory-code` does the same.
set -eu

repo="aivorynet/aivory-code"
prefix="${AIVORY_PREFIX:-$HOME/.local}"
home="$prefix/share/aivory-code"
bin="$prefix/bin"

case "$(uname -s)" in
  Darwin) os=macos ;;
  Linux) os=linux ;;
  *) echo "aivory: this script supports macOS and Linux. On Windows: winget install Aivory.AivoryCode" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) arch=arm64 ;;
  x86_64 | amd64) arch=x64 ;;
  *) echo "aivory: no build for CPU $(uname -m)" >&2; exit 1 ;;
esac
if [ "$os" = linux ] && [ "$arch" = arm64 ]; then
  echo "aivory: no Linux arm64 build yet" >&2
  exit 1
fi

name="aivory-$os-$arch"
if [ -n "${AIVORY_VERSION:-}" ]; then
  url="https://github.com/$repo/releases/download/v$AIVORY_VERSION/$name.tar.gz"
else
  url="https://github.com/$repo/releases/latest/download/$name.tar.gz"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM
echo "aivory: downloading $url"
if command -v curl >/dev/null 2>&1; then
  curl -fsSL "$url" -o "$tmp/a.tar.gz"
else
  wget -qO "$tmp/a.tar.gz" "$url"
fi
tar -xzf "$tmp/a.tar.gz" -C "$tmp"

mkdir -p "$home" "$bin"
cp "$tmp/$name/aivory" "$tmp/$name/aivoryd" "$home/"
chmod +x "$home/aivory" "$home/aivoryd"
# A download by curl has no quarantine flag; remove it anyway when a browser download is reused.
if [ "$os" = macos ]; then
  xattr -d com.apple.quarantine "$home/aivory" "$home/aivoryd" 2>/dev/null || true
fi
ln -sf "$home/aivory" "$bin/aivory"
ln -sf "$home/aivoryd" "$bin/aivoryd"

echo "aivory: installed in $home"
case ":$PATH:" in
  *":$bin:"*) echo "aivory: start it with: aivory" ;;
  *) echo "aivory: add $bin to your PATH, then start it with: aivory" ;;
esac
