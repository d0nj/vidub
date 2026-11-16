# ViDub — Third-Party Notices

Sidecars ship inside the app package with their respective licenses below.
Workspace crates (`vidub_*`) are `publish = false` and pre-licensing.

| Component | Version pin | License (shipped form) | Source |
|---|---|---|---|
| yt-dlp | 2026.10.26 | **GPL-3.0+** for the bundled standalone executables. Upstream source is The Unlicense, but the official standalone binaries (PyInstaller bundles) include GPLv3+-licensed code and the combined work is GPL-3.0+. | https://github.com/yt-dlp/yt-dlp (license: https://github.com/yt-dlp/yt-dlp/blob/master/LICENSE; bundle notice: https://github.com/yt-dlp/yt-dlp/THIRD_PARTY_LICENSES.txt) |
| FFmpeg (LGPL shared, BtbN autobuild) | N-128728-g49a3b1f259 (`autobuild-2026-11-10-12-39`, `lgpl-shared`) | LGPL-2.1-or-later (`lgpl-shared` excludes GPL-only libraries such as libx264/libx265). | Build: https://github.com/BtbN/FFmpeg-Builds · Upstream: https://ffmpeg.org / https://github.com/FFmpeg/FFmpeg |
| mpv (Windows/macOS only; Linux uses system package) | v0.42.0 | GPL-2.0-or-later in official builds. | https://github.com/mpv-player/mpv (license text: https://github.com/mpv-player/mpv/blob/master/Copyright + `LICENSE.GPL`) |
| piper | 2023.11.14-2 | MIT | https://github.com/rhasspy/piper |
| espeak-ng (bundled with piper + `espeak-ng-data`) | bundled with piper 2023.11.14-2 | GPL-3.0-or-later | https://github.com/espeak-ng/espeak-ng |
| piper-phonemize (bundled with piper) | bundled with piper 2023.11.14-2 | MIT | https://github.com/rhasspy/piper-phonemize |
| ONNX Runtime (bundled with piper) | bundled with piper 2023.11.14-2 | MIT | https://github.com/microsoft/onnxruntime |
| QuickJS-NG (`qjs`, YouTube JS challenges for yt-dlp) | v0.18.0 | MIT | https://github.com/quickjs-ng/quickjs |
| Piper voice `ngochuyennew.onnx` + shared `config.json` | bucket snapshot 2026-11-11 | See caveat below — Apache-2.0 (upstream repo LICENSE) with no standalone license found for the ONNX weights themselves. | Mirror: https://huggingface.co/buckets/renzynx/nghitts-copy-bucket · Upstream: https://github.com/nghimestudio/nghitts |

## Voice-model license caveat

- The upstream nghitts repository (`nghimestudio/nghitts`) carries an **Apache-2.0** `LICENSE`.
- The ONNX voice weights are not vendored in that repo, and the Hugging Face mirror bucket (`renzynx/nghitts-copy-bucket`) carries no license file.
- No standalone license was found upstream for the `ngochuyennew.onnx` / `config.json` voice files themselves; redistribution assumes the upstream repo's Apache-2.0 LICENSE.

## Distribution notes

- **FFmpeg LGPL-shared:** keep DLLs/`.so` replaceable next to the binaries; do not static-link.
- **mpv GPL-2.0+ / yt-dlp GPL-3.0+ / espeak-ng GPL-3.0+:** keep this notice and source links in every bundle. The Linux `.deb` declares `Recommends: mpv` and does not bundle mpv.
- **Rust crates:** audited in CI via `deny.toml` (`cargo-deny check bans licenses sources`; no GPL allowed in Rust crate dependencies).
- **Cloud models (not redistributed):** NVIDIA NIM / Whisper NVCF, OpenRouter, and Google Gemini are live API calls under their respective service terms.
