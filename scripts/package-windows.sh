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

echo "==> Building release with gphoto2 feature..."
export PATH="/c/msys64/ucrt64/bin:$PATH"
export PKG_CONFIG_PATH="/c/msys64/ucrt64/lib/pkgconfig"
export PKG_CONFIG_ALLOW_CROSS=1
export LIBCLANG_PATH="/c/msys64/ucrt64/bin"
cargo build --release --features gphoto2 \
    -p dragonslayer-cli -p dragonslayer-app \
    --target x86_64-pc-windows-gnu

echo "==> Staging to $STAGE..."
rm -rf "$STAGE" "$ZIP"
mkdir -p "$STAGE"/{bin,camera-drivers}

# Executables + bundled runtime DLLs already sitting in target/release/
cp target/x86_64-pc-windows-gnu/release/dragonslayer.exe "$STAGE/"
cp target/x86_64-pc-windows-gnu/release/dragonslayer-app.exe "$STAGE/"
cp target/x86_64-pc-windows-gnu/release/*.dll "$STAGE/" 2>/dev/null || true
cp target/x86_64-pc-windows-gnu/release/zadig.exe "$STAGE/camera-drivers/" 2>/dev/null || true

# libgphoto2 camlibs/iolibs (needed at runtime, referenced via CAMLIBS/IOLIBS env vars)
mkdir -p "$STAGE/libgphoto2/camlibs" "$STAGE/libgphoto2/iolibs"
cp /c/msys64/ucrt64/lib/libgphoto2/2.5.34/*.dll "$STAGE/libgphoto2/camlibs/"
cp /c/msys64/ucrt64/lib/libgphoto2_port/0.12.2/*.dll "$STAGE/libgphoto2/iolibs/"

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
