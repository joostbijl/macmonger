#!/bin/sh
# Builds target/MacMonger.app. Run on a Mac:
#   ./build.sh            build the app bundle
#   ./build.sh install    build and copy it to /Applications
set -eu
cd "$(dirname "$0")"

cargo build --release

APP=target/MacMonger.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/macmonger "$APP/Contents/MacOS/MacMonger"
cp packaging/Info.plist "$APP/Contents/Info.plist"
cp packaging/MacMonger.icns "$APP/Contents/Resources/MacMonger.icns"

# Signing. macOS ties privacy permissions (Full Disk Access) to the signature.
# An ad-hoc signature is different for every build, so the permission is lost
# on each rebuild. A certificate named "MacMonger Signing" in the keychain (or
# any identity given in CODESIGN_IDENTITY) keeps it stable. See the README.
IDENTITY="${CODESIGN_IDENTITY:-}"
if [ -z "$IDENTITY" ] && security find-identity -p codesigning 2>/dev/null | grep -q '"MacMonger Signing"'; then
    IDENTITY="MacMonger Signing"
fi
if [ -n "$IDENTITY" ]; then
    codesign --force --sign "$IDENTITY" --identifier com.jaxeadv.macmonger "$APP"
    echo "Built $APP (signed with: $IDENTITY)"
else
    codesign --force --sign - "$APP"
    echo "Built $APP (ad-hoc signed: Full Disk Access must be granted again after every build)"
fi

if [ "${1:-}" = "install" ]; then
    rm -rf /Applications/MacMonger.app
    cp -R "$APP" /Applications/MacMonger.app
    echo "Installed /Applications/MacMonger.app"
fi
