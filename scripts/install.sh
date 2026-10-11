#!/bin/sh
set -eu

# The release workflow replaces these defaults in its install.sh asset.
repo="${AM2_REPO:-@AM2_REPO@}"
tag="${AM2_TAG:-@AM2_TAG@}"
if ! printf '%s' "$repo" | grep -Eq '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$'; then
    echo 'AM2 repository is not configured; use the install.sh asset from a GitHub Release' >&2
    exit 2
fi
if ! printf '%s' "$tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'; then
    echo 'AM2 release tag is not configured or is invalid' >&2
    exit 2
fi
if [ "$(uname -s)" != Darwin ]; then
    echo 'AM2 beta supports macOS only' >&2
    exit 2
fi
case "$(uname -m)" in
    arm64) target=aarch64-apple-darwin ;;
    x86_64) target=x86_64-apple-darwin ;;
    *) echo 'Unsupported Mac architecture' >&2; exit 2 ;;
esac

version=${tag#v}
archive="am2-v${version}-${target}.tar.gz"
base_url="${AM2_DOWNLOAD_BASE_URL:-https://github.com/$repo/releases/download/$tag}"
install_dir="${AM2_INSTALL_DIR:-${HOME:?HOME is required}/.local/bin}"
case "$install_dir" in
    /*) ;;
    *) echo 'AM2_INSTALL_DIR must be an absolute path' >&2; exit 2 ;;
esac

work_dir=$(mktemp -d)
trap 'rm -rf "$work_dir"' 0
trap 'exit 1' 1 2 3 15
curl -fsSL --retry 3 --output "$work_dir/$archive" "$base_url/$archive"
curl -fsSL --retry 3 --output "$work_dir/SHA256SUMS" "$base_url/SHA256SUMS"
awk -v archive="$archive" '$2 == archive { print; count++ } END { if (count != 1) exit 1 }' \
    "$work_dir/SHA256SUMS" > "$work_dir/CHECKSUM"
(cd "$work_dir" && shasum -a 256 -c CHECKSUM)

mkdir "$work_dir/unpacked"
tar -xzf "$work_dir/$archive" -C "$work_dir/unpacked" \
    am2 am2-ghostty-bridge am2-terminal-bridge
mkdir -p "$install_dir"
install -m 755 "$work_dir/unpacked/am2-ghostty-bridge" "$install_dir/am2-ghostty-bridge"
install -m 755 "$work_dir/unpacked/am2-terminal-bridge" "$install_dir/am2-terminal-bridge"
install -m 755 "$work_dir/unpacked/am2" "$install_dir/am2"
echo "Installed AM2 $tag to $install_dir"

case ":$PATH:" in
    *":$install_dir:"*) echo 'Run: am2 doctor'; exit 0 ;;
esac
if [ -z "${AM2_INSTALL_DIR:-}" ] && [ "$install_dir" = "$HOME/.local/bin" ]; then
    case "${SHELL##*/}" in
        zsh) profile="$HOME/.zprofile" ;;
        bash) profile="$HOME/.bash_profile" ;;
        *) profile='' ;;
    esac
    if [ -n "$profile" ]; then
        path_line='export PATH="$HOME/.local/bin:$PATH"'
        if ! grep -Fq "$path_line" "$profile" 2>/dev/null; then
            printf '\n# AM2 command-line tools\n%s\n' "$path_line" >> "$profile"
        fi
        echo "Added $install_dir to $profile. Open a new terminal, then run: am2 doctor"
        exit 0
    fi
fi
echo "Add $install_dir to PATH, then run: am2 doctor"
