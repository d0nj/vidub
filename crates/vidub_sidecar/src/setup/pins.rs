//! Pin table: what to download per (os, arch), and system-only guidance.
//!
//! The pin table values are copied verbatim from `scripts/fetch-sidecars.ps1`
//! and `scripts/fetch-sidecars.sh`. If upstream assets change, bump the pins in
//! both scripts and this table together.

/// How a pinned artifact is turned into staged files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinKind {
    /// Download staged as-is (yt-dlp single-file binaries, voice model/config).
    SingleFile,
    /// Zip archive: copy the directory containing `exe` wholesale into cache
    /// `bin/` (mirrors the fetch scripts' zip-dir semantics, exe + DLLs/data).
    ZipExe { exe: &'static str },
    /// tar.gz archive: same wholesale copy as [`PinKind::ZipExe`].
    TarGzExe { exe: &'static str },
    /// Linux ffmpeg tar.xz: stage `bin/ffmpeg` + `bin/ffprobe` and `lib/libav*.so*`.
    TarXzFfmpeg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin {
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub kind: PinKind,
    /// Staged path relative to the cache base (`<cache-root>/..`),
    /// e.g. `bin/yt-dlp.exe`, `models/config.json`.
    pub dest: &'static str,
}

const VOICE_ONNX_URL: &str = "https://huggingface.co/buckets/renzynx/nghitts-copy-bucket/resolve/piper-tts/ngochuyennew.onnx?download=true";
const VOICE_ONNX_SHA: &str = "88bc21477831dd99759cff164f0ab270e258faf435d01741f0211ff8620255e2";
const VOICE_CONFIG_URL: &str = "https://huggingface.co/buckets/renzynx/nghitts-copy-bucket/resolve/piper-tts/config.json?download=true";
const VOICE_CONFIG_SHA: &str = "6ce7b1bac4cf40410cd3c75fb100475306877850f5697f3d4daeeb3225410271";

const VOICE_ONNX_PIN: Pin = Pin {
    name: "voice-onnx",
    url: VOICE_ONNX_URL,
    sha256: VOICE_ONNX_SHA,
    kind: PinKind::SingleFile,
    dest: "models/ngochuyennew.onnx",
};

const VOICE_CONFIG_PIN: Pin = Pin {
    name: "voice-config",
    url: VOICE_CONFIG_URL,
    sha256: VOICE_CONFIG_SHA,
    kind: PinKind::SingleFile,
    dest: "models/config.json",
};

/// Downloadable pins for one (os, arch). Empty for unsupported combinations.
pub(crate) fn pins_for(os: &str, arch: &str) -> Vec<Pin> {
    match (os, arch) {
        ("windows", "x86_64") => vec![
            Pin {
                name: "yt-dlp",
                url: "https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/yt-dlp.exe",
                sha256: "f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d",
                kind: PinKind::SingleFile,
                dest: "bin/yt-dlp.exe",
            },
            Pin {
                name: "mpv",
                url: "https://github.com/mpv-player/mpv/releases/download/v0.42.0/mpv-v0.42.0-x86_64-pc-windows-msvc.zip",
                sha256: "8dd69cbab079086765fa5d873c09513b2834b85d146c1301718a757e8c0595ef",
                kind: PinKind::ZipExe { exe: "mpv.exe" },
                dest: "bin/mpv.exe",
            },
            Pin {
                name: "ffmpeg",
                url: "https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-10-12-39/ffmpeg-N-128728-g49a3b1f259-win64-lgpl-shared.zip",
                sha256: "acb67f124da510d346c0f2afe19face6f316f0d8959b595ad5050b1b86bfbcbc",
                kind: PinKind::ZipExe { exe: "ffmpeg.exe" },
                dest: "bin/ffmpeg.exe",
            },
            Pin {
                name: "ffprobe",
                url: "https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-10-12-39/ffmpeg-N-128728-g49a3b1f259-win64-lgpl-shared.zip",
                sha256: "acb67f124da510d346c0f2afe19face6f316f0d8959b595ad5050b1b86bfbcbc",
                kind: PinKind::ZipExe { exe: "ffprobe.exe" },
                dest: "bin/ffprobe.exe",
            },
            Pin {
                name: "piper",
                url: "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_windows_amd64.zip",
                sha256: "f3c58906402b24f3a96d92145f58acba6d86c9b5db896d207f78dc80811efcea",
                kind: PinKind::ZipExe { exe: "piper.exe" },
                dest: "bin/piper.exe",
            },
            Pin {
                name: "qjs",
                url: "https://github.com/quickjs-ng/quickjs/releases/download/v0.18.0/qjs-windows-x86_64.exe",
                sha256: "ff03398d74ea6e6bd3a8e58058f7003225abcac229d7d57775b7aaae812a49c7",
                kind: PinKind::SingleFile,
                dest: "bin/qjs.exe",
            },
            VOICE_ONNX_PIN,
            VOICE_CONFIG_PIN,
        ],
        ("linux", "x86_64") => vec![
            Pin {
                name: "yt-dlp",
                url: "https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/yt-dlp_linux",
                sha256: "767ba9dce7052aa196f8b12652f433d76de128ba168966fa84b6394d5e6c6795",
                kind: PinKind::SingleFile,
                dest: "bin/yt-dlp",
            },
            Pin {
                name: "ffmpeg",
                url: "https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-10-12-39/ffmpeg-N-128728-g49a3b1f259-linux64-lgpl-shared.tar.xz",
                sha256: "39055baa287af2a3acea041ccd7cca295d2f374dda7210fd2503f637fa238c72",
                kind: PinKind::TarXzFfmpeg,
                dest: "bin/ffmpeg",
            },
            Pin {
                name: "ffprobe",
                url: "https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-10-12-39/ffmpeg-N-128728-g49a3b1f259-linux64-lgpl-shared.tar.xz",
                sha256: "39055baa287af2a3acea041ccd7cca295d2f374dda7210fd2503f637fa238c72",
                kind: PinKind::TarXzFfmpeg,
                dest: "bin/ffprobe",
            },
            Pin {
                name: "piper",
                url: "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_linux_x86_64.tar.gz",
                sha256: "a50cb45f355b7af1f6d758c1b360717877ba0a398cc8cbe6d2a7a3a26e225992",
                kind: PinKind::TarGzExe { exe: "piper" },
                dest: "bin/piper",
            },
            Pin {
                name: "qjs",
                url: "https://github.com/quickjs-ng/quickjs/releases/download/v0.18.0/qjs-linux-x86_64",
                sha256: "85ded23b9fabe8c1644fc40a8d2bbdd71157affcbd807359c618cc717ee24d7d",
                kind: PinKind::SingleFile,
                dest: "bin/qjs",
            },
            VOICE_ONNX_PIN,
            VOICE_CONFIG_PIN,
        ],
        ("macos", "aarch64") => vec![
            Pin {
                name: "yt-dlp",
                url: "https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/yt-dlp_macos",
                sha256: "0f922c15ccc0b83eefa1c8bc9104e434318c4bf16e408e1ee93b7745a6efe0aa",
                kind: PinKind::SingleFile,
                dest: "bin/yt-dlp",
            },
            Pin {
                name: "mpv",
                url: "https://github.com/mpv-player/mpv/releases/download/v0.42.0/mpv-v0.42.0-macos-15-arm.zip",
                sha256: "be1b8d2445e6880860869c080b8710dee189721f1fa4689840db6a2c10d7be69",
                kind: PinKind::ZipExe { exe: "mpv" },
                dest: "bin/mpv",
            },
            Pin {
                name: "piper",
                url: "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_macos_aarch64.tar.gz",
                sha256: "6b1eb03b3735946cb35216e063e7eebcc33a6bbf5dd96ec0217959bf1cdcb0cc",
                kind: PinKind::TarGzExe { exe: "piper" },
                dest: "bin/piper",
            },
            Pin {
                name: "qjs",
                url: "https://github.com/quickjs-ng/quickjs/releases/download/v0.18.0/qjs-darwin-arm64",
                sha256: "91fe89916d3b0a6c01c4dc8382a99c3c1fd91ecfc214aa81ca051902bcce590d",
                kind: PinKind::SingleFile,
                dest: "bin/qjs",
            },
            VOICE_ONNX_PIN,
            VOICE_CONFIG_PIN,
        ],
        ("macos", "x86_64") => vec![
            Pin {
                name: "yt-dlp",
                url: "https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/yt-dlp_macos",
                sha256: "0f922c15ccc0b83eefa1c8bc9104e434318c4bf16e408e1ee93b7745a6efe0aa",
                kind: PinKind::SingleFile,
                dest: "bin/yt-dlp",
            },
            Pin {
                name: "mpv",
                url: "https://github.com/mpv-player/mpv/releases/download/v0.42.0/mpv-v0.42.0-macos-15-intel.zip",
                sha256: "2d2f34350ff844b6014c35f21ca142937275e19c0c673150a8564fcd2f3bfa14",
                kind: PinKind::ZipExe { exe: "mpv" },
                dest: "bin/mpv",
            },
            Pin {
                name: "piper",
                url: "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_macos_x64.tar.gz",
                sha256: "ced85c0a3df13945b1e623b878a48fdc2854d5c485b4b67f62857cf551deaf8b",
                kind: PinKind::TarGzExe { exe: "piper" },
                dest: "bin/piper",
            },
            Pin {
                name: "qjs",
                url: "https://github.com/quickjs-ng/quickjs/releases/download/v0.18.0/qjs-darwin-x86_64",
                sha256: "fa2c5dc78095daff566b32fab995ced12927875253b84216b91b834af2ef2b74",
                kind: PinKind::SingleFile,
                dest: "bin/qjs",
            },
            VOICE_ONNX_PIN,
            VOICE_CONFIG_PIN,
        ],
        _ => Vec::new(),
    }
}

/// System-only binaries per OS (no upstream build to pin): an install
/// instruction is reported instead of a download. Names match the probe names.
pub(crate) fn guidance_for(os: &str) -> Vec<(&'static str, &'static str)> {
    match os {
        // No upstream Linux mpv binaries (fetch-sidecars.sh note).
        "linux" => vec![(
            "mpv",
            "mpv không có bản đóng gói cho Linux. Hãy cài mpv hệ thống: sudo apt install mpv",
        )],
        "macos" => vec![
            (
                "ffmpeg",
                "ffmpeg không được đóng gói cho macOS. Hãy cài ffmpeg hệ thống bằng Homebrew: brew install ffmpeg",
            ),
            (
                "ffprobe",
                "ffprobe không được đóng gói cho macOS. Hãy cài ffmpeg hệ thống bằng Homebrew: brew install ffmpeg",
            ),
        ],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locator::VOICE_FILE_NAME;

    fn is_sha256(s: &str) -> bool {
        s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
    }

    #[test]
    fn pins_all_https_and_sha_pinned() {
        for (os, arch) in [
            ("windows", "x86_64"),
            ("linux", "x86_64"),
            ("macos", "x86_64"),
            ("macos", "aarch64"),
        ] {
            let pins = pins_for(os, arch);
            assert!(!pins.is_empty(), "{os}/{arch} should have pins");
            for pin in &pins {
                assert!(pin.url.starts_with("https://"), "{}", pin.url);
                assert!(is_sha256(pin.sha256), "{}: {}", pin.name, pin.sha256);
            }
        }
    }

    #[test]
    fn pins_per_os_coverage() {
        let names = |os: &str, arch: &str| {
            pins_for(os, arch)
                .into_iter()
                .map(|p| p.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names("windows", "x86_64"),
            [
                "yt-dlp",
                "mpv",
                "ffmpeg",
                "ffprobe",
                "piper",
                "qjs",
                "voice-onnx",
                "voice-config"
            ]
        );
        assert_eq!(
            names("linux", "x86_64"),
            [
                "yt-dlp",
                "ffmpeg",
                "ffprobe",
                "piper",
                "qjs",
                "voice-onnx",
                "voice-config"
            ]
        );
        assert!(!names("linux", "x86_64").contains(&"mpv"));
        for arch in ["x86_64", "aarch64"] {
            assert_eq!(
                names("macos", arch),
                [
                    "yt-dlp",
                    "mpv",
                    "piper",
                    "qjs",
                    "voice-onnx",
                    "voice-config"
                ]
            );
        }
        // macOS arch split: mpv and piper differ per arch, yt-dlp is universal.
        let arm = pins_for("macos", "aarch64");
        let intel = pins_for("macos", "x86_64");
        let find = |ps: &[Pin], name: &str| ps.iter().find(|p| p.name == name).unwrap().url;
        assert_ne!(find(&arm, "mpv"), find(&intel, "mpv"));
        assert_ne!(find(&arm, "piper"), find(&intel, "piper"));
        assert_eq!(find(&arm, "yt-dlp"), find(&intel, "yt-dlp"));
        // qjs comes from a SingleFile pin staged under the canonical name
        // (upstream asset `qjs-<os>-<arch>` → `bin/qjs[.exe]`).
        for (os, arch, dest) in [
            ("windows", "x86_64", "bin/qjs.exe"),
            ("linux", "x86_64", "bin/qjs"),
            ("macos", "aarch64", "bin/qjs"),
            ("macos", "x86_64", "bin/qjs"),
        ] {
            let q = pins_for(os, arch)
                .into_iter()
                .find(|p| p.name == "qjs")
                .expect("qjs pin missing");
            assert_eq!(q.dest, dest);
            assert_eq!(q.kind, PinKind::SingleFile);
            assert!(q.url.starts_with("https://"));
        }
        assert_ne!(find(&arm, "qjs"), find(&intel, "qjs"));
        assert!(pins_for("plan9", "x86_64").is_empty());
        assert_eq!(VOICE_ONNX_PIN.dest, format!("models/{VOICE_FILE_NAME}"));
    }

    #[test]
    fn guidance_for_system_only_oses() {
        assert!(guidance_for("windows").is_empty());
        let linux = guidance_for("linux");
        assert_eq!(linux.len(), 1);
        assert_eq!(linux[0].0, "mpv");
        assert!(linux[0].1.contains("sudo apt install mpv"));
        let mac = guidance_for("macos");
        let names: Vec<_> = mac.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["ffmpeg", "ffprobe"]);
        assert!(mac.iter().all(|(_, g)| g.contains("brew install ffmpeg")));
    }
}
