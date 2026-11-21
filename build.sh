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
# Ad-hoc signature: required for Apple Silicon binaries to run.
codesign --force --sign - "$APP"
echo "Built $APP"

if [ "${1:-}" = "install" ]; then
    rm -rf /Applications/MacMonger.app
    cp -R "$APP" /Applications/MacMonger.app
    echo "Installed /Applications/MacMonger.app"
fi
