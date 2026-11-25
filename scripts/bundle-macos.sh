#!/bin/sh
# Builds "dist/Studio Record.app" (release) and signs it.
#
# Signing matters for permissions: macOS remembers "Allow camera" for one
# code signature. Ad-hoc signatures change with every build, so approvals
# seem to get stuck. This script signs with a stable identity — your
# Developer ID or Apple Development certificate if one is in the keychain
# (override with SIGN_IDENTITY=...), ad-hoc only as a last resort.
# For Developer ID distribution, use SIGN_TIMESTAMP=--timestamp.
set -e
cd "$(dirname "$0")/.."
cargo build --locked --release
APP="dist/Studio Record.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/studio-record "$APP/Contents/MacOS/studio-record"
cp packaging/macos/Info.plist "$APP/Contents/Info.plist"
mkdir -p "$APP/Contents/Resources/Licenses"
cp LICENSE-MIT LICENSE-APACHE assets/fonts/*-OFL.txt "$APP/Contents/Resources/Licenses/"
# Optional: ship FFmpeg inside the app so recording works with no setup.
if [ -n "$BUNDLE_FFMPEG" ] && [ -x "$BUNDLE_FFMPEG" ]; then cp "$BUNDLE_FFMPEG" "$APP/Contents/Resources/ffmpeg"; fi

if [ -z "$SIGN_IDENTITY" ]; then
  SIGN_IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null | grep -m1 "Developer ID Application" | sed -E 's/.*"(.*)"/\1/')
fi
if [ -z "$SIGN_IDENTITY" ]; then
  SIGN_IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null | grep -m1 "Apple Development" | sed -E 's/.*"(.*)"/\1/')
fi
if [ -n "$SIGN_IDENTITY" ]; then
  codesign --force --options runtime "${SIGN_TIMESTAMP:---timestamp=none}" --entitlements packaging/macos/entitlements.plist --sign "$SIGN_IDENTITY" "$APP"
  echo "Signed with a stable identity — permission approvals will stick across updates."
else
  codesign --force --sign - "$APP"
  echo "WARNING: ad-hoc signed. Camera approvals reset on every rebuild."
fi
if [ -n "$INSTALL" ]; then
  rm -rf "/Applications/Studio Record.app" && cp -R "$APP" /Applications/ && echo "Installed to /Applications"
fi
echo "Built $APP"
