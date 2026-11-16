#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

DIST_DIR="${DIST_DIR:-$ROOT/dist}"
OUT_DIR="${OUT_DIR:-$ROOT/output}"
DEB_TMPL="$ROOT/packaging/linux/deb"
APP_BIN="$ROOT/target/release/vidub_app"
MODELS_TOML="$ROOT/models.toml"
NOTICES="$ROOT/THIRD_PARTY_NOTICES.md"
VERSION="${VIDUB_VERSION:-$(grep -m1 '^version = ' "$ROOT/Cargo.toml" | sed 's/^version = "//; s/"$//')}"
[ -n "$VERSION" ] || { echo "could not determine version from Cargo.toml" >&2; exit 1; }

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64) DEB_ARCH="amd64" ;;
  arm64|aarch64) DEB_ARCH="arm64" ;;
  *) echo "unexpected Linux arch: $ARCH (expected x86_64/aarch64)" >&2; exit 1 ;;
esac

DO_TAR=1
DO_DEB=1
for arg in "$@"; do
  case "$arg" in
    --tarball-only) DO_DEB=0 ;;
    --deb-only) DO_TAR=0 ;;
    -h|--help) echo "usage: $0 [--tarball-only|--deb-only]" >&2; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

command -v patchelf >/dev/null 2>&1 || {
  echo "patchelf is required (sudo apt-get install patchelf)" >&2; exit 1;
}

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
require_file "$DIST_DIR/bin/yt-dlp" "fetch-sidecars.sh (linux yt-dlp) did not stage"
require_file "$DIST_DIR/bin/ffmpeg" "fetch-sidecars.sh (BtbN ffmpeg) did not stage"
require_file "$DIST_DIR/bin/ffprobe" "fetch-sidecars.sh (BtbN ffprobe) did not stage"
require_file "$DIST_DIR/bin/piper" "fetch-sidecars.sh (piper + espeak-ng-data) did not stage"
require_file "$DIST_DIR/bin/qjs" "fetch-sidecars.sh (qjs) did not stage"
require_file "$DIST_DIR/models/ngochuyennew.onnx" "fetch-sidecars.sh voice"
require_file "$DIST_DIR/models/config.json" "fetch-sidecars.sh voice config"
require_file "$DIST_DIR/versions.json" "fetch-sidecars.sh manifest"
require_file "$MODELS_TOML" "workspace models.toml"
require_file "$NOTICES" "repo-root THIRD_PARTY_NOTICES.md"
require_file "$DEB_TMPL/control" "packaging/linux/deb template"
require_file "$DEB_TMPL/postinst" "packaging/linux/deb template"
require_file "$DEB_TMPL/vidub.desktop" "packaging/linux/deb template"

STAGE="$(mktemp -d "${TMPDIR:-/tmp}/vidub-linux.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT
PAYLOAD="$STAGE/vidub"

mkdir -p "$PAYLOAD/bin" "$PAYLOAD/models"
install -m 0755 "$APP_BIN" "$PAYLOAD/vidub_app"
install -m 0644 "$MODELS_TOML" "$PAYLOAD/models.toml"
install -m 0644 "$DIST_DIR/versions.json" "$PAYLOAD/versions.json"
install -m 0644 "$NOTICES" "$PAYLOAD/THIRD_PARTY_NOTICES.md"
cp -R "$DIST_DIR/bin/." "$PAYLOAD/bin/"
if [ -d "$DIST_DIR/lib" ]; then
  for f in "$DIST_DIR/lib"/*.so*; do
    [ -e "$f" ] && cp "$f" "$PAYLOAD/bin/"
  done
else
  echo "warning: $DIST_DIR/lib missing (ffmpeg shared libs not staged); ffmpeg may fail to launch" >&2
fi
find "$PAYLOAD/bin" -type f -exec chmod u+x {} +
chmod +x "$PAYLOAD/vidub_app"

install -m 0644 "$DIST_DIR/models/ngochuyennew.onnx" "$PAYLOAD/models/ngochuyennew.onnx"
install -m 0644 "$DIST_DIR/models/config.json" "$PAYLOAD/models/config.json"

for b in ffmpeg ffprobe piper; do
  if [ -f "$PAYLOAD/bin/$b" ]; then
    patchelf --set-rpath '$ORIGIN' "$PAYLOAD/bin/$b" || {
      echo "warning: patchelf failed on $b (already has rpath?)" >&2;
    }
  fi
done

mkdir -p "$OUT_DIR"

TARBALL="$OUT_DIR/vidub-${VERSION}-linux-${DEB_ARCH}.tar.gz"
if [ "$DO_TAR" = "1" ]; then
  tar -czf "$TARBALL" -C "$STAGE" vidub
  echo "wrote $TARBALL"
fi

if [ "$DO_DEB" = "1" ]; then
  DROOT="$STAGE/debroot"
  mkdir -p "$DROOT/DEBIAN" "$DROOT/usr/lib" "$DROOT/usr/bin" \
    "$DROOT/usr/share/applications"
  cp -R "$PAYLOAD" "$DROOT/usr/lib/vidub"
  ln -s ../lib/vidub/vidub_app "$DROOT/usr/bin/vidub"
  sed -e "s/__VERSION__/$VERSION/g" -e "s/__ARCH__/$DEB_ARCH/g" \
    "$DEB_TMPL/control" > "$DROOT/DEBIAN/control"
  chmod 0644 "$DROOT/DEBIAN/control"
  install -m 0755 "$DEB_TMPL/postinst" "$DROOT/DEBIAN/postinst"
  install -m 0644 "$DEB_TMPL/vidub.desktop" "$DROOT/usr/share/applications/vidub.desktop"
  DEB_OUT="$OUT_DIR/vidub_${VERSION}_${DEB_ARCH}.deb"
  dpkg-deb --build --root-owner-group "$DROOT" "$DEB_OUT"
  echo "wrote $DEB_OUT"
fi

echo "tarball root: vidub/{vidub_app,bin,models,models.toml,versions.json,THIRD_PARTY_NOTICES.md}"
