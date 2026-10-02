#!/usr/bin/env bash
# Builds "Dither Channels.app" (and optionally a .dmg) into target/macos/.
#
# Usage: macos/build-app.sh [--universal] [--dmg]
#   --universal  Also build for Intel Macs and merge both into one binary.
#                Needs: rustup target add x86_64-apple-darwin
#   --dmg        Also pack the app into a disk image for sharing.
#
# BUNDLE_ID=... overrides the bundle identifier (default local.dither-channels).
# The app is signed ad-hoc: it runs on this Mac, but other Macs will ask to
# confirm opening it (right-click → Open) since it isn't notarized.
set -euo pipefail
cd "$(dirname "$0")/.."

APP_NAME="Dither Channels"
BIN=dither_gui
BUNDLE_ID="${BUNDLE_ID:-local.dither-channels}"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

universal=false
dmg=false
for arg in "$@"; do
    case $arg in
        --universal) universal=true ;;
        --dmg) dmg=true ;;
        *) echo "Unknown option: $arg" >&2; exit 1 ;;
    esac
done

OUT=target/macos
APP="$OUT/$APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

if $universal; then
    for target in aarch64-apple-darwin x86_64-apple-darwin; do
        cargo build --release --bin "$BIN" --target "$target"
    done
    lipo -create -output "$APP/Contents/MacOS/$BIN" \
        "target/aarch64-apple-darwin/release/$BIN" "target/x86_64-apple-darwin/release/$BIN"
else
    cargo build --release --bin "$BIN"
    cp "target/release/$BIN" "$APP/Contents/MacOS/"
fi

# Icon: every size macOS asks for, from the 1024px source.
ICONSET="$OUT/AppIcon.iconset"
rm -rf "$ICONSET"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
    sips -z $size $size macos/icon.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    sips -z $((size * 2)) $((size * 2)) macos/icon.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
rm -rf "$ICONSET"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>$APP_NAME</string>
    <key>CFBundleDisplayName</key>
    <string>$APP_NAME</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleExecutable</key>
    <string>$BIN</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.graphics-design</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
</dict>
</plist>
EOF

codesign --force --sign - "$APP"
echo "Built $APP"

if $dmg; then
    # The disk image shows the app next to an Applications shortcut to drag it onto.
    STAGE="$OUT/dmg"
    rm -rf "$STAGE"
    mkdir -p "$STAGE"
    cp -R "$APP" "$STAGE/"
    ln -s /Applications "$STAGE/Applications"
    DMG="$OUT/$APP_NAME $VERSION.dmg"
    rm -f "$DMG"
    hdiutil create -quiet -volname "$APP_NAME" -srcfolder "$STAGE" -format UDZO "$DMG"
    rm -rf "$STAGE"
    echo "Built $DMG"
fi
