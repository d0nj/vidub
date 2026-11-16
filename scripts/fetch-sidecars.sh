#!/usr/bin/env bash
set -euo pipefail

DRY_RUN="${DRY_RUN:-0}"
for arg in "$@"; do
  case "$arg" in
    --dry-run|-d) DRY_RUN=1 ;;
    --help|-h)
      echo "Usage: $0 [--dry-run]"; echo "  DIST_DIR=<dir> overrides ./dist"; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS-$ARCH" in
  Linux-x86_64)  PLATFORM="linux-x86_64" ;;
  Darwin-arm64)  PLATFORM="darwin-arm64" ;;
  Darwin-x86_64) PLATFORM="darwin-x86_64" ;;
  *) echo "unsupported platform: $OS/$ARCH (Windows: use fetch-sidecars.ps1)" >&2; exit 1 ;;
esac

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${DIST_DIR:-$ROOT/dist}"
BIN_DIR="$DIST_DIR/bin"
MODELS_DIR="$DIST_DIR/models"
STATE_DIR="$DIST_DIR/.fetch-state"
CACHE_DIR="${TMPDIR:-/tmp}/vidub-fetch"
EXT_ROOT="$CACHE_DIR/extracted"

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}

log() { echo "[DRY-RUN] $*"; }

ensure_dir() {
  [ -n "$1" ] || { echo "ensure_dir: empty path (internal error)" >&2; return 1; }
  if [ "$DRY_RUN" = "1" ]; then log "would mkdir -p $1"; else mkdir -p "$1"; fi
}

write_marker() {
  local marker="$STATE_DIR/$(basename "$1").sha256"
  if [ "$DRY_RUN" = "1" ]; then log "would write marker $marker"; else
    ensure_dir "$STATE_DIR"
    local tmp="$marker.tmp.$$"
    sha256_of "$1" > "$tmp"
    mv "$tmp" "$marker"
  fi
}

ARTIFACTS=()
add() { ARTIFACTS+=("$*"); }

if [ "$PLATFORM" = "linux-x86_64" ]; then
  add "yt-dlp|2026.10.26|file|767ba9dce7052aa196f8b12652f433d76de128ba168966fa84b6394d5e6c6795|https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/yt-dlp_linux|bin/yt-dlp|-"
  add "ffmpeg|N-128728-g49a3b1f259|ffmpeg-linux|39055baa287af2a3acea041ccd7cca295d2f374dda7210fd2503f637fa238c72|https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-10-12-39/ffmpeg-N-128728-g49a3b1f259-linux64-lgpl-shared.tar.xz|bin/ffmpeg|ffmpeg"
  add "ffprobe|N-128728-g49a3b1f259|ffmpeg-linux|39055baa287af2a3acea041ccd7cca295d2f374dda7210fd2503f637fa238c72|https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-10-12-39/ffmpeg-N-128728-g49a3b1f259-linux64-lgpl-shared.tar.xz|bin/ffprobe|ffprobe"
  add "piper|2023.11.14-2|piper-tgz|a50cb45f355b7af1f6d758c1b360717877ba0a398cc8cbe6d2a7a3a26e225992|https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_linux_x86_64.tar.gz|bin/piper|piper"
  add "qjs|v0.18.0|file|85ded23b9fabe8c1644fc40a8d2bbdd71157affcbd807359c618cc717ee24d7d|https://github.com/quickjs-ng/quickjs/releases/download/v0.18.0/qjs-linux-x86_64|bin/qjs|-"
elif [ "$PLATFORM" = "darwin-arm64" ]; then
  add "yt-dlp|2026.10.26|file|0f922c15ccc0b83eefa1c8bc9104e434318c4bf16e408e1ee93b7745a6efe0aa|https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/yt-dlp_macos|bin/yt-dlp|-"
  add "mpv|v0.42.0|mpv-app|be1b8d2445e6880860869c080b8710dee189721f1fa4689840db6a2c10d7be69|https://github.com/mpv-player/mpv/releases/download/v0.42.0/mpv-v0.42.0-macos-15-arm.zip|bin/mpv|mpv"
  add "piper|2023.11.14-2|piper-tgz|6b1eb03b3735946cb35216e063e7eebcc33a6bbf5dd96ec0217959bf1cdcb0cc|https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_macos_aarch64.tar.gz|bin/piper|piper"
  add "piper-dylibs|2023.11.14-4|piper-dylibs|78a9c28b3c94baf6e9526b2e386ce547909abaec4f31aadd7e16b01fbfe5f322|https://github.com/rhasspy/piper-phonemize/releases/download/2023.11.14-4/piper-phonemize_macos_aarch64.tar.gz|bin/lib|lib"
  add "qjs|v0.18.0|file|91fe89916d3b0a6c01c4dc8382a99c3c1fd91ecfc214aa81ca051902bcce590d|https://github.com/quickjs-ng/quickjs/releases/download/v0.18.0/qjs-darwin-arm64|bin/qjs|-"
else
  add "yt-dlp|2026.10.26|file|0f922c15ccc0b83eefa1c8bc9104e434318c4bf16e408e1ee93b7745a6efe0aa|https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/yt-dlp_macos|bin/yt-dlp|-"
  add "mpv|v0.42.0|mpv-app|2d2f34350ff844b6014c35f21ca142937275e19c0c673150a8564fcd2f3bfa14|https://github.com/mpv-player/mpv/releases/download/v0.42.0/mpv-v0.42.0-macos-15-intel.zip|bin/mpv|mpv"
  add "piper|2023.11.14-2|piper-tgz|ced85c0a3df13945b1e623b878a48fdc2854d5c485b4b67f62857cf551deaf8b|https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_macos_x64.tar.gz|bin/piper|piper"
  add "piper-dylibs|2023.11.14-4|piper-dylibs|9ec6e300c0d012a663758bc45a097b47ee759761a3b91c7742de042af789d84b|https://github.com/rhasspy/piper-phonemize/releases/download/2023.11.14-4/piper-phonemize_macos_x64.tar.gz|bin/lib|lib"
  add "qjs|v0.18.0|file|fa2c5dc78095daff566b32fab995ced12927875253b84216b91b834af2ef2b74|https://github.com/quickjs-ng/quickjs/releases/download/v0.18.0/qjs-darwin-x86_64|bin/qjs|-"
fi
BUCKET_BASE="https://huggingface.co/buckets/renzynx/nghitts-copy-bucket/resolve/piper-tts"
add "voice-onnx|ngochuyennew|file|88bc21477831dd99759cff164f0ab270e258faf435d01741f0211ff8620255e2|$BUCKET_BASE/ngochuyennew.onnx?download=true|models/ngochuyennew.onnx|-"
add "voice-config|nghitts-shared|file|6ce7b1bac4cf40410cd3c75fb100475306877850f5697f3d4daeeb3225410271|$BUCKET_BASE/config.json?download=true|models/config.json|-"

fetch_archive() {
  local url="$1" pin="$2"
  local leaf archive ext
  leaf="$(basename "${url%%\?*}")"
  archive="$CACHE_DIR/$leaf"
  ext="$EXT_ROOT/${leaf%.*}"
  if [ -f "$archive" ] && [ "$(sha256_of "$archive")" = "$pin" ]; then
    echo "  cache: $leaf (SHA-256 matches pin, no download)" >&2
    printf '%s\n%s\n' "$archive" "$ext"
    return 0
  fi
  if [ "$DRY_RUN" = "1" ]; then
    echo "[DRY-RUN] would download $leaf -> $archive" >&2
    printf '%s\n%s\n' "$archive" "$ext"
    return 0
  fi
  ensure_dir "$CACHE_DIR"
  curl -fsSL --retry 3 --retry-delay 2 -C - -o "$archive" "$url"
  local actual
  actual="$(sha256_of "$archive")"
  if [ "$actual" != "$pin" ]; then
    rm -f "$archive"
    echo "SHA-256 mismatch for $leaf:" >&2
    echo "  expected $pin" >&2
    echo "  actual   $actual" >&2
    echo "Refusing to stage. Delete the cache and retry; if upstream changed, bump the pin in both fetch scripts." >&2
    return 1
  fi
  echo "  verified: $leaf (SHA-256 matches pin)" >&2
  printf '%s\n%s\n' "$archive" "$ext"
}

extract_archive() {
  local archive="$1" extdir="$2"
  [ -n "$archive" ] && [ -n "$extdir" ] || { echo "extract_archive: empty argument (internal error)" >&2; return 1; }
  if [ -d "$extdir" ]; then return 0; fi
  ensure_dir "$extdir"
  case "$archive" in
    *.tar.gz)  tar -xzf "$archive" -C "$extdir" ;;
    *.tar.xz)
      command -v xz >/dev/null 2>&1 || { echo "xz is required to extract $archive (apt install xz-utils)" >&2; return 1; }
      tar -xf "$archive" -C "$extdir" ;;
    *.zip)
      command -v unzip >/dev/null 2>&1 || { echo "unzip is required to extract $archive" >&2; return 1; }
      unzip -q "$archive" -d "$extdir" ;;
    *) echo "unsupported archive: $archive" >&2; return 1 ;;
  esac
}

find_one() { find "$1" -name "$2" -type f 2>/dev/null | head -n 1 || true; }

json_quote() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }

main() {
  echo "ViDub fetch-sidecars — platform: $PLATFORM"
  [ "$DRY_RUN" = "1" ] && echo "DRY RUN: no network, no writes"
  ensure_dir "$BIN_DIR"; ensure_dir "$MODELS_DIR"; ensure_dir "$STATE_DIR"
  [ "$PLATFORM" = "linux-x86_64" ] && echo "note: mpv is NOT bundled on Linux (no upstream bins) — install system mpv: sudo apt install mpv"
  case "$PLATFORM" in darwin-*) echo "note: ffmpeg via system/Homebrew (brew install ffmpeg) — not bundled on macOS";; esac

  local manifest_entries=()
  local entry name version kind pin url dest exe
  for entry in "${ARTIFACTS[@]}"; do
    IFS='|' read -r name version kind pin url dest exe <<< "$entry"
    echo "== $name ($version)"
    local staged="$DIST_DIR/$dest"
    local marker="$STATE_DIR/$(basename "$dest").sha256"
    local staged_hash="" up_to_date=0
    if [ -d "$staged" ]; then
      if [ -f "$marker" ] && [ "$(cat "$marker")" = "$pin" ]; then up_to_date=1; fi
    elif [ -f "$staged" ]; then
      staged_hash="$(sha256_of "$staged")"
      [ "$staged_hash" = "$pin" ] && up_to_date=1
      if [ "$up_to_date" = "0" ] && [ -f "$marker" ] && [ "$(cat "$marker")" = "$staged_hash" ]; then up_to_date=1; fi
    fi
    if [ "$up_to_date" = "1" ]; then
      echo "  up to date (skip, no download)"
    elif [ "$DRY_RUN" = "1" ]; then
      log "would download $url (sha256=$pin) -> $staged"
      continue
    else
      local archive ext
      local fetched
      fetched="$(fetch_archive "$url" "$pin")"
      archive="$(printf "%s\n" "$fetched" | sed -n 1p)"
      ext="$(printf "%s\n" "$fetched" | sed -n 2p)"
      case "$kind" in
        file)
          ensure_dir "$(dirname "$staged")"
          cp "$archive" "$staged" ;;
        piper-dylibs)
          extract_archive "$archive" "$ext"
          local libsrc
          libsrc="$(find "$ext" -type d -name lib 2>/dev/null | head -n 1 || true)"
          [ -n "$libsrc" ] || { echo "staged archive for $name has no lib/ directory" >&2; return 1; }
          ensure_dir "$BIN_DIR/lib"
          cp -R "$libsrc/." "$BIN_DIR/lib/"
          cp -R "$libsrc/." "$BIN_DIR/" 2>/dev/null || true ;;
        piper-tgz)
          extract_archive "$archive" "$ext"
          local piper_bin parent
          piper_bin="$(find_one "$ext" "$exe")"
          [ -n "$piper_bin" ] || { echo "staged archive for $name does not contain '$exe'" >&2; return 1; }
          parent="$(dirname "$piper_bin")"
          ensure_dir "$BIN_DIR"
          mkdir -p "$BIN_DIR" && cp -R "$parent/." "$BIN_DIR/" ;;
        mpv-app)
          extract_archive "$archive" "$ext"
          local inner
          inner="$(find "$ext" -maxdepth 1 -name '*.tar.gz' -type f 2>/dev/null | head -n 1 || true)"
          if [ -n "$inner" ]; then
            local inner_dir="$ext/inner"
            ensure_dir "$inner_dir"
            tar -xzf "$inner" -C "$inner_dir"
            ext="$inner_dir"
          fi
          local macos_dir
          macos_dir="$(find "$ext" -type d -path "*/Contents/MacOS" 2>/dev/null | head -n 1 || true)"
          [ -n "$macos_dir" ] || { echo "staged archive for $name does not contain Contents/MacOS" >&2; return 1; }
          ensure_dir "$BIN_DIR"
          cp -R "$macos_dir/." "$BIN_DIR/"
          [ -f "$BIN_DIR/mpv" ] || { echo "staged archive for $name does not contain Contents/MacOS/mpv" >&2; return 1; }
          [ "$OS" = "Darwin" ] && { chmod +x "$BIN_DIR/mpv"; xattr -dr com.apple.quarantine "$BIN_DIR/mpv" 2>/dev/null || true; } ;;
        ffmpeg-linux)
          extract_archive "$archive" "$ext"
          local exe_bin exe_dir lib_dir
          exe_bin="$(find_one "$ext" "$exe")"
          [ -n "$exe_bin" ] || { echo "staged archive for $name does not contain '$exe'" >&2; return 1; }
          exe_dir="$(dirname "$exe_bin")"
          lib_dir="$(dirname "$(find "$ext" -path '*/lib/libavcodec.so*' 2>/dev/null | head -n 1 || true)")"
          ensure_dir "$BIN_DIR"
          cp "$exe_dir/ffmpeg" "$exe_dir/ffprobe" "$BIN_DIR/"
          if [ -n "$lib_dir" ] && [ -d "$lib_dir" ]; then
            ensure_dir "$DIST_DIR/lib"
            cp "$lib_dir"/libav*.so* "$lib_dir"/libsw*.so* "$DIST_DIR/lib/"
          fi ;;
      esac
      chmod +x "$staged" 2>/dev/null || true
      if [ -d "$staged" ]; then
        staged_hash="$pin"
        ensure_dir "$STATE_DIR"
        printf '%s\n' "$pin" > "$STATE_DIR/$(basename "$dest").sha256"
      else
        staged_hash="$(sha256_of "$staged")"
        write_marker "$staged"
      fi
    fi
    local stem
    stem="$(basename "$dest")"
    manifest_entries+=("{
    \"name\": \"$(json_quote "$stem")\",
    \"version\": \"$(json_quote "$version")\",
    \"sha256\": \"$(json_quote "$staged_hash")\",
    \"source_url\": \"$(json_quote "$url")\"
  }")
  done

  local versions_path="$DIST_DIR/versions.json"
  if [ "$DRY_RUN" = "1" ]; then
    log "would write $versions_path (manifest)"
    printf '[\n%s\n]\n' "$(IFS=','; echo "${manifest_entries[*]}")"
  else
    local tmp
    tmp="$versions_path.tmp.$$"
    {
      echo "["
      local i=0
      for e in "${manifest_entries[@]}"; do
        i=$((i+1))
        printf '%s' "$e"
        [ "$i" -lt "${#manifest_entries[@]}" ] && echo ","
        echo
      done
      echo "]"
    } > "$tmp"
    mv "$tmp" "$versions_path"
    echo "versions.json written: ${#manifest_entries[@]} artifact(s) in $DIST_DIR"
  fi
  echo "Done. Stage dist/{bin,models}/ + versions.json into the installer."
}

main "$@"
