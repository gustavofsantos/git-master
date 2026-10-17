#!/bin/sh
# Installs GitMaster from its GitHub releases.
#
#   curl -fsSL https://raw.githubusercontent.com/gustavofsantos/git-master/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/gustavofsantos/git-master/main/install.sh | sh -s -- --version v0.1.0 --dir /usr/local/bin
#
# Downloads the archive for this OS and CPU, checks it against the release's
# checksums.txt, and puts the gitmaster binary in --dir (default: ~/.local/bin).
# On Linux it also installs the icons and a desktop entry under ~/.local/share;
# on macOS, GitMaster.app under ~/Applications (--app-dir).
# Run it again to upgrade.
set -eu

repo=gustavofsantos/git-master
version=latest
dir="${HOME}/.local/bin"
app_dir="${HOME}/Applications"

usage() {
	cat <<'EOF2'
Usage: install.sh [--version TAG] [--dir DIR] [--app-dir DIR]

  --version TAG  release to install, such as v0.1.0 (default: the latest)
  --dir DIR      where to put the gitmaster binary (default: ~/.local/bin)
  --app-dir DIR  macOS only: where to put GitMaster.app (default: ~/Applications)
EOF2
}

fail() {
	echo "install.sh: $*" >&2
	exit 1
}

while [ $# -gt 0 ]; do
	case "$1" in
	--version)
		[ $# -ge 2 ] || fail "--version needs a tag"
		version=$2
		shift 2
		;;
	--dir)
		[ $# -ge 2 ] || fail "--dir needs a directory"
		dir=$2
		shift 2
		;;
	--app-dir)
		[ $# -ge 2 ] || fail "--app-dir needs a directory"
		app_dir=$2
		shift 2
		;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		usage >&2
		fail "unknown argument: $1"
		;;
	esac
done

case "$(uname -s)" in
Linux) os=linux ;;
Darwin) os=darwin ;;
*) fail "no prebuilt binary for $(uname -s); download one from https://github.com/$repo/releases or run: cargo install --git https://github.com/$repo gitmaster" ;;
esac

case "$(uname -m)" in
x86_64 | amd64) arch=amd64 ;;
aarch64 | arm64) arch=arm64 ;;
*) fail "no prebuilt binary for $(uname -m); run: cargo install --git https://github.com/$repo gitmaster" ;;
esac

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"

if [ "$version" = latest ]; then
	base="https://github.com/$repo/releases/latest/download"
else
	case "$version" in
	v*) ;;
	*) version="v$version" ;;
	esac
	base="https://github.com/$repo/releases/download/$version"
fi
archive="gitmaster_${os}_${arch}.tar.gz"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "Downloading $archive ($version)" >&2
curl -fsSL -o "$tmp/$archive" "$base/$archive" || fail "could not download $base/$archive"
curl -fsSL -o "$tmp/checksums.txt" "$base/checksums.txt" || fail "could not download $base/checksums.txt"

want=$(awk -v f="$archive" '$2 == f { print $1 }' "$tmp/checksums.txt")
[ -n "$want" ] || fail "$archive is not listed in checksums.txt"
if command -v sha256sum >/dev/null 2>&1; then
	got=$(sha256sum "$tmp/$archive" | awk '{ print $1 }')
elif command -v shasum >/dev/null 2>&1; then
	got=$(shasum -a 256 "$tmp/$archive" | awk '{ print $1 }')
else
	fail "sha256sum or shasum is required to verify the download"
fi
[ "$want" = "$got" ] || fail "checksum mismatch for $archive"

mkdir "$tmp/stage"
tar -xzf "$tmp/$archive" -C "$tmp/stage"
mkdir -p "$dir" || fail "cannot create $dir; pick another with --dir"
# Move into place from the same directory, so a running gitmaster is never
# overwritten halfway.
cp "$tmp/stage/gitmaster" "$dir/.gitmaster.new" || fail "cannot write to $dir; pick another with --dir"
chmod 755 "$dir/.gitmaster.new"
mv -f "$dir/.gitmaster.new" "$dir/gitmaster"

echo "Installed $("$dir/gitmaster" --version) to $dir/gitmaster" >&2

if [ "$os" = linux ]; then
	# The desktop entry's Exec is the absolute path, since a desktop session's
	# PATH rarely includes ~/.local/bin.
	data="${XDG_DATA_HOME:-$HOME/.local/share}"
	mkdir -p "$data/applications" "$data/icons"
	cp -R "$tmp/stage/share/icons/hicolor" "$data/icons/"
	sed "s|^Exec=.*|Exec=$dir/gitmaster|" "$tmp/stage/share/applications/gitmaster.desktop" >"$data/applications/gitmaster.desktop"
	echo "Installed the icons and desktop entry under $data" >&2
else
	mkdir -p "$app_dir" || fail "cannot create $app_dir; pick another with --app-dir"
	rm -rf "$app_dir/GitMaster.app"
	cp -R "$tmp/stage/GitMaster.app" "$app_dir/GitMaster.app"
	# A download carries the quarantine flag, and the app is only ad-hoc signed.
	xattr -dr com.apple.quarantine "$app_dir/GitMaster.app" 2>/dev/null || true
	echo "Installed $app_dir/GitMaster.app" >&2
fi
case ":$PATH:" in
*":$dir:"*) ;;
*) echo "Note: $dir is not on your PATH. Add it, for example: export PATH=\"$dir:\$PATH\"" >&2 ;;
esac
