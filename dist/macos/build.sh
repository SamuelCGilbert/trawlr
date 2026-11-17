#!/bin/bash
# Builds Trawlr.app and its installer package for Apple Silicon and Intel
# Macs, into dist/out:
#   dist/macos/build.sh [--no-pkg]
# Needs Rust (rustup) and Qt 6 for macOS with qmake and macdeployqt on PATH.
#
# There is no Apple Developer ID behind it: the app is signed ad hoc, which
# Apple Silicon requires, and macOS asks people to confirm the first opening
# of the installer (see the README).
set -euo pipefail
cd "$(dirname "$0")/../.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
OUT=dist/out
STAGE=$OUT/macos-stage
export MACOSX_DEPLOYMENT_TARGET=12.0

rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
for t in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --release --locked --target $t
done
mkdir -p $OUT/universal
for b in trawlrd trawlr; do
    lipo -create -output $OUT/universal/$b target/aarch64-apple-darwin/release/$b target/x86_64-apple-darwin/release/$b
done

rm -rf gui/build-macos && mkdir -p gui/build-macos
(cd gui/build-macos && qmake ../trawlr.pro QMAKE_APPLE_DEVICE_ARCHS="x86_64 arm64" && make -j"$(sysctl -n hw.ncpu)")

rm -rf "$STAGE" && mkdir -p "$STAGE/Applications"
APP="$STAGE/Applications/Trawlr.app"
cp -R gui/build-macos/trawlr-gui.app "$APP"
macdeployqt "$APP" -verbose=1
cp $OUT/universal/trawlrd $OUT/universal/trawlr "$APP/Contents/MacOS/"
cp dist/macos/uninstall.sh LICENSE "$APP/Contents/Resources/"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $VERSION" -c "Set :CFBundleVersion $VERSION" "$APP/Contents/Info.plist"
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"
for b in trawlr-gui trawlrd trawlr; do echo "$b: $(lipo -archs "$APP/Contents/MacOS/$b")"; done

[ "${1:-}" = --no-pkg ] && exit 0

# Installed where it says, even if a copy of the app is elsewhere.
pkgbuild --analyze --root "$STAGE" $OUT/components.plist
/usr/libexec/PlistBuddy -c "Set :0:BundleIsRelocatable false" $OUT/components.plist
pkgbuild --root "$STAGE" --component-plist $OUT/components.plist --identifier io.trawlr.Trawlr --version "$VERSION" \
    --scripts dist/macos/scripts --install-location / "$OUT/trawlr-$VERSION-macos.pkg"
ls -l "$OUT/trawlr-$VERSION-macos.pkg"
