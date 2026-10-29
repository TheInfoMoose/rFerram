#!/usr/bin/env bash
# rFerram Linux Release Packaging Script (.tar.gz, .deb, .rpm, .AppImage)
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

if [[ -f "target/${TARGET}/release/rferram-ui" && ( ! -f "target/release/rferram-ui" || "target/${TARGET}/release/rferram-ui" -nt "target/release/rferram-ui" ) ]]; then
    BIN_DIR="target/${TARGET}/release"
elif [[ -f "target/release/rferram-ui" ]]; then
    BIN_DIR="target/release"
else
    echo "ERROR: Compiled binaries not found in target/${TARGET}/release or target/release" >&2
    exit 1
fi
echo "==> Packaging release binaries from: ${BIN_DIR}"

case "${TARGET}" in
    x86_64-*)
        DEB_ARCH="amd64"
        RPM_ARCH="x86_64"
        APPIMAGE_ARCH="x86_64"
        ;;
    aarch64-*)
        DEB_ARCH="arm64"
        RPM_ARCH="aarch64"
        APPIMAGE_ARCH="aarch64"
        ;;
    *)
        echo "ERROR: Unsupported Linux target architecture: ${TARGET}" >&2
        exit 1
        ;;
esac

HICOLOR_SIZES=(16 32 48 64 128 256 512)

install_hicolor_icons() {
    local dest_hicolor="$1"
    for sz in "${HICOLOR_SIZES[@]}"; do
        local src_icon="packaging/icons/hicolor/${sz}x${sz}/apps/com.theinfomoose.rFerram.png"
        if [[ ! -f "${src_icon}" ]]; then
            src_icon="packaging/icons/rferram.png"
        fi
        mkdir -p "${dest_hicolor}/${sz}x${sz}/apps"
        install -m 0644 "${src_icon}" "${dest_hicolor}/${sz}x${sz}/apps/com.theinfomoose.rFerram.png"
    done
    if [[ -f "packaging/icons/hicolor/scalable/apps/com.theinfomoose.rFerram-symbolic.svg" ]]; then
        mkdir -p "${dest_hicolor}/scalable/apps"
        install -m 0644 \
            "packaging/icons/hicolor/scalable/apps/com.theinfomoose.rFerram-symbolic.svg" \
            "${dest_hicolor}/scalable/apps/com.theinfomoose.rFerram-symbolic.svg"
    fi
}

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
install_hicolor_icons "${STAGE_TAR}/share/icons/hicolor"
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
        "${DEB_ROOT}/lib/systemd/system" \
        "${DEB_ROOT}/usr/share/doc/rferram"

    install -m 0755 "${BIN_DIR}/rferram-ui" "${DEB_ROOT}/usr/bin/rferram-ui"
    install -m 0755 "${BIN_DIR}/rferram-server" "${DEB_ROOT}/usr/bin/rferram-server"
    install -m 0644 "packaging/linux/com.theinfomoose.rFerram.desktop" "${DEB_ROOT}/usr/share/applications/com.theinfomoose.rFerram.desktop"
    install -m 0644 "packaging/linux/com.theinfomoose.rFerram.metainfo.xml" "${DEB_ROOT}/usr/share/metainfo/com.theinfomoose.rFerram.metainfo.xml"
    install_hicolor_icons "${DEB_ROOT}/usr/share/icons/hicolor"
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
mkdir -p %{buildroot}/usr/lib/systemd/system
mkdir -p %{buildroot}/usr/share/doc/rferram

install -m 0755 "${WORKSPACE_ROOT}/${BIN_DIR}/rferram-ui" %{buildroot}/usr/bin/rferram-ui
install -m 0755 "${WORKSPACE_ROOT}/${BIN_DIR}/rferram-server" %{buildroot}/usr/bin/rferram-server
install -m 0644 "${WORKSPACE_ROOT}/packaging/linux/com.theinfomoose.rFerram.desktop" %{buildroot}/usr/share/applications/com.theinfomoose.rFerram.desktop
install -m 0644 "${WORKSPACE_ROOT}/packaging/linux/com.theinfomoose.rFerram.metainfo.xml" %{buildroot}/usr/share/metainfo/com.theinfomoose.rFerram.metainfo.xml
for sz in 16 32 48 64 128 256 512; do
    mkdir -p %{buildroot}/usr/share/icons/hicolor/\${sz}x\${sz}/apps
    install -m 0644 "${WORKSPACE_ROOT}/packaging/icons/hicolor/\${sz}x\${sz}/apps/com.theinfomoose.rFerram.png" %{buildroot}/usr/share/icons/hicolor/\${sz}x\${sz}/apps/com.theinfomoose.rFerram.png
done
mkdir -p %{buildroot}/usr/share/icons/hicolor/scalable/apps
install -m 0644 "${WORKSPACE_ROOT}/packaging/icons/hicolor/scalable/apps/com.theinfomoose.rFerram-symbolic.svg" %{buildroot}/usr/share/icons/hicolor/scalable/apps/com.theinfomoose.rFerram-symbolic.svg
install -m 0644 "${WORKSPACE_ROOT}/packaging/linux/rferram-server.service" %{buildroot}/usr/lib/systemd/system/rferram-server.service
install -m 0644 "${WORKSPACE_ROOT}/README.md" %{buildroot}/usr/share/doc/rferram/README.md
install -m 0644 "${WORKSPACE_ROOT}/LICENSE" %{buildroot}/usr/share/doc/rferram/LICENSE

%files
/usr/bin/rferram-ui
/usr/bin/rferram-server
/usr/share/applications/com.theinfomoose.rFerram.desktop
/usr/share/metainfo/com.theinfomoose.rFerram.metainfo.xml
/usr/share/icons/hicolor/*/apps/com.theinfomoose.rFerram*
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

# ---------------------------------------------------------------------------
# 4. Portable Linux AppImage (.AppImage)
# ---------------------------------------------------------------------------
APPDIR="${WORK_DIR}/rFerram.AppDir"
mkdir -p \
    "${APPDIR}/usr/bin" \
    "${APPDIR}/usr/share/applications" \
    "${APPDIR}/usr/share/metainfo" \
    "${APPDIR}/usr/lib/systemd/system" \
    "${APPDIR}/usr/share/doc/rferram"

install -m 0755 "packaging/linux/AppRun" "${APPDIR}/AppRun"
install -m 0644 "packaging/linux/com.theinfomoose.rFerram.desktop" "${APPDIR}/com.theinfomoose.rFerram.desktop"
if [[ -f "packaging/icons/hicolor/256x256/apps/com.theinfomoose.rFerram.png" ]]; then
    install -m 0644 "packaging/icons/hicolor/256x256/apps/com.theinfomoose.rFerram.png" "${APPDIR}/com.theinfomoose.rFerram.png"
else
    install -m 0644 "packaging/icons/rferram.png" "${APPDIR}/com.theinfomoose.rFerram.png"
fi
ln -sf "com.theinfomoose.rFerram.png" "${APPDIR}/.DirIcon"

install -m 0755 "${BIN_DIR}/rferram-ui" "${APPDIR}/usr/bin/rferram-ui"
install -m 0755 "${BIN_DIR}/rferram-server" "${APPDIR}/usr/bin/rferram-server"
install -m 0644 "packaging/linux/com.theinfomoose.rFerram.desktop" "${APPDIR}/usr/share/applications/com.theinfomoose.rFerram.desktop"
install -m 0644 "packaging/linux/com.theinfomoose.rFerram.metainfo.xml" "${APPDIR}/usr/share/metainfo/com.theinfomoose.rFerram.metainfo.xml"
install_hicolor_icons "${APPDIR}/usr/share/icons/hicolor"
install -m 0644 "packaging/linux/rferram-server.service" "${APPDIR}/usr/lib/systemd/system/rferram-server.service"
install -m 0644 "README.md" "${APPDIR}/usr/share/doc/rferram/README.md"
install -m 0644 "LICENSE" "${APPDIR}/usr/share/doc/rferram/LICENSE"

APPIMAGE_FILE="${OUT_DIR}/rFerram-${VERSION}-${APPIMAGE_ARCH}.AppImage"
rm -f "${APPIMAGE_FILE}"

if command -v appimagetool >/dev/null 2>&1 && ARCH="${APPIMAGE_ARCH}" APPIMAGE_EXTRACT_AND_RUN=1 appimagetool --no-appstream "${APPDIR}" "${APPIMAGE_FILE}"; then
    chmod 0755 "${APPIMAGE_FILE}"
    echo "==> Created AppImage (via appimagetool): ${APPIMAGE_FILE}"
elif command -v cc >/dev/null 2>&1 || command -v gcc >/dev/null 2>&1; then
    CC_BIN="$(command -v gcc || command -v cc)"
    RUNTIME_SRC="${WORK_DIR}/appimage_runtime.c"
    RUNTIME_BIN="${WORK_DIR}/appimage_runtime"
    SQUASHFS_ROOT="${WORK_DIR}/squashfs-root"
    PAYLOAD_TAR="${WORK_DIR}/appdir_payload.tar.gz"

    cp -a "${APPDIR}" "${SQUASHFS_ROOT}"
    tar -C "${WORK_DIR}" -czf "${PAYLOAD_TAR}" "squashfs-root"

    cat > "${RUNTIME_SRC}" << 'CEOF'
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#define TRAILER_MAGIC "RFAI2\0\0\0"
#define TRAILER_SIZE 16

static int read_payload_bounds(const char *self_path, uint64_t *out_offset, uint64_t *out_size) {
    int fd = open(self_path, O_RDONLY);
    if (fd < 0) {
        return -1;
    }
    off_t end = lseek(fd, 0, SEEK_END);
    if (end < TRAILER_SIZE) {
        close(fd);
        return -1;
    }
    if (lseek(fd, end - TRAILER_SIZE, SEEK_SET) < 0) {
        close(fd);
        return -1;
    }
    unsigned char buf[TRAILER_SIZE];
    if (read(fd, buf, TRAILER_SIZE) != TRAILER_SIZE) {
        close(fd);
        return -1;
    }
    close(fd);
    if (memcmp(buf + 8, TRAILER_MAGIC, 8) != 0) {
        return -1;
    }
    uint64_t offset = 0;
    for (int i = 0; i < 8; i++) {
        offset |= ((uint64_t)buf[i]) << (i * 8);
    }
    if (offset == 0 || offset >= (uint64_t)(end - TRAILER_SIZE)) {
        return -1;
    }
    *out_offset = offset;
    *out_size = (uint64_t)(end - TRAILER_SIZE) - offset;
    return 0;
}

static int extract_payload_to_dir(const char *self_path, uint64_t offset, uint64_t size, const char *dest_dir) {
    int in_fd = open(self_path, O_RDONLY);
    if (in_fd < 0) {
        return -1;
    }
    if (lseek(in_fd, (off_t)offset, SEEK_SET) < 0) {
        close(in_fd);
        return -1;
    }

    int pipefd[2];
    if (pipe(pipefd) < 0) {
        close(in_fd);
        return -1;
    }

    pid_t pid = fork();
    if (pid < 0) {
        close(pipefd[0]);
        close(pipefd[1]);
        close(in_fd);
        return -1;
    }
    if (pid == 0) {
        close(pipefd[1]);
        dup2(pipefd[0], STDIN_FILENO);
        close(pipefd[0]);
        close(in_fd);
        execlp("tar", "tar", "-C", dest_dir, "-xzf", "-", (char *)NULL);
        _exit(127);
    }

    close(pipefd[0]);
    unsigned char chunk[65536];
    uint64_t remaining = size;
    int write_err = 0;
    while (remaining > 0) {
        size_t to_read = remaining < sizeof(chunk) ? (size_t)remaining : sizeof(chunk);
        ssize_t n = read(in_fd, chunk, to_read);
        if (n <= 0) {
            write_err = 1;
            break;
        }
        size_t written = 0;
        while (written < (size_t)n) {
            ssize_t w = write(pipefd[1], chunk + written, (size_t)n - written);
            if (w <= 0) {
                write_err = 1;
                break;
            }
            written += (size_t)w;
        }
        if (write_err) {
            break;
        }
        remaining -= (uint64_t)n;
    }
    close(pipefd[1]);
    close(in_fd);

    int status = 0;
    if (waitpid(pid, &status, 0) < 0 || write_err || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        return -1;
    }
    return 0;
}

static void remove_tree(const char *dir_path) {
    pid_t pid = fork();
    if (pid == 0) {
        execlp("rm", "rm", "-rf", dir_path, (char *)NULL);
        _exit(127);
    } else if (pid > 0) {
        int st = 0;
        waitpid(pid, &st, 0);
    }
}

int main(int argc, char **argv) {
    char self_path[PATH_MAX];
    ssize_t len = readlink("/proc/self/exe", self_path, sizeof(self_path) - 1);
    if (len < 0) {
        perror("readlink(/proc/self/exe)");
        return 1;
    }
    self_path[len] = '\0';

    uint64_t offset = 0;
    uint64_t size = 0;
    if (read_payload_bounds(self_path, &offset, &size) != 0) {
        fprintf(stderr, "ERROR: Invalid or corrupted rFerram AppImage payload header.\n");
        return 1;
    }

    int arg_shift = 0;
    if (argc > 1) {
        if (strcmp(argv[1], "--appimage-help") == 0) {
            printf("rFerram Type-2 Self-Extracting AppImage Runtime\n");
            printf("  --appimage-extract           Extract content to squashfs-root/\n");
            printf("  --appimage-extract-and-run   Extract content to temporary directory and execute AppRun\n");
            printf("  --appimage-offset            Print byte offset of embedded AppDir archive\n");
            printf("  --appimage-version           Print AppImage runtime version\n");
            printf("  --appimage-help              Print this help message\n");
            return 0;
        }
        if (strcmp(argv[1], "--appimage-version") == 0) {
            printf("rFerram AppImage Type-2 Runtime v2.0\n");
            return 0;
        }
        if (strcmp(argv[1], "--appimage-offset") == 0) {
            printf("%" PRIu64 "\n", offset);
            return 0;
        }
        if (strcmp(argv[1], "--appimage-extract") == 0) {
            if (extract_payload_to_dir(self_path, offset, size, ".") != 0) {
                fprintf(stderr, "ERROR: Failed to extract squashfs-root\n");
                return 1;
            }
            return 0;
        }
        if (strcmp(argv[1], "--appimage-extract-and-run") == 0) {
            arg_shift = 1;
        }
    }

    const char *tmp_base = getenv("TMPDIR");
    if (!tmp_base || tmp_base[0] == '\0') {
        tmp_base = "/tmp";
    }
    char work_dir[PATH_MAX];
    snprintf(work_dir, sizeof(work_dir), "%s/appimage_rferram_XXXXXX", tmp_base);
    if (!mkdtemp(work_dir)) {
        perror("mkdtemp");
        return 1;
    }

    if (extract_payload_to_dir(self_path, offset, size, work_dir) != 0) {
        fprintf(stderr, "ERROR: Failed to extract AppImage runtime payload\n");
        remove_tree(work_dir);
        return 1;
    }

    char apprun_path[PATH_MAX];
    snprintf(apprun_path, sizeof(apprun_path), "%s/squashfs-root/AppRun", work_dir);
    setenv("APPIMAGE", self_path, 1);
    setenv("ARGV0", argv[0], 1);

    int fwd_argc = argc - arg_shift;
    char **fwd_argv = calloc((size_t)(fwd_argc + 1), sizeof(char *));
    if (!fwd_argv) {
        remove_tree(work_dir);
        return 1;
    }
    fwd_argv[0] = apprun_path;
    for (int i = 1; i < fwd_argc; i++) {
        fwd_argv[i] = argv[i + arg_shift];
    }
    fwd_argv[fwd_argc] = NULL;

    pid_t child = fork();
    if (child == 0) {
        execv(apprun_path, fwd_argv);
        perror("execv(AppRun)");
        _exit(127);
    }

    int status = 0;
    int exit_code = 1;
    if (child > 0 && waitpid(child, &status, 0) > 0) {
        if (WIFEXITED(status)) {
            exit_code = WEXITSTATUS(status);
        } else if (WIFSIGNALED(status)) {
            exit_code = 128 + WTERMSIG(status);
        }
    }

    free(fwd_argv);
    remove_tree(work_dir);
    return exit_code;
}
CEOF

    "${CC_BIN}" -O2 -s "${RUNTIME_SRC}" -o "${RUNTIME_BIN}"

    python3 - "${RUNTIME_BIN}" "${PAYLOAD_TAR}" "${APPIMAGE_FILE}" << 'PYEOF'
import struct
import sys

runtime_path, payload_path, out_path = sys.argv[1], sys.argv[2], sys.argv[3]
with open(runtime_path, "rb") as f:
    runtime = bytearray(f.read())

# Stamp Type-2 AppImage ELF magic bytes ('A', 'I', 0x02) at e_ident[8..11]
runtime[8:11] = b"AI\x02"

with open(payload_path, "rb") as f:
    payload = f.read()

offset = len(runtime)
trailer = struct.pack("<Q8s", offset, b"RFAI2\x00\x00\x00")

with open(out_path, "wb") as f:
    f.write(runtime)
    f.write(payload)
    f.write(trailer)
PYEOF

    chmod 0755 "${APPIMAGE_FILE}"
    echo "==> Created AppImage (self-extracting Type-2 ELF bundle): ${APPIMAGE_FILE}"
else
    echo "ERROR: Neither appimagetool nor a C compiler (gcc/cc) is available to build .AppImage" >&2
    exit 1
fi
