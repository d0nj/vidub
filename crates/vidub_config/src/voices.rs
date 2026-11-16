//! Bucket voice catalog + download URLs.

#[cfg(test)]
use std::collections::HashSet;

/// One bucket voice: display name + `.onnx` file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceInfo {
    pub name: &'static str,
    pub file: &'static str,
}

/// All 25 bucket voices, alphabetical by display name.
///
/// Non-default voices download on demand into the cache `models/` dir.
pub const VOICES: &[VoiceInfo] = &[
    VoiceInfo {
        name: "adam1",
        file: "adam1.onnx",
    },
    VoiceInfo {
        name: "Ban Mai",
        file: "banmai.onnx",
    },
    VoiceInfo {
        name: "calmwoman3688",
        file: "calmwoman3688.onnx",
    },
    VoiceInfo {
        name: "Chiếu Thành",
        file: "chieuthanh.onnx",
    },
    VoiceInfo {
        name: "deepman3909",
        file: "deepman3909.onnx",
    },
    VoiceInfo {
        name: "Duy Oryx",
        file: "duyoryx3175.onnx",
    },
    VoiceInfo {
        name: "Lạc Phi",
        file: "lacphi.onnx",
    },
    VoiceInfo {
        name: "Mai Phương",
        file: "maiphuong.onnx",
    },
    VoiceInfo {
        name: "Mạnh Dũng",
        file: "manhdung.onnx",
    },
    VoiceInfo {
        name: "Minh Khang",
        file: "minhkhang.onnx",
    },
    VoiceInfo {
        name: "Minh Quang",
        file: "minhquang.onnx",
    },
    VoiceInfo {
        name: "Minh Thu",
        file: "minhthu.onnx",
    },
    VoiceInfo {
        name: "Mỹ Tâm",
        file: "mytam2794.onnx",
    },
    VoiceInfo {
        name: "Mỹ Tâm 2",
        file: "mytam2.onnx",
    },
    VoiceInfo {
        name: "Ngọc Huyền",
        file: "ngochuyen.onnx",
    },
    VoiceInfo {
        name: "Ngọc Huyền mới",
        file: "ngochuyennew.onnx",
    },
    VoiceInfo {
        name: "Ngọc Ngạn",
        file: "ngocngan3701.onnx",
    },
    VoiceInfo {
        name: "Phương Trang",
        file: "phuongtrang.onnx",
    },
    VoiceInfo {
        name: "Tài An",
        file: "taian2.onnx",
    },
    VoiceInfo {
        name: "Tài An 4",
        file: "taian4.onnx",
    },
    VoiceInfo {
        name: "Thanh Phương",
        file: "thanhphuong2.onnx",
    },
    VoiceInfo {
        name: "Thiện Tâm",
        file: "thientam.onnx",
    },
    VoiceInfo {
        name: "Trấn Thành",
        file: "tranthanh3870.onnx",
    },
    VoiceInfo {
        name: "Việt Thảo",
        file: "vietthao3886.onnx",
    },
    VoiceInfo {
        name: "yannew",
        file: "yannew.onnx",
    },
];

/// Bucket resolve base for voice files (renzynx/nghitts-copy-bucket).
pub const VOICE_DOWNLOAD_BASE: &str =
    "https://huggingface.co/buckets/renzynx/nghitts-copy-bucket/resolve/piper-tts";

/// Full download URL for a voice file (e.g. `ngochuyennew.onnx`).
/// `file` is a bare file name as listed in [`VOICES`].
#[must_use]
pub fn voice_url(file: &str) -> String {
    format!("{VOICE_DOWNLOAD_BASE}/{file}?download=true")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voices_twenty_five_unique_onnx() {
        assert_eq!(VOICES.len(), 25);
        let mut files = HashSet::new();
        let mut names = HashSet::new();
        for v in VOICES {
            assert!(v.file.ends_with(".onnx"), "file={}", v.file);
            assert!(!v.name.is_empty() && !v.file.is_empty());
            assert!(files.insert(v.file), "dup file {}", v.file);
            assert!(names.insert(v.name), "dup name {}", v.name);
        }
        // Default voice must be present.
        assert!(files.contains("ngochuyennew.onnx"));
    }

    #[test]
    fn voice_url_shape() {
        let u = voice_url("ngochuyennew.onnx");
        assert_eq!(
            u,
            "https://huggingface.co/buckets/renzynx/nghitts-copy-bucket/resolve/piper-tts/ngochuyennew.onnx?download=true"
        );
        assert!(u.ends_with("?download=true"));
        // Every catalog entry builds a well-formed URL.
        for v in VOICES {
            let u = voice_url(v.file);
            assert!(u.starts_with(VOICE_DOWNLOAD_BASE));
            assert!(u.ends_with(&format!("/{}?download=true", v.file)));
        }
    }
}
