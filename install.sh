#!/bin/sh
# Installs a Ronnie 3000 release for the current user. It needs no Rust and no build tools.
#
#   curl -fsSL https://raw.githubusercontent.com/KonScanner/ronnie-3000/main/install.sh | sh
#   sh install.sh [--version vX.Y.Z] [--prefix DIR] [--uninstall]
#
# The script downloads the Linux tarball of the release, checks it against the SHA256SUMS file of
# the same release, and unpacks it into PREFIX (default ~/.local). It never uses sudo.
set -eu

repo=KonScanner/ronnie-3000
prefix=${PREFIX:-$HOME/.local}
version=
uninstall=no
icon_sizes='16 32 48 64 128 256 512'

say() { printf '%s\n' "$*"; }
die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

usage() {
    say 'Usage: install.sh [--version vX.Y.Z] [--prefix DIR] [--uninstall]'
    say ''
    say '  --version   Install this release tag. Default: the latest release.'
    say '  --prefix    Install under this directory. Default: ~/.local'
    say '  --uninstall Remove the files that this script installs.'
}

while [ $# -gt 0 ]; do
    case $1 in
    --version)
        [ $# -ge 2 ] || die '--version needs a value'
        version=$2
        shift 2
        ;;
    --prefix)
        [ $# -ge 2 ] || die '--prefix needs a value'
        prefix=$2
        shift 2
        ;;
    --uninstall)
        uninstall=yes
        shift
        ;;
    -h | --help)
        usage
        exit 0
        ;;
    *) die "unknown option $1" ;;
    esac
done

installed_files() {
    say "$prefix/bin/ronie"
    say "$prefix/share/applications/ronnie-3000.desktop"
    say "$prefix/share/icons/hicolor/scalable/apps/ronnie-3000.svg"
    for size in $icon_sizes; do
        say "$prefix/share/icons/hicolor/${size}x${size}/apps/ronnie-3000.png"
    done
    say "$prefix/share/doc/ronnie/LICENSE"
}

refresh_desktop() {
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$prefix/share/applications" >/dev/null 2>&1 || true
    fi
    # GTK reads the icon folders directly when they have no cache. An old cache hides a new
    # icon, so refresh only a cache that exists.
    if [ -f "$prefix/share/icons/hicolor/icon-theme.cache" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -q -t -f "$prefix/share/icons/hicolor" >/dev/null 2>&1 || true
    fi
}

if [ "$uninstall" = yes ]; then
    installed_files | while IFS= read -r file; do rm -f "$file"; done
    refresh_desktop
    say "Removed Ronnie 3000 from $prefix. Your presets and settings stay in ~/.config/Ronie."
    exit 0
fi

case $(uname -s) in
Linux) ;;
Darwin) die "this script installs the Linux build. For macOS, download the .zip from https://github.com/$repo/releases/latest" ;;
*) die "this script installs the Linux build. Download your build from https://github.com/$repo/releases/latest" ;;
esac

case $(uname -m) in
x86_64 | amd64) arch=x86_64 ;;
aarch64 | arm64) arch=aarch64 ;;
*) die "no release build exists for $(uname -m). Build from source: https://github.com/$repo#build-from-source" ;;
esac

if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q --https-only -O "$2" "$1"; }
else
    die 'curl or wget is required'
fi
if command -v sha256sum >/dev/null 2>&1; then
    digest() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
    digest() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
    die 'sha256sum or shasum is required'
fi
command -v tar >/dev/null 2>&1 || die 'tar is required'

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM

if [ -z "$version" ]; then
    fetch "https://api.github.com/repos/$repo/releases/latest" "$work/latest.json" ||
        die 'could not find the latest release'
    version=$(sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$work/latest.json" | head -n 1)
    [ -n "$version" ] || die 'could not read the latest release tag'
fi
case $version in
v*) ;;
*) version=v$version ;;
esac

name="ronnie-3000-${version#v}-linux-$arch.tar.gz"
base="https://github.com/$repo/releases/download/$version"
say "Downloading Ronnie 3000 $version for Linux $arch..."
fetch "$base/$name" "$work/$name" || die "could not download $base/$name"
fetch "$base/SHA256SUMS" "$work/SHA256SUMS" || die "could not download $base/SHA256SUMS"

expected=$(awk -v file="$name" '$2 == file || $2 == "*" file { print $1 }' "$work/SHA256SUMS")
[ -n "$expected" ] || die "SHA256SUMS has no line for $name"
[ "$(digest "$work/$name")" = "$expected" ] || die "the checksum of $name does not match SHA256SUMS"

mkdir -p "$prefix"
tar -xzf "$work/$name" -C "$prefix" --strip-components=2
# The PATH of a desktop session often has no ~/.local/bin. The launcher then hides an entry
# whose TryExec it cannot find, so give the desktop file the full path of the binary.
desktop="$prefix/share/applications/ronnie-3000.desktop"
sed -e "s|^Exec=ronie|Exec=\"$prefix/bin/ronie\"|" -e "s|^TryExec=ronie\$|TryExec=$prefix/bin/ronie|" \
    "$desktop" >"$work/ronnie-3000.desktop"
cat "$work/ronnie-3000.desktop" >"$desktop"
refresh_desktop

say "Installed Ronnie 3000 $version in $prefix."
case ":$PATH:" in
*":$prefix/bin:"*) say 'Open Ronnie 3000 from your application menu, or run: ronie' ;;
*) say "Open Ronnie 3000 from your application menu, or run: $prefix/bin/ronie" ;;
esac
