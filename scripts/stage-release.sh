#!/bin/sh
# Lays out what a release archive (and `make install`) carries for this OS.
#
#   scripts/stage-release.sh BINARY STAGE_DIR [VERSION]
#
# Linux: gitmaster, share/applications/gitmaster.desktop and the hicolor icons.
# macOS: gitmaster and GitMaster.app (ad-hoc signed). VERSION, such as v1.2.0,
# is written into the app's Info.plist; the default is the workspace version.
set -eu

[ $# -ge 2 ] || { echo "usage: $0 BINARY STAGE_DIR [VERSION]" >&2; exit 2; }
bin=$1
stage=$2
version=${3:-0.0.0}
version=${version#v}
root=$(cd "$(dirname "$0")/.." && pwd)

mkdir -p "$stage"
install -m755 "$bin" "$stage/gitmaster"

case "$(uname -s)" in
Linux)
	install -Dm644 "$root/packaging/linux/gitmaster.desktop" "$stage/share/applications/gitmaster.desktop"
	for n in 16 24 32 48 64 128 256 512; do
		install -Dm644 "$root/assets/icons/png/dark/gitmaster-$n.png" \
			"$stage/share/icons/hicolor/${n}x${n}/apps/gitmaster.png"
	done
	;;
Darwin)
	app="$stage/GitMaster.app"
	rm -rf "$app"
	mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
	sed "s|<string>0.0.0</string>|<string>$version</string>|" "$root/packaging/macos/Info.plist" >"$app/Contents/Info.plist"
	cp "$root/assets/icons/macos/GitMaster-dark.icns" "$app/Contents/Resources/GitMaster.icns"
	install -m755 "$bin" "$app/Contents/MacOS/gitmaster"
	# Ad-hoc: stops the "damaged" prompt on Apple silicon, not Gatekeeper's
	# "unidentified developer" one.
	codesign --force --sign - "$app"
	;;
*)
	echo "stage-release.sh: unsupported OS $(uname -s)" >&2
	exit 1
	;;
esac
