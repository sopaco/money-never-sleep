#!/usr/bin/env bash
# Builds MNS.app from the release binary and packages it into a .dmg.
#
#   ./scripts/make-dmg.sh                  # ad-hoc signature, runs on this Mac
#   CODESIGN_ID="Developer ID Application: Name (TEAMID)" ./scripts/make-dmg.sh
#
# Ad-hoc signing is enough for local use and for handing the DMG to another Mac
# on the same team (they clear the quarantine flag once). Distributing outside
# the team requires a Developer ID plus notarisation — see docs/gui-release.md.
#
# Output:
#   target/dist/MNS.app
#   target/dist/MNS-<version>-<arch>.dmg

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP_NAME="MNS"
EXECUTABLE="mns-gui"
BUNDLE_ID="${BUNDLE_ID:-com.sopaco.mns}"
VERSION="$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' Cargo.toml | head -1)"
DIST="$ROOT/target/dist"

# Apple spells the architecture arm64, Rust spells it aarch64. The DMG name
# should use Apple's spelling so users know which build they downloaded.
arch_name() {
    case "$1" in
        aarch64*) echo "arm64" ;;
        x86_64*) echo "x86_64" ;;
        *) echo "$1" ;;
    esac
}

if [[ -n "${TARGET:-}" ]]; then
    ARCH="$(arch_name "${TARGET%%-apple-*}")"
else
    ARCH="$(arch_name "$(uname -m)")"
fi

APP="$DIST/$APP_NAME.app"
DMG="$DIST/$APP_NAME-$VERSION-$ARCH.dmg"

if [[ -z "$VERSION" ]]; then
    echo "cannot read version from Cargo.toml" >&2
    exit 1
fi

if [[ -n "${TARGET:-}" ]]; then
    echo "==> building $EXECUTABLE $VERSION ($ARCH, release, $TARGET)"
    cargo build --release --features gui --bin "$EXECUTABLE" --target "$TARGET"
    BIN="$ROOT/target/$TARGET/release/$EXECUTABLE"
else
    echo "==> building $EXECUTABLE $VERSION ($ARCH, release)"
    cargo build --release --features gui --bin "$EXECUTABLE"
    BIN="$ROOT/target/release/$EXECUTABLE"
fi

# The deployment target actually baked into the binary is the only honest
# source for LSMinimumSystemVersion; a lower value crashes on launch, a higher
# one needlessly locks old Macs out.
MIN_MACOS="$(otool -l "$BIN" | awk '/LC_BUILD_VERSION/{found=1} found&&/minos/{print $2; exit}')"
MIN_MACOS="${MIN_MACOS:-12.0}"

echo "==> assembling $APP_NAME.app (min macOS $MIN_MACOS)"
rm -rf "$APP" "$DIST/dmg-root"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$BIN" "$APP/Contents/MacOS/$EXECUTABLE"
cp "$ROOT/assets/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>zh_CN</string>
    <key>CFBundleExecutable</key>
    <string>$EXECUTABLE</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>$APP_NAME</string>
    <key>CFBundleDisplayName</key>
    <string>MNS 投资助手</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>LSMinimumSystemVersion</key>
    <string>$MIN_MACOS</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.finance</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>© 2026 Sopaco. MIT licensed.</string>
</dict>
</plist>
PLIST

# Signing identity: ad-hoc by default. Hardened runtime is only meaningful with
# a real certificate, so it is enabled together with Developer ID.
if [[ -n "${CODESIGN_ID:-}" ]]; then
    echo "==> signing with $CODESIGN_ID"
    codesign --force --options runtime --timestamp \
        --sign "$CODESIGN_ID" "$APP"
else
    echo "==> signing ad-hoc (local use)"
    codesign --force --sign - "$APP"
fi
codesign --verify --strict --verbose=2 "$APP"

echo "==> building $DMG"
ROOT_STAGE="$DIST/dmg-root"
mkdir -p "$ROOT_STAGE"
cp -R "$APP" "$ROOT_STAGE/"
ln -s /Applications "$ROOT_STAGE/Applications"
rm -f "$DMG"
hdiutil create \
    -volname "$APP_NAME" \
    -srcfolder "$ROOT_STAGE" \
    -fs HFS+ \
    -format UDZO \
    -imagekey zlib-level=9 \
    -ov \
    "$DMG" >/dev/null

rm -rf "$ROOT_STAGE"

SIZE="$(du -h "$DMG" | cut -f1 | tr -d ' ')"
echo
echo "✅ $DMG"
echo "   $SIZE · $(shasum -a 256 "$DMG" | cut -d' ' -f1 | cut -c1-16)…"
