#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
    echo "usage: package-release.sh TARGET_DIR VERSION ARCH DIST_DIR" >&2
    exit 2
fi

target_dir="$1"
version="$2"
arch="$3"
dist_dir="$4"
case "$arch" in
    aarch64-apple-darwin|x86_64-apple-darwin) ;;
    *) echo "unsupported release architecture: $arch" >&2; exit 2 ;;
esac
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
    echo "invalid release version: $version" >&2
    exit 2
fi
if [[ ! -f "$target_dir/release/am2" ]]; then
    echo "missing release binary: $target_dir/release/am2" >&2
    exit 1
fi

find_helper() {
    local matches
    matches="$(find "$target_dir/release/build" -type f -path "*/out/$1" -print)"
    if [[ -z "$matches" || "$matches" == *$'\n'* ]]; then
        echo "expected exactly one $1 in $target_dir/release/build" >&2
        exit 1
    fi
    printf '%s' "$matches"
}

ghostty_bridge="$(find_helper ghostty-bridge)"
terminal_bridge="$(find_helper terminal-bridge)"
staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
cp "$target_dir/release/am2" "$staging/am2"
cp "$ghostty_bridge" "$staging/am2-ghostty-bridge"
cp "$terminal_bridge" "$staging/am2-terminal-bridge"
cat > "$staging/INSTALL.md" <<'EOF'
# Install AM2

Keep `am2`, `am2-ghostty-bridge`, and `am2-terminal-bridge` together.
Copy all three files to a directory on PATH, such as `~/.local/bin`, then
run `am2 doctor` in a normal terminal. The GitHub Release also provides an
`install.sh` asset that automates the download, checksum check, and install.
EOF
chmod 755 "$staging/am2" "$staging/am2-ghostty-bridge" "$staging/am2-terminal-bridge"
mkdir -p "$dist_dir"
archive="$dist_dir/am2-v$version-$arch.tar.gz"
tar -czf "$archive" -C "$staging" am2 am2-ghostty-bridge am2-terminal-bridge INSTALL.md
echo "Created $archive"
