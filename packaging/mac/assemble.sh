#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

DIST_DIR="${DIST_DIR:-$ROOT/dist}"
OUT_DIR="${OUT_DIR:-$ROOT/output}"
APP_BIN="$ROOT/target/release/vidub_app"
MODELS_TOML="$ROOT/models.toml"
NOTICES="$ROOT/THIRD_PARTY_NOTICES.md"
ARCH="$(uname -m)"
case "$ARCH" in
  arm64|x86_64) ;;
  *) echo "unexpected macOS arch: $ARCH (expected arm64|x86_64)" >&2; exit 1 ;;
esac
VERSION="${VIDUB_VERSION:-$(grep -m1 '^version = ' "$ROOT/Cargo.toml" | sed 's/^version = "//; s/"$//')}"
[ -n "$VERSION" ] || { echo "could not determine version from Cargo.toml" >&2; exit 1; }
OUT_ZIP="$OUT_DIR/ViDub-${VERSION}-macos-${ARCH}.zip"

STAGE="$(mktemp -d "${TMPDIR:-/tmp}/vidub-macos.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT
APP_DIR="$STAGE/ViDub.app"
MACOS_DIR="$APP_DIR/Contents/MacOS"
RES_DIR="$APP_DIR/Contents/Resources"
RES_BIN="$RES_DIR/bin"
RES_MODELS="$RES_DIR/models"

require_file() {
  if [ ! -f "$1" ]; then
    echo "missing input: $1" >&2
    echo "  $*" >&2
    exit 1
  fi
}
require_dir() {
  if [ ! -d "$1" ]; then
    echo "missing input dir: $1" >&2
    echo "  $*" >&2
    exit 1
  fi
}

require_file "$APP_BIN" "build first: cargo build --release -p vidub_app"
require_dir "$DIST_DIR/bin" "run scripts/fetch-sidecars.sh first"
require_dir "$DIST_DIR/models" "run scripts/fetch-sidecars.sh first"
require_file "$DIST_DIR/bin/yt-dlp" "fetch-sidecars.sh (macOS yt-dlp) did not stage"
require_file "$DIST_DIR/bin/piper" "fetch-sidecars.sh (macOS piper) did not stage"
require_file "$DIST_DIR/bin/mpv" "fetch-sidecars.sh (macOS mpv.app extraction) did not stage"
require_file "$DIST_DIR/bin/qjs" "fetch-sidecars.sh (macOS QuickJS for YouTube) did not stage"
require_file "$DIST_DIR/models/ngochuyennew.onnx" "fetch-sidecars.sh voice"
require_file "$DIST_DIR/models/config.json" "fetch-sidecars.sh voice config"
require_file "$DIST_DIR/versions.json" "fetch-sidecars.sh manifest"
require_file "$MODELS_TOML" "workspace models.toml"
require_file "$NOTICES" "repo-root THIRD_PARTY_NOTICES.md"

mkdir -p "$MACOS_DIR" "$RES_BIN" "$RES_MODELS"

install -m 0755 "$APP_BIN" "$MACOS_DIR/vidub_app"
install -m 0644 "$MODELS_TOML" "$MACOS_DIR/models.toml"
install -m 0755 "$DIST_DIR/bin/mpv" "$MACOS_DIR/mpv"

cp -R "$DIST_DIR/bin/." "$RES_BIN/"

repair_macho_paths() {
  local bin="$1" libdir="$2" dep
  [ -f "$bin" ] && [ -d "$libdir" ] || return 0
  command -v install_name_tool >/dev/null 2>&1 || {
    echo "install_name_tool missing (Xcode CLT required)" >&2; return 1; }
  for dep in libespeak-ng.1.dylib libpiper_phonemize.1.dylib \
             libonnxruntime.1.14.1.dylib; do
    [ -f "$libdir/$dep" ] || continue
    install_name_tool -change "@rpath/$dep" "@executable_path/lib/$dep" "$bin" \
      2>/dev/null || true
  done
  install_name_tool -add_rpath "@executable_path/lib" "$bin" 2>/dev/null || true
  return 0
}

if [ -d "$RES_BIN/lib" ]; then
  repair_macho_paths "$RES_BIN/piper" "$RES_BIN/lib" || true
  repair_macho_paths "$RES_BIN/piper_phonemize" "$RES_BIN/lib" || true
fi

if [ -f "$RES_BIN/piper" ] && command -v otool >/dev/null 2>&1; then
  if otool -L "$RES_BIN/piper" | grep -q "@executable_path/lib/libespeak-ng"; then
    echo "macho: piper deps rewritten to @executable_path/lib"
  else
    echo "macho: piper still references @rpath (libespeak-ng) — refusing to bundle" >&2
    exit 1
  fi
fi

chmod +x "$MACOS_DIR/vidub_app" "$MACOS_DIR/mpv" "$RES_BIN"/* 2>/dev/null || true
find "$RES_BIN" -type f -exec chmod u+x {} +

install -m 0644 "$DIST_DIR/models/ngochuyennew.onnx" "$RES_MODELS/ngochuyennew.onnx"
install -m 0644 "$DIST_DIR/models/config.json" "$RES_MODELS/config.json"

install -m 0644 "$DIST_DIR/versions.json" "$RES_DIR/versions.json"
install -m 0644 "$NOTICES" "$RES_DIR/THIRD_PARTY_NOTICES.md"

cat > "$APP_DIR/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>ViDub</string>
  <key>CFBundleDisplayName</key>
  <string>ViDub</string>
  <key>CFBundleIdentifier</key>
  <string>com.vidub.app</string>
  <key>CFBundleVersion</key>
  <string>$VERSION</string>
  <key>CFBundleShortVersionString</key>
  <string>$VERSION</string>
  <key>CFBundleExecutable</key>
  <string>vidub_app</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleSignature</key>
  <string>????</string>
  <key>CFBundleSupportedPlatforms</key>
  <array><string>MacOSX</string></array>
  <key>LSMinimumSystemVersion</key>
  <string>15.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSPrincipalClass</key>
  <string>NSApplication</string>
</dict>
</plist>
EOF

cat > "$STAGE/README-MACOS.txt" <<EOF
ViDub for macOS $VERSION ($ARCH)
====================================

INSTALL
1. Unzip this archive and drag ViDub.app into /Applications.
2. Minimum macOS 15 (arm64 and Intel builds ship separately).

FIRST LAUNCH (unsigned build)
-----------------------------
This build is unsigned. Gatekeeper will block a plain double-click.
Launch once by right-clicking ViDub.app and choosing Open, then Open again,
or run in Terminal:

  xattr -d com.apple.quarantine /Applications/ViDub.app

PREREQUISITE — ffmpeg via Homebrew
----------------------------------
ffmpeg is not bundled on macOS. Install it before first use:

  brew install ffmpeg

The app resolves Homebrew ffmpeg at /opt/homebrew/bin (Apple Silicon) or
/usr/local/bin (Intel), falling back to your PATH.

API KEY
-------
Choose NVIDIA, OpenRouter, or Google Gemini in Settings (Cài đặt) and paste
your API key. Environment variables NVAPI_KEY, OPENROUTER_API_KEY, or
GEMINI_API_KEY also prefill the corresponding provider when no key is saved.

Third-party licenses: see ViDub.app/Contents/Resources/THIRD_PARTY_NOTICES.md.
EOF

command -v xattr >/dev/null 2>&1 && xattr -dr com.apple.quarantine "$APP_DIR" 2>/dev/null || true

if command -v plutil >/dev/null 2>&1; then
  plutil -lint "$APP_DIR/Contents/Info.plist"
else
  echo "note: plutil not found; skipping Info.plist lint"
fi

mkdir -p "$OUT_DIR"
rm -f "$OUT_ZIP"
ditto -c -k --sequesterRsrc "$STAGE" "$OUT_ZIP"
echo "wrote $OUT_ZIP"
echo "bundle root: ViDub.app/Contents/{MacOS/vidub_app, MacOS/mpv, MacOS/models.toml, Resources/{bin,models}}"
