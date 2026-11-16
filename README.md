# ViDub — local video dubbing (Rust + Slint)

ViDub dubs local videos or YouTube/Bilibili links into Vietnamese speech:
transcript (captions-first, falling back to Whisper STT) → translation
(NVIDIA Build, OpenRouter, or Google Gemini, with Google `gtx` fallback) →
Piper TTS → pace/sync → mpv preview → ffmpeg export.

## Packages (built by `.github/workflows/release.yml`)

Release builds bundle pinned, SHA-256-verified sidecars alongside
`THIRD_PARTY_NOTICES.md`, `versions.json`, and `models.toml`.

### Windows — NSIS installer

`output/ViDubSetup-<VERSION>.exe` — per-user install into `%LOCALAPPDATA%\ViDub`
with a Start Menu shortcut and third-party license notices. Silent install:

```powershell
ViDubSetup-<VERSION>.exe /S /D=C:\path\to\dir
```

Layout: `vidub_app.exe` beside `bin\` (`yt-dlp`, `ffmpeg`, `ffprobe`, `mpv`,
`qjs`, `piper` + DLLs + `espeak-ng-data\`), `models\` (`ngochuyennew.onnx` +
`config.json`), `versions.json`, `models.toml`, `THIRD_PARTY_NOTICES.md`, and
`Uninstall.exe`.

### Linux — tarball or `.deb` (`x86_64`)

`output/vidub-<VERSION>-linux-amd64.tar.gz` or `output/vidub_<VERSION>_amd64.deb`
(installs to `/usr/lib/vidub`, symlinked at `/usr/bin/vidub`, with
`vidub.desktop`). `ffmpeg`/`ffprobe`/`piper` are bundled in `bin/` with
`$ORIGIN` RUNPATH; `mpv` is not bundled (`Recommends: mpv` — install via
`sudo apt install mpv` for preview; export works without it).

### macOS — app-bundle script

`packaging/mac/assemble.sh` builds an unsigned `ViDub-<VERSION>-macos-<arch>.zip`
targeting macOS 15+. `ffmpeg` is resolved from Homebrew (`/opt/homebrew/bin` or
`/usr/local/bin`); `yt-dlp`, `mpv`, `qjs`, `piper`, and the voice model are
bundled. There is currently no active macOS job in CI/release workflows.

### First run

- **API key:** choose NVIDIA, OpenRouter, or Google Gemini in **Cài đặt** and
  paste your key. Keys are stored in the OS keyring (`NVAPI_KEY`,
  `OPENROUTER_API_KEY`, or `GEMINI_API_KEY` env vars act as fallback/prefill).
- **Missing tools (dev builds):** click **Thiết lập tất cả** in **Cài đặt** to
  download pinned sidecars into the local cache.

## Layout

```text
Cargo.toml                     workspace root (edition 2021, rust-version 1.92)
crates/vidub_core/             transcript (probe/parse/whisper) · translate · voice · sync · export · pipeline
crates/vidub_sidecar/          locator · setup · downloader · versions · quarantine
crates/vidub_mpv/              control · transport · protocol (JSON IPC over pipe/UDS)
crates/vidub_config/           settings.json · keyring key store · voice catalog · model presets
crates/vidub_process/          child-process console hiding
crates/vidub_app/              Slint app wiring
ui/                            Slint UI (ui/app.slint)
scripts/fetch-sidecars.{ps1,sh}  pinned sidecar staging into dist/
packaging/{windows,mac,linux}/   installer, app-bundle, tarball, and .deb assembly
```

## Build / test / lint

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo-deny check bans licenses sources
```

CI (`.github/workflows/ci.yml`) runs `verify` on `ubuntu-latest` (format, check,
clippy, workspace tests, cargo-deny) and `matrix` on `windows-latest` for pull
requests and tags (check, clippy, workspace tests).

## Fetching sidecars

Run the fetch script at release/packaging time to populate `dist/`:

```powershell
.\scripts\fetch-sidecars.ps1            # Windows
.\scripts\fetch-sidecars.ps1 -WhatIf    # dry run
```

```sh
./scripts/fetch-sidecars.sh             # Linux / macOS
./scripts/fetch-sidecars.sh --dry-run   # dry run
```

Both scripts stage `dist/bin/`, `dist/models/` (`ngochuyennew.onnx` +
`config.json`), `dist/versions.json`, and (on Linux) `dist/lib/`, verifying every
download against its pinned SHA-256. Pinned versions match
`crates/vidub_sidecar/src/setup/pins.rs` (`yt-dlp` 2026.10.26, `mpv` v0.42.0,
BtbN FFmpeg LGPL `autobuild-2026-11-10-12-39`, `piper` 2023.11.14-2, `qjs`
v0.18.0, voice `ngochuyennew.onnx`). Update pins in both fetch scripts and
`pins.rs` together.

## Licensing

Bundled third-party binaries and weights carry their own licenses — see
`THIRD_PARTY_NOTICES.md`. Rust crate dependencies are checked by `deny.toml`.
