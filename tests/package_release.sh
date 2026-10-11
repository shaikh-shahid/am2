#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/target/release/build/am2-test/out" "$fixture/dist"
printf '#!/bin/sh\n' > "$fixture/target/release/am2"
printf '#!/bin/sh\n' > "$fixture/target/release/build/am2-test/out/ghostty-bridge"
printf '#!/bin/sh\n' > "$fixture/target/release/build/am2-test/out/terminal-bridge"

bash "$root/scripts/package-release.sh" "$fixture/target" 0.1.0 aarch64-apple-darwin "$fixture/dist"
archive="$fixture/dist/am2-v0.1.0-aarch64-apple-darwin.tar.gz"
test -f "$archive"
entries="$(tar -tzf "$archive")"
for expected in am2 am2-ghostty-bridge am2-terminal-bridge INSTALL.md; do
    [[ "$entries" == *"$expected"* ]]
done

rm "$fixture/target/release/build/am2-test/out/terminal-bridge"
if bash "$root/scripts/package-release.sh" "$fixture/target" 0.1.0 x86_64-apple-darwin "$fixture/dist"; then
    echo "packaging should reject a missing companion helper" >&2
    exit 1
fi
