#!/bin/bash
set -euo pipefail

if [[ ${1:-} == --help || ${1:-} == -h ]]; then
    echo 'Usage: scripts/package-macos.sh'
    echo 'Build a locally signed Manga GPUI.app for this Mac (requires Xcode 26+).'
    exit 0
fi
if [[ $# != 0 || $(uname -s) != Darwin ]]; then
    echo 'Run scripts/package-macos.sh on macOS, without arguments.' >&2
    exit 1
fi

root=$(cd "$(dirname "$0")/.." && pwd -P)
cd "$root"
xcode_major=$(xcodebuild -version | awk '/^Xcode / { split($2, v, "."); print v[1] }')
if [[ ! $xcode_major =~ ^[0-9]+$ ]] || (( xcode_major < 26 )); then
    echo 'Icon Composer assets require full Xcode 26 or later; check xcode-select -p.' >&2
    exit 1
fi

output_dir="$root/target/release/bundle"
mkdir -p "$output_dir"
staging=$(mktemp -d "$output_dir/.manga-gpui.XXXXXX")
output="$output_dir/Manga GPUI.app"
cleanup() {
    # A failed replacement leaves the last working bundle available.
    if [[ -d "$staging/previous.app" && ! -e "$output" ]]; then
        mv "$staging/previous.app" "$output"
    fi
    rm -rf "$staging"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

host=$(rustc -vV | awk '/^host:/ { print $2 }')
cargo build --release --locked --target "$host" --target-dir "$root/target" -p manga-app --bin manga-gpui
binary="$root/target/$host/release/manga-gpui"
minimum_macos=$(xcrun vtool -show-build "$binary" | awk '/minos/ { print $2; exit }')
if [[ ! $minimum_macos =~ ^[0-9]+\.[0-9]+(\.[0-9]+)?$ ]]; then
    echo 'Cannot read the built executable macOS deployment target.' >&2
    exit 1
fi
package_id=$(cargo pkgid -p manga-app)
version=${package_id##*#}
version=${version##*@}
if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Unsupported bundle version: $version" >&2
    exit 1
fi

app="$staging/Manga GPUI.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/manga-gpui"
cp "$root/packaging/macos/Info.plist" "$app/Contents/Info.plist"

xcrun actool "$root/assets/macos/AppIcon.icon" \
    --compile "$app/Contents/Resources" \
    --platform macosx --minimum-deployment-target "$minimum_macos" \
    --app-icon AppIcon --output-partial-info-plist "$staging/icon-info.plist" \
    --output-format human-readable-text --warnings --errors
test -s "$app/Contents/Resources/Assets.car"
(cd "$staging" && /usr/libexec/PlistBuddy -c 'Merge icon-info.plist' "$app/Contents/Info.plist")
/usr/libexec/PlistBuddy -c "Add :CFBundleShortVersionString string $version" "$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Add :CFBundleVersion string $version" "$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Add :LSMinimumSystemVersion string $minimum_macos" "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
codesign --force --sign - "$app"
codesign --verify --strict "$app"

if [[ -e "$output" ]]; then
    mv "$output" "$staging/previous.app"
fi
mv "$app" "$output"
echo "Built: $output"
