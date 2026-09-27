#!/usr/bin/env bash
# Package a distributable Windows folder as dragonslayer-<version>-windows-x64.zip
# Run from an MSYS2 UCRT64 shell (needs libgphoto2, pkg-config, clang for the build).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Read version from workspace Cargo.toml
VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/version = "(.*)"/\1/')
STAGE="target/dist/dragonslayer-$VERSION-windows-x64"
ZIP="target/dist/dragonslayer-$VERSION-windows-x64.zip"

# Inside an MSYS2 UCRT64 shell (CI) MINGW_PREFIX is /ucrt64; from Git Bash, assume the default install.
UCRT="${MINGW_PREFIX:-/c/msys64/ucrt64}"
export PATH="$UCRT/bin:$PATH"
export PKG_CONFIG_PATH="$UCRT/lib/pkgconfig"
export PKG_CONFIG_ALLOW_CROSS=1
export LIBCLANG_PATH="$UCRT/bin"
REL="target/x86_64-pc-windows-gnu/release"

if [[ -z "${SKIP_BUILD:-}" ]]; then
    echo "==> Building release with gphoto2 feature..."
    cargo build --release --features gphoto2 \
        -p dragonslayer-cli -p dragonslayer-app \
        --target x86_64-pc-windows-gnu
fi

echo "==> Staging to $STAGE..."
rm -rf "$STAGE" "$ZIP"
mkdir -p "$STAGE"/{camera-drivers,libgphoto2/camlibs,libgphoto2/iolibs}

cp "$REL/dragonslayer.exe" "$REL/dragonslayer-app.exe" "$STAGE/"
cp "$REL/zadig.exe" "$STAGE/camera-drivers/" 2>/dev/null || echo "   (no zadig.exe in $REL; bundle ships without it)"

# libgphoto2 camera drivers (camlibs) and port drivers (iolibs), found via CAMLIBS/IOLIBS at
# runtime. Globbed so a newer libgphoto2 from pacman doesn't break the script.
cp "$UCRT"/lib/libgphoto2/*/*.dll "$STAGE/libgphoto2/camlibs/"
cp "$UCRT"/lib/libgphoto2_port/*/*.dll "$STAGE/libgphoto2/iolibs/"

# Runtime DLLs: walk the import tables of everything we ship and copy each MSYS2 DLL they need,
# recursively. Windows system DLLs aren't in $UCRT/bin, so they're skipped.
echo "==> Collecting runtime DLLs..."
declare -A SEEN
collect() {
    local dll
    for dll in $(objdump -p "$1" | awk '/DLL Name:/ { print $3 }'); do
        local key="${dll,,}"
        [[ -n "${SEEN[$key]:-}" ]] && continue
        SEEN[$key]=1
        if [[ -f "$UCRT/bin/$dll" ]]; then
            cp "$UCRT/bin/$dll" "$STAGE/"
            collect "$UCRT/bin/$dll"
        fi
    done
}
for f in "$STAGE"/*.exe "$STAGE"/libgphoto2/camlibs/*.dll "$STAGE"/libgphoto2/iolibs/*.dll; do
    collect "$f"
done
echo "   $(ls "$STAGE"/*.dll | wc -l) DLLs bundled"

# Docs
cp README.md CAMERAS.md MANUAL_TESTING.md dragonslayer-spec.md "$STAGE/"

# Portable launcher: sets CAMLIBS/IOLIBS to the bundled folders so it runs on
# machines without MSYS2.
cat > "$STAGE/DragonSlayer.cmd" <<'CMD'
@echo off
taskkill /IM dragonslayer-app.exe /F >nul 2>&1
set "CAMLIBS=%~dp0libgphoto2\camlibs"
set "IOLIBS=%~dp0libgphoto2\iolibs"
start "" "%~dp0dragonslayer-app.exe" %*
CMD

cat > "$STAGE/DragonSlayer-CLI.cmd" <<'CMD'
@echo off
set "CAMLIBS=%~dp0libgphoto2\camlibs"
set "IOLIBS=%~dp0libgphoto2\iolibs"
"%~dp0dragonslayer.exe" %*
CMD

cat > "$STAGE/README-BUNDLE.txt" <<TXT
DragonSlayer $VERSION — Windows x64

To run:    double-click DragonSlayer.cmd
CLI:       DragonSlayer-CLI.cmd cameras   (etc.)
Drivers:   camera-drivers/zadig.exe       (one-time WinUSB swap; see in-app Help)

Full docs in README.md.
TXT

echo "==> Zipping..."
( cd target/dist && powershell.exe -NoProfile -Command "Compress-Archive -Path 'dragonslayer-$VERSION-windows-x64' -DestinationPath 'dragonslayer-$VERSION-windows-x64.zip' -Force" )

echo
echo "Bundle:  $ZIP"
du -h "$ZIP"
