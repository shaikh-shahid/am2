#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
release_dir="$fixture/release"
mkdir -p "$release_dir" "$fixture/staging"
for executable in am2 am2-ghostty-bridge am2-terminal-bridge; do
    printf '#!/bin/sh\necho %s\n' "$executable" > "$fixture/staging/$executable"
    chmod +x "$fixture/staging/$executable"
done

case "$(uname -m)" in
    arm64) target=aarch64-apple-darwin ;;
    x86_64) target=x86_64-apple-darwin ;;
    *) echo "unsupported test architecture" >&2; exit 1 ;;
esac
archive="am2-v0.1.0-$target.tar.gz"
tar -czf "$release_dir/$archive" -C "$fixture/staging" am2 am2-ghostty-bridge am2-terminal-bridge
(cd "$release_dir" && shasum -a 256 "$archive" > SHA256SUMS)

AM2_REPO=example/am2 AM2_TAG=v0.1.0 \
AM2_DOWNLOAD_BASE_URL="file://$release_dir" AM2_INSTALL_DIR="$fixture/bin" \
    sh "$root/scripts/install.sh"
for executable in am2 am2-ghostty-bridge am2-terminal-bridge; do
    test -x "$fixture/bin/$executable"
done
test "$("$fixture/bin/am2")" = am2

printf 'tampered' >> "$release_dir/$archive"
if AM2_REPO=example/am2 AM2_TAG=v0.1.0 \
    AM2_DOWNLOAD_BASE_URL="file://$release_dir" AM2_INSTALL_DIR="$fixture/bad-bin" \
    sh "$root/scripts/install.sh"; then
    echo "installer accepted a bad checksum" >&2
    exit 1
fi
test ! -e "$fixture/bad-bin/am2"
