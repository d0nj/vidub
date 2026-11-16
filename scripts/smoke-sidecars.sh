#!/usr/bin/env bash
set -uo pipefail

BIN_DIR="${1:-}"
shift || true
REQUIRED=""
while [ $# -gt 0 ]; do
  case "$1" in
    --require) REQUIRED="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

[ -n "$BIN_DIR" ] && [ -d "$BIN_DIR" ] || { echo "smoke: bin dir missing: $BIN_DIR" >&2; exit 2; }

is_required() { case ",$REQUIRED," in *",$1,"*) return 0 ;; *) return 1 ;; esac; }

failures=0
checked=0
skipped=0

probe() {
  local name="$1" expect="$2"; shift 2
  local exe
  for cand in "$BIN_DIR/$name" "$BIN_DIR/$name.exe"; do
    [ -f "$cand" ] && exe="$cand" && break
  done
  if [ -z "${exe:-}" ]; then
    if is_required "$name"; then
      echo "FAIL  $name — required but not shipped"
      failures=$((failures + 1))
    else
      echo "skip  $name — not shipped (optional here)"
      skipped=$((skipped + 1))
    fi
    return
  fi
  checked=$((checked + 1))
  local out rc
  out="$("$exe" "$@" 2>&1)"; rc=$?
  if [ $rc -eq 0 ] && { [ -z "$expect" ] || printf '%s' "$out" | grep -qF "$expect"; }; then
    echo "ok    $name"
    return
  fi
  echo "FAIL  $name — exit $rc"
  printf '%s\n' "$out" | head -6 | sed 's/^/        /'
  failures=$((failures + 1))
}

probe yt-dlp "2026" --version
probe mpv "" --version
probe piper "" --help
probe qjs "" --version
probe ffmpeg "ffmpeg version" -version
probe ffprobe "ffprobe version" -version

echo "---"
echo "checked=$checked skipped=$skipped failures=$failures"
if [ "$failures" -gt 0 ]; then
  echo "SMOKE FAILED: $failures bundled binary/binaries do not run" >&2
  exit 1
fi
echo "SMOKE OK"
