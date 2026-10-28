#!/usr/bin/env bash
# rFerram Linux Release Packaging Script (.tar.gz, .deb, .rpm)
# Usage: bash packaging/linux/package-linux.sh <version> <target-triple> [output-dir]
set -euo pipefail

RAW_VERSION="${1:-0.1.0}"
VERSION="${RAW_VERSION#v}"
# Sanitize version for Debian/RPM (replace '-' with '~' for deb and '_' for rpm if needed)
DEB_VERSION="${VERSION//-/\~}"
RPM_VERSION="${VERSION//-/_}"
TARGET="${2:-x86_64-unknown-linux-gnu}"
OUT_DIR="${3:-dist}"

WORKSPACE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${WORKSPACE_ROOT}"

mkdir -p "${OUT_DIR}"
OUT_DIR="$(cd "${OUT_DIR}" && pwd)"

if [[ -f "target/${TARGET}/release/rferram-ui" ]]; then
    BIN_DIR="target/${TARGET}/release"
elif [[ -f "target/release/rferram-ui" ]]; then
    BIN_DIR="target/release"
else
    echo "ERROR: Compiled binaries not found in target/${TARGET}/release or target/release" >&2
    exit 1
fi

case "${TARGET}" in
    x86_64-*)
        DEB_ARCH="amd64"
        RPM_ARCH="x86_64"
        ;;
    aarch64-*)
        DEB_ARCH="arm64"
        RPM_ARCH="aarch64"
        ;;
    *)
        echo "ERROR: Unsupported Linux target architecture: ${TARGET}" >&2
        exit 1
        ;;
esac

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "${WORK_DIR}"' EXIT

# ---------------------------------------------------------------------------
# 1. Portable Tarball (.tar.gz)
# ---------------------------------------------------------------------------
STAGE_TAR="${WORK_DIR}/rferram-${VERSION}-${TARGET}"
mkdir -p "${STAGE_TAR}/bin" "${STAGE_TAR}/share/applications" "${STAGE_TAR}/share/icons" "${STAGE_TAR}/share/systemd"

install -m 0755 "${BIN_DIR}/rferram-ui" "${STAGE_TAR}/bin/rferram-ui"
install -m 0755 "${BIN_DIR}/rferram-server" "${STAGE_TAR}/bin/rferram-server"
install -m 0644 "packaging/linux/com.theinfomoose.rFerram.desktop" "${STAGE_TAR}/share/applications/"
install -m 0644 "packaging/linux/com.theinfomoose.rFerram.metainfo.xml" "${STAGE_TAR}/share/applications/"
install -m 0644 "packaging/icons/rferram.png" "${STAGE_TAR}/share/icons/com.theinfomoose.rFerram.png"
install -m 0644 "packaging/linux/rferram-server.service" "${STAGE_TAR}/share/systemd/"
install -m 0644 "README.md" "${STAGE_TAR}/README.md"
install -m 0644 "LICENSE" "${STAGE_TAR}/LICENSE"

TAR_ARCHIVE="${OUT_DIR}/rferram-${VERSION}-${TARGET}.tar.gz"
tar -C "${WORK_DIR}" -czf "${TAR_ARCHIVE}" "rferram-${VERSION}-${TARGET}"
echo "==> Created portable archive: ${TAR_ARCHIVE}"

# ---------------------------------------------------------------------------
# 2. Debian / Ubuntu / Pop!_OS Package (.deb)
# ---------------------------------------------------------------------------
if command -v dpkg-deb >/dev/null 2>&1; then
    DEB_ROOT="${WORK_DIR}/deb-pkg"
    mkdir -p \
        "${DEB_ROOT}/DEBIAN" \
        "${DEB_ROOT}/usr/bin" \
        "${DEB_ROOT}/usr/share/applications" \
        "${DEB_ROOT}/usr/share/metainfo" \
        "${DEB_ROOT}/usr/share/icons/hicolor/256x256/apps" \
        "${DEB_ROOT}/lib/systemd/system" \
        "${DEB_ROOT}/usr/share/doc/rferram"

    install -m 0755 "${BIN_DIR}/rferram-ui" "${DEB_ROOT}/usr/bin/rferram-ui"
    install -m 0755 "${BIN_DIR}/rferram-server" "${DEB_ROOT}/usr/bin/rferram-server"
    install -m 0644 "packaging/linux/com.theinfomoose.rFerram.desktop" "${DEB_ROOT}/usr/share/applications/com.theinfomoose.rFerram.desktop"
    install -m 0644 "packaging/linux/com.theinfomoose.rFerram.metainfo.xml" "${DEB_ROOT}/usr/share/metainfo/com.theinfomoose.rFerram.metainfo.xml"
    install -m 0644 "packaging/icons/rferram.png" "${DEB_ROOT}/usr/share/icons/hicolor/256x256/apps/com.theinfomoose.rFerram.png"
    install -m 0644 "packaging/linux/rferram-server.service" "${DEB_ROOT}/lib/systemd/system/rferram-server.service"
    install -m 0644 "README.md" "${DEB_ROOT}/usr/share/doc/rferram/README.md"
    install -m 0644 "LICENSE" "${DEB_ROOT}/usr/share/doc/rferram/copyright"

    INSTALLED_KB="$(du -sk "${DEB_ROOT}/usr" "${DEB_ROOT}/lib" | awk '{s+=$1} END {print s}')"

    cat > "${DEB_ROOT}/DEBIAN/control" <<EOF
Package: rferram
Version: ${DEB_VERSION}
Section: net
Priority: optional
Architecture: ${DEB_ARCH}
Installed-Size: ${INSTALLED_KB}
Depends: libc6, libwayland-client0, libdrm2, libgbm1, libxkbcommon0, libdbus-1-3
Maintainer: TheInfoMoose <https://github.com/TheInfoMoose/rFerram>
Homepage: https://github.com/TheInfoMoose/rFerram
Description: Zero-copy QUIC remote display viewer and host daemon for COSMIC
 rFerram is a hyper-optimized, zero-copy, cross-platform remote display
 application built in pure Rust for COSMIC Desktop and modern systems.
 Includes both the rferram-ui viewer and the rferram-server host daemon.
EOF

    DEB_FILE="${OUT_DIR}/rferram_${VERSION}_${DEB_ARCH}.deb"
    dpkg-deb --build --root-owner-group "${DEB_ROOT}" "${DEB_FILE}"
    echo "==> Created Debian package: ${DEB_FILE}"
elif [[ "${CI:-false}" == "true" ]]; then
    echo "ERROR: dpkg-deb not found in CI environment" >&2
    exit 1
else
    echo "WARN: dpkg-deb not found; skipping .deb generation"
fi

# ---------------------------------------------------------------------------
# 3. RHEL / Fedora / SUSE Package (.rpm)
# ---------------------------------------------------------------------------
if command -v rpmbuild >/dev/null 2>&1; then
    RPM_TOPDIR="${WORK_DIR}/rpmbuild"
    mkdir -p "${RPM_TOPDIR}"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}

    SPEC_FILE="${RPM_TOPDIR}/SPECS/rferram.spec"
    cat > "${SPEC_FILE}" <<EOF
%global debug_package %{nil}
%define _build_id_links none

Name:           rferram
Version:        ${RPM_VERSION}
Release:        1
Summary:        Zero-copy QUIC remote display viewer and host daemon for COSMIC
License:        GPL-3.0-only
URL:            https://github.com/TheInfoMoose/rFerram
BuildArch:      ${RPM_ARCH}

%description
rFerram is a hyper-optimized, zero-copy, cross-platform remote display
application built in pure Rust for COSMIC Desktop and modern platforms.
Includes both the rferram-ui viewer and the rferram-server host daemon.

%install
mkdir -p %{buildroot}/usr/bin
mkdir -p %{buildroot}/usr/share/applications
mkdir -p %{buildroot}/usr/share/metainfo
mkdir -p %{buildroot}/usr/share/icons/hicolor/256x256/apps
mkdir -p %{buildroot}/usr/lib/systemd/system
mkdir -p %{buildroot}/usr/share/doc/rferram

install -m 0755 "${WORKSPACE_ROOT}/${BIN_DIR}/rferram-ui" %{buildroot}/usr/bin/rferram-ui
install -m 0755 "${WORKSPACE_ROOT}/${BIN_DIR}/rferram-server" %{buildroot}/usr/bin/rferram-server
install -m 0644 "${WORKSPACE_ROOT}/packaging/linux/com.theinfomoose.rFerram.desktop" %{buildroot}/usr/share/applications/com.theinfomoose.rFerram.desktop
install -m 0644 "${WORKSPACE_ROOT}/packaging/linux/com.theinfomoose.rFerram.metainfo.xml" %{buildroot}/usr/share/metainfo/com.theinfomoose.rFerram.metainfo.xml
install -m 0644 "${WORKSPACE_ROOT}/packaging/icons/rferram.png" %{buildroot}/usr/share/icons/hicolor/256x256/apps/com.theinfomoose.rFerram.png
install -m 0644 "${WORKSPACE_ROOT}/packaging/linux/rferram-server.service" %{buildroot}/usr/lib/systemd/system/rferram-server.service
install -m 0644 "${WORKSPACE_ROOT}/README.md" %{buildroot}/usr/share/doc/rferram/README.md
install -m 0644 "${WORKSPACE_ROOT}/LICENSE" %{buildroot}/usr/share/doc/rferram/LICENSE

%files
/usr/bin/rferram-ui
/usr/bin/rferram-server
/usr/share/applications/com.theinfomoose.rFerram.desktop
/usr/share/metainfo/com.theinfomoose.rFerram.metainfo.xml
/usr/share/icons/hicolor/256x256/apps/com.theinfomoose.rFerram.png
/usr/lib/systemd/system/rferram-server.service
/usr/share/doc/rferram/README.md
/usr/share/doc/rferram/LICENSE
EOF

    rpmbuild --define "_topdir ${RPM_TOPDIR}" --target "${RPM_ARCH}" -bb "${SPEC_FILE}"
    FOUND_RPM="$(find "${RPM_TOPDIR}/RPMS" -name "*.rpm" | head -n 1)"
    RPM_OUT="${OUT_DIR}/rferram-${VERSION}-1.${RPM_ARCH}.rpm"
    cp "${FOUND_RPM}" "${RPM_OUT}"
    echo "==> Created RPM package: ${RPM_OUT}"
elif [[ "${CI:-false}" == "true" ]]; then
    echo "ERROR: rpmbuild not found in CI environment" >&2
    exit 1
else
    echo "WARN: rpmbuild not found; skipping .rpm generation"
fi
