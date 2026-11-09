#!/bin/bash
# scripts/bundle.sh - builds target/bundle/MacRunnerbar.app (release build, LSUIElement, ad-hoc signed)
#
#   scripts/bundle.sh            build the release binary and assemble the .app
#   scripts/bundle.sh --check    also verify it: Info.plist is valid and has LSUIElement, the signature
#                                holds, and the app starts, draws a popover frame and exits
#                                (`macrunnerbar --smoke`: no engine, no network, no Keychain; needs a
#                                logged-in GUI session, exit 2 without one)
set -euo pipefail

cd "$(dirname "$0")/.."
check=0
case "${1:-}" in
  "") ;;
  --check) check=1 ;;
  *) echo "usage: scripts/bundle.sh [--check]" >&2; exit 2 ;;
esac

version="$(awk '/^\[workspace.package\]/{f=1;next} f&&/^version/{gsub(/[" ]/,"",$3);print $3;exit}' Cargo.toml)"
[ -n "$version" ] || { echo "cannot read the workspace version from Cargo.toml" >&2; exit 1; }

cargo build --release -p macrunnerbar-app

app="target/bundle/MacRunnerbar.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/macrunnerbar "$app/Contents/MacOS/macrunnerbar"
cp LICENSE-MIT LICENSE-APACHE THIRD_PARTY_NOTICES.md "$app/Contents/Resources/"
mkdir -p "$app/Contents/Resources/licenses"
cp licenses/*.txt "$app/Contents/Resources/licenses/"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>MacRunnerbar</string>
	<key>CFBundleDisplayName</key>
	<string>MacRunnerbar</string>
	<key>CFBundleIdentifier</key>
	<string>my.ijat.macrunnerbar</string>
	<key>CFBundleExecutable</key>
	<string>macrunnerbar</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>${version}</string>
	<key>CFBundleVersion</key>
	<string>${version}</string>
	<key>LSMinimumSystemVersion</key>
	<string>13.0</string>
	<key>LSUIElement</key>
	<true/>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
PLIST

# Ad-hoc signature: enough to run locally; no notarization (not needed for a personal tool).
codesign --force --sign - --timestamp=none "$app"
echo "built $app (version $version)"

if [ "$check" = 1 ]; then
  plutil -lint "$app/Contents/Info.plist"
  [ "$(plutil -extract LSUIElement raw "$app/Contents/Info.plist")" = "true" ] || { echo "LSUIElement is not true" >&2; exit 1; }
  [ "$(plutil -extract CFBundleIdentifier raw "$app/Contents/Info.plist")" = "my.ijat.macrunnerbar" ] || { echo "unexpected bundle id" >&2; exit 1; }
  codesign --verify --strict "$app"
  set +e
  "$app/Contents/MacOS/macrunnerbar" --smoke
  rc=$?
  set -e
  case "$rc" in
    0) echo "check ok: signature holds, LSUIElement set, smoke start/stop passed" ;;
    2) echo "check: no GUI session to run the smoke in (bundle and plist verified)" >&2; exit 2 ;;
    *) echo "check failed: smoke exited $rc" >&2; exit 1 ;;
  esac
fi
