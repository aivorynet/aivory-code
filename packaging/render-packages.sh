#!/usr/bin/env bash
# Render the winget manifests and the Homebrew formula from packaging/ with the hashes of
# the release files. Usage: packaging/render-packages.sh <version> <dist-dir> <out-dir>
# The release workflow runs it and attaches the output to the GitHub release.
set -euo pipefail

version="${1:?version, for example 2026.1.0}"
dist="${2:?folder with the release files}"
out="${3:?output folder}"
root="$(cd "$(dirname "$0")/.." && pwd)"
# Absolute: the checksum step below runs inside the dist folder.
mkdir -p "$out"
out="$(cd "$out" && pwd)"

hash() {
  local f="$dist/$1"
  if [ -f "$f" ]; then
    sha256sum "$f" | cut -d' ' -f1
  else
    echo "render-packages: missing $f" >&2
    echo "MISSING"
  fi
}

win=$(hash aivory-code-windows-x64-setup.exe)
mac_arm=$(hash aivory-macos-arm64.tar.gz)
mac_x64=$(hash aivory-macos-x64.tar.gz)
linux_x64=$(hash aivory-linux-x64.tar.gz)
date=$(date -u +%Y-%m-%d)

render() {
  sed -e "s/{{VERSION}}/$version/g" \
      -e "s/{{DATE}}/$date/g" \
      -e "s/{{SHA256_WINDOWS_X64}}/$win/g" \
      -e "s/{{SHA256_MACOS_ARM64}}/$mac_arm/g" \
      -e "s/{{SHA256_MACOS_X64}}/$mac_x64/g" \
      -e "s/{{SHA256_LINUX_X64}}/$linux_x64/g" \
      -e '/Template: packaging\/render-packages.sh/d' \
      "$1" > "$2"
}

# winget-pkgs layout: manifests/a/Aivory/AivoryCode/<version>/
wdir="$out/winget/manifests/a/Aivory/AivoryCode/$version"
mkdir -p "$wdir" "$out/homebrew/Formula"
for f in "$root"/packaging/winget/*.yaml; do
  render "$f" "$wdir/$(basename "$f")"
done
render "$root/packaging/homebrew/aivory-code.rb" "$out/homebrew/Formula/aivory-code.rb"

(cd "$dist" && sha256sum -- * > "$out/SHA256SUMS.txt")
echo "render-packages: wrote $out"
