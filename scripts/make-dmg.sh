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
# The bundle ships all three executables, at the same version:
#   Contents/MacOS/mns-gui   desktop shell
#   Contents/MacOS/mns       CLI, the one an agent must call
#   Contents/MacOS/mns-help  standalone help viewer (tray menu opens it)
# Agents resolve the CLI from the installed app, never from a guessable path
# such as ~/mns or a stale copy on PATH. $HOME/QuickBins/mns has already drifted
# to 0.7.0 while the app was 0.8.0, so a PATH lookup is not a trustworthy
# version check; the bundle path below always matches the shipped GUI.
#
# Output:
#   target/dist/MNS.app
#   target/dist/MNS-<version>-<arch>.dmg

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP_NAME="MNS"
EXECUTABLE="mns-gui"
CLI="mns"
HELP="mns-help"
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
    echo "==> building $CLI $VERSION ($ARCH, release, $TARGET)"
    cargo build --release --bin "$CLI" --target "$TARGET"
    CLI_BIN="$ROOT/target/$TARGET/release/$CLI"
    echo "==> building $HELP $VERSION ($ARCH, release, $TARGET)"
    cargo build --release --features gui --bin "$HELP" --target "$TARGET"
    HELP_BIN="$ROOT/target/$TARGET/release/$HELP"
else
    echo "==> building $EXECUTABLE $VERSION ($ARCH, release)"
    cargo build --release --features gui --bin "$EXECUTABLE"
    BIN="$ROOT/target/release/$EXECUTABLE"
    echo "==> building $CLI $VERSION ($ARCH, release)"
    cargo build --release --bin "$CLI"
    CLI_BIN="$ROOT/target/release/$CLI"
    echo "==> building $HELP $VERSION ($ARCH, release)"
    cargo build --release --features gui --bin "$HELP"
    HELP_BIN="$ROOT/target/release/$HELP"
fi

for artifact in "$BIN" "$CLI_BIN" "$HELP_BIN"; do
    if [[ ! -x "$artifact" ]]; then
        echo "missing build artifact: $artifact" >&2
        exit 1
    fi
done

# The CLI is built with default features on purpose: it must be byte-identical
# to the standalone CLI users get from `cargo build --release --bin mns`, not a
# GUI-linked copy. That costs one extra lib compile per packaging run.
#
# A cross-compiled CLI cannot be executed here, so the version gate only runs
# when the artifact matches the host architecture.
HOST_ARCH="$(arch_name "$(uname -m)")"
if [[ "$ARCH" == "$HOST_ARCH" ]]; then
    CLI_VERSION="$("$CLI_BIN" --version 2>/dev/null | head -1)"
    if [[ "$CLI_VERSION" != *"$VERSION"* ]]; then
        echo "CLI version mismatch: expected $VERSION, got '$CLI_VERSION'" >&2
        exit 1
    fi
else
    echo "==> skipping CLI version gate ($ARCH build on $HOST_ARCH host)"
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
# The CLI lives next to the GUI so the installed path is stable and short:
#   /Applications/MNS.app/Contents/MacOS/mns
# Contents/MacOS is the one place macOS expects extra executables in a bundle,
# and it means agents never have to guess a home-directory path or trust PATH.
cp "$CLI_BIN" "$APP/Contents/MacOS/$CLI"
# Help viewer + its runtime screenshots. Screenshots go to Contents/Resources:
# Contents/MacOS must contain only Mach-O executables, otherwise codesign sees
# each PNG as an unsigned nested code object and `--verify --strict` fails.
cp "$HELP_BIN" "$APP/Contents/MacOS/$HELP"
if [[ -d "$ROOT/help/images" ]]; then
    mkdir -p "$APP/Contents/Resources/help"
    cp -R "$ROOT/help/images" "$APP/Contents/Resources/help/images"
fi
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
#
# The embedded CLI is signed explicitly BEFORE the bundle. Signing a bundle does
# not reliably seal an extra Mach-O that is not the main executable, and a
# helper with a stale or absent signature fails Gatekeeper on a copied DMG.
if [[ -n "${CODESIGN_ID:-}" ]]; then
    echo "==> signing with $CODESIGN_ID"
    codesign --force --options runtime --timestamp \
        --sign "$CODESIGN_ID" "$APP/Contents/MacOS/$CLI"
    codesign --force --options runtime --timestamp \
        --sign "$CODESIGN_ID" "$APP"
else
    echo "==> signing ad-hoc (local use)"
    codesign --force --sign - "$APP/Contents/MacOS/$CLI"
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
echo
echo "   GUI  $APP/Contents/MacOS/$EXECUTABLE"
echo "   CLI  $APP/Contents/MacOS/$CLI  (${CLI_VERSION:-not executable on this host})"
echo "   装到 /Applications 后，agent 应调用："
echo "   /Applications/$APP_NAME.app/Contents/MacOS/$CLI"
