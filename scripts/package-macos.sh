#!/usr/bin/env bash
# Package a self-contained DragonSlayer.app as dragonslayer-<version>-macos-arm64.zip
#   scripts/package-macos.sh            # build target/dist/DragonSlayer.app + zip
#   scripts/package-macos.sh --install  # ...and copy the app to ~/Applications with a Desktop alias
#   ICON=my-icon.png scripts/package-macos.sh --install   # use your own 1024x1024 PNG as the app icon
#
# Build machine needs Homebrew's libgphoto2 + pkg-config (see MAC-DEV-SETUP.md). The finished
# app does not: libgphoto2, its camera drivers and a static ffmpeg all ship inside the bundle.
# Apple Silicon only (the Homebrew libraries it bundles are arm64).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/version = "(.*)"/\1/')
BREW="$(brew --prefix)"
GP="$(brew --prefix libgphoto2)"
DIST="target/dist"
APP="$DIST/DragonSlayer.app"
ZIP="$DIST/dragonslayer-$VERSION-macos-arm64.zip"

# Static arm64 ffmpeg (GPL, signed by its builder). Pinned by checksum; bump both together.
FFMPEG_URL="https://ffmpeg.martin-riedl.de/download/macos/arm64/1789931890_9.0.2/ffmpeg.zip"
FFMPEG_SHA256="c8ed4c4e6978a03c485edbfe4e0a5dc2380f8a30bba5150531b31b094492d924"
FFMPEG_CACHE="target/ffmpeg-cache"

echo "==> Building release with gphoto2 feature..."
export PKG_CONFIG_PATH="$BREW/lib/pkgconfig"
cargo build --release --features gphoto2 -p dragonslayer-cli -p dragonslayer-app

echo "==> Assembling $APP..."
rm -rf "$APP" "$ZIP"
C="$APP/Contents"
mkdir -p "$C/MacOS" "$C/Frameworks" "$C/Resources/bin" \
         "$C/Resources/libgphoto2/camlibs" "$C/Resources/libgphoto2/iolibs"
cp target/release/dragonslayer-app "$C/MacOS/"
# CLI gets a distinct name: APFS is case-insensitive, so "dragonslayer" would clash with the "DragonSlayer" launcher
cp target/release/dragonslayer "$C/MacOS/dragonslayer-cli"
cp "$GP"/lib/libgphoto2/*/*.so "$C/Resources/libgphoto2/camlibs/"
cp "$GP"/lib/libgphoto2_port/*/*.so "$C/Resources/libgphoto2/iolibs/"

# --- Bundle Homebrew dylibs and rewrite every reference to @rpath ---------------------------
# Walks each Mach-O's dependencies; anything outside /System and /usr/lib is copied into
# Frameworks/ and relinked, recursively, so nothing points back at Homebrew.
brew_deps() { otool -L "$1" | tail -n +2 | awk '{print $1}' | grep -E "^($BREW|/usr/local)/" || true; }

relink() {  # relink <macho> <rpath-to-Frameworks>
  local f="$1" rp="$2" dep base
  chmod u+w "$f"
  for dep in $(brew_deps "$f"); do
    base="$(basename "$dep")"
    if [[ ! -e "$C/Frameworks/$base" ]]; then
      cp -L "$dep" "$C/Frameworks/$base"
      chmod u+w "$C/Frameworks/$base"
      install_name_tool -id "@rpath/$base" "$C/Frameworks/$base" 2>/dev/null
      relink "$C/Frameworks/$base" "@loader_path"
    fi
    install_name_tool -change "$dep" "@rpath/$base" "$f" 2>/dev/null
  done
  install_name_tool -add_rpath "$rp" "$f" 2>/dev/null || true
}

for f in "$C"/MacOS/dragonslayer-app "$C"/MacOS/dragonslayer-cli; do relink "$f" "@executable_path/../Frameworks"; done
for f in "$C"/Resources/libgphoto2/*/*.so; do relink "$f" "@loader_path/../../../Frameworks"; done

LEFTOVER=$(find "$C" -type f \( -name '*.dylib' -o -name '*.so' -o -perm -u+x \) -exec sh -c \
  'otool -L "$1" 2>/dev/null | tail -n +2 | grep -E "(/opt/homebrew|/usr/local)/" | sed "s|^|$1: |"' _ {} \;)
if [[ -n "$LEFTOVER" ]]; then echo "ERROR: still linked to Homebrew:"; echo "$LEFTOVER"; exit 1; fi

# --- ffmpeg -------------------------------------------------------------------------------
mkdir -p "$FFMPEG_CACHE"
if [[ ! -f "$FFMPEG_CACHE/ffmpeg.zip" ]] || ! echo "$FFMPEG_SHA256  $FFMPEG_CACHE/ffmpeg.zip" | shasum -a 256 -c -s; then
  echo "==> Downloading ffmpeg..."
  curl -fsSL -o "$FFMPEG_CACHE/ffmpeg.zip" "$FFMPEG_URL"
fi
echo "$FFMPEG_SHA256  $FFMPEG_CACHE/ffmpeg.zip" | shasum -a 256 -c -s \
  || { echo "ERROR: ffmpeg checksum mismatch"; exit 1; }
unzip -o -q "$FFMPEG_CACHE/ffmpeg.zip" ffmpeg -d "$C/Resources/bin"

# --- Launcher -----------------------------------------------------------------------------
# Finder/Dock launches get a bare PATH, so point at the bundled ffmpeg and camera drivers here.
# Also frees the camera from macOS's PTP daemon, which otherwise grabs it on plug-in.
cat > "$C/MacOS/DragonSlayer" <<'SH'
#!/bin/zsh
HERE="${0:A:h}"
RES="$HERE/../Resources"
export PATH="$RES/bin:$PATH"
export CAMLIBS="$RES/libgphoto2/camlibs"
export IOLIBS="$RES/libgphoto2/iolibs"
killall ptpcamerad 2>/dev/null
cd "$HOME"
exec "$HERE/dragonslayer-app" "$@"
SH
chmod +x "$C/MacOS/DragonSlayer"

# --- Icon: rasterise docs/icon.svg (the pixel icon the window uses), or ICON=some.png ------
TMP="$(mktemp -d)"
if [[ -n "${ICON:-}" ]]; then cp "$ICON" "$TMP/icon.svg.png"; else qlmanage -t -s 1024 -o "$TMP" docs/icon.svg >/dev/null 2>&1; fi
ICONSET="$TMP/AppIcon.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s "$TMP/icon.svg.png" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s*2)) $((s*2)) "$TMP/icon.svg.png" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$C/Resources/AppIcon.icns"
rm -rf "$TMP"

cat > "$C/Info.plist" <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>DragonSlayer</string>
  <key>CFBundleDisplayName</key><string>DragonSlayer</string>
  <key>CFBundleIdentifier</key><string>com.themagnificentronnie.dragonslayer</string>
  <key>CFBundleExecutable</key><string>DragonSlayer</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSArchitecturePriority</key><array><string>arm64</string></array>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PL

# Licences: DragonSlayer is MIT; bundled libgphoto2 is LGPL-2.1+, ffmpeg is GPL-3.0.
cp LICENSE "$C/Resources/LICENSE-DragonSlayer.txt"
cat > "$C/Resources/THIRD-PARTY-NOTICES.txt" <<TXT
DragonSlayer $VERSION for macOS bundles the following third-party software, unmodified:

libgphoto2 $(pkg-config --modversion libgphoto2) and its dependencies (Contents/Frameworks, Contents/Resources/libgphoto2)
  Licence: LGPL-2.1-or-later. Source: https://github.com/gphoto/libgphoto2
  Dynamically linked; you may replace these libraries with your own builds.

FFmpeg 9.0.2 static build by Martin Riedl (Contents/Resources/bin/ffmpeg)
  Licence: GPL-3.0-or-later (built with --enable-gpl --enable-version3).
  Source: https://ffmpeg.org/releases/  Build scripts: https://git.martin-riedl.de/ffmpeg/build-script
  Run as a separate program; not linked into DragonSlayer.
TXT

# --- Sign ---------------------------------------------------------------------------------
# Ad-hoc signatures (Apple Silicon refuses unsigned code). ffmpeg keeps its builder's signature.
find "$C/Frameworks" "$C/Resources/libgphoto2" -type f -exec codesign --force --sign - {} \;
codesign --force --sign - "$C/MacOS/dragonslayer-app" "$C/MacOS/dragonslayer-cli"
codesign --force --sign - "$APP"
codesign --verify --strict "$APP"

echo "==> Zipping $ZIP..."
cp README.md "$DIST/README.md"
ditto -c -k --keepParent "$APP" "$ZIP"
echo "    $(du -h "$ZIP" | cut -f1)  $(shasum -a 256 "$ZIP" | cut -d' ' -f1)"

if [[ "${1:-}" == "--install" ]]; then
  echo "==> Installing to ~/Applications..."
  mkdir -p ~/Applications
  rm -rf ~/Applications/DragonSlayer.app
  ditto "$APP" ~/Applications/DragonSlayer.app
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f ~/Applications/DragonSlayer.app
  rm -f ~/Desktop/DragonSlayer
  osascript -e "tell application \"Finder\" to make alias file to (POSIX file \"$HOME/Applications/DragonSlayer.app\") at (path to desktop folder) with properties {name:\"DragonSlayer\"}" >/dev/null
  echo "==> Installed: ~/Applications/DragonSlayer.app, alias on your Desktop"
fi
echo "==> Done."
