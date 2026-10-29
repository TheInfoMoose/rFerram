#!/usr/bin/env bash
# rFerram macOS Release Packaging Script (.tar.gz, rFerram.app, .dmg)
# Usage: bash packaging/macos/package-macos.sh <version> <target-triple> [output-dir]
set -euo pipefail

RAW_VERSION="${1:-0.1.0}"
VERSION="${RAW_VERSION#v}"
TARGET="${2:-aarch64-apple-darwin}"
OUT_DIR="${3:-dist}"

WORKSPACE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${WORKSPACE_ROOT}"

mkdir -p "${OUT_DIR}"
OUT_DIR="$(cd "${OUT_DIR}" && pwd)"

if [[ -f "target/${TARGET}/release/rferram-ui" && ( ! -f "target/release/rferram-ui" || "target/${TARGET}/release/rferram-ui" -nt "target/release/rferram-ui" ) ]]; then
    BIN_DIR="target/${TARGET}/release"
elif [[ -f "target/release/rferram-ui" ]]; then
    BIN_DIR="target/release"
else
    echo "ERROR: Compiled binaries not found in target/${TARGET}/release or target/release" >&2
    exit 1
fi
echo "==> Packaging release binaries from: ${BIN_DIR}"

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "${WORK_DIR}"' EXIT

# ---------------------------------------------------------------------------
# 1. Portable Tarball (.tar.gz)
# ---------------------------------------------------------------------------
STAGE_TAR="${WORK_DIR}/rferram-${VERSION}-${TARGET}"
mkdir -p "${STAGE_TAR}/bin" "${STAGE_TAR}/share/launchd"

install -m 0755 "${BIN_DIR}/rferram-ui" "${STAGE_TAR}/bin/rferram-ui"
install -m 0755 "${BIN_DIR}/rferram-server" "${STAGE_TAR}/bin/rferram-server"
install -m 0644 "packaging/macos/com.theinfomoose.rferram-server.plist" "${STAGE_TAR}/share/launchd/"
install -m 0644 "README.md" "${STAGE_TAR}/README.md"
install -m 0644 "LICENSE" "${STAGE_TAR}/LICENSE"

TAR_ARCHIVE="${OUT_DIR}/rferram-${VERSION}-${TARGET}.tar.gz"
tar -C "${WORK_DIR}" -czf "${TAR_ARCHIVE}" "rferram-${VERSION}-${TARGET}"
echo "==> Created portable archive: ${TAR_ARCHIVE}"

# ---------------------------------------------------------------------------
# 2. macOS Application Bundle (rFerram.app) & Disk Image (.dmg)
# ---------------------------------------------------------------------------
DMG_STAGE="${WORK_DIR}/dmg-root"
APP_BUNDLE="${DMG_STAGE}/rFerram.app"
mkdir -p "${APP_BUNDLE}/Contents/MacOS" "${APP_BUNDLE}/Contents/Resources"

install -m 0755 "${BIN_DIR}/rferram-ui" "${APP_BUNDLE}/Contents/MacOS/rferram-ui"
install -m 0755 "${BIN_DIR}/rferram-server" "${APP_BUNDLE}/Contents/MacOS/rferram-server"
install -m 0644 "packaging/macos/com.theinfomoose.rferram-server.plist" "${APP_BUNDLE}/Contents/Resources/"
sed "s/__VERSION__/${VERSION}/g" "packaging/macos/Info.plist" > "${APP_BUNDLE}/Contents/Info.plist"

if command -v sips >/dev/null 2>&1 && command -v iconutil >/dev/null 2>&1; then
    ICONSET_DIR="${WORK_DIR}/rFerram.iconset"
    mkdir -p "${ICONSET_DIR}"
    if [[ -f "packaging/icons/rferram-1024.png" ]]; then
        SRC_ICON="packaging/icons/rferram-1024.png"
    else
        SRC_ICON="packaging/icons/rferram.png"
    fi
    sips -z 16 16     "${SRC_ICON}" --out "${ICONSET_DIR}/icon_16x16.png" >/dev/null
    sips -z 32 32     "${SRC_ICON}" --out "${ICONSET_DIR}/icon_16x16@2x.png" >/dev/null
    sips -z 32 32     "${SRC_ICON}" --out "${ICONSET_DIR}/icon_32x32.png" >/dev/null
    sips -z 64 64     "${SRC_ICON}" --out "${ICONSET_DIR}/icon_32x32@2x.png" >/dev/null
    sips -z 128 128   "${SRC_ICON}" --out "${ICONSET_DIR}/icon_128x128.png" >/dev/null
    sips -z 256 256   "${SRC_ICON}" --out "${ICONSET_DIR}/icon_128x128@2x.png" >/dev/null
    sips -z 256 256   "${SRC_ICON}" --out "${ICONSET_DIR}/icon_256x256.png" >/dev/null
    sips -z 512 512   "${SRC_ICON}" --out "${ICONSET_DIR}/icon_256x256@2x.png" >/dev/null
    sips -z 512 512   "${SRC_ICON}" --out "${ICONSET_DIR}/icon_512x512.png" >/dev/null
    sips -z 1024 1024 "${SRC_ICON}" --out "${ICONSET_DIR}/icon_512x512@2x.png" >/dev/null
    iconutil -c icns "${ICONSET_DIR}" -o "${APP_BUNDLE}/Contents/Resources/rFerram.icns"
elif [[ -f "packaging/icons/rferram.icns" ]]; then
    install -m 0644 "packaging/icons/rferram.icns" "${APP_BUNDLE}/Contents/Resources/rFerram.icns"
fi

if command -v codesign >/dev/null 2>&1; then
    codesign --force --deep --sign - "${APP_BUNDLE}"
fi

ln -s /Applications "${DMG_STAGE}/Applications"

if command -v hdiutil >/dev/null 2>&1; then
    DMG_FILE="${OUT_DIR}/rFerram-${VERSION}-${TARGET}.dmg"
    hdiutil create \
        -volname "rFerram ${VERSION}" \
        -srcfolder "${DMG_STAGE}" \
        -ov \
        -format UDZO \
        "${DMG_FILE}"
    echo "==> Created macOS DMG: ${DMG_FILE}"
elif [[ "${CI:-false}" == "true" ]]; then
    echo "ERROR: hdiutil not found in CI environment" >&2
    exit 1
else
    echo "WARN: hdiutil not found; skipping .dmg creation"
fi
