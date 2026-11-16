use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

pub const DEFAULT_VOICE_FILE: &str = "ngochuyennew.onnx";

pub const MODEL_BOLT: &str = "nvidia/nemotron-3.5-bolt-e4b";
pub const MODEL_GEMMA: &str = "google/gemma-4-31b-it";
pub const MODEL_MINIMAX: &str = "minimaxai/minimax-m3.1-flash";
pub const MODEL_GLIMMER: &str = "meta/muse-glimmer-30b";
pub const MODEL_ULTRA: &str = "nvidia/nemotron-3-ultra-550b-a55b";
pub const MODEL_RIVA: &str = "nvidia/riva-translate-4b-instruct-v2";

pub const OR_GEMINI_FLASH: &str = "google/gemini-2.5-flash";
pub const OR_GEMINI_PRO: &str = "google/gemini-2.5-pro";
pub const OR_LLAMA_70B: &str = "meta-llama/llama-3.3-70b-instruct";
pub const OR_CLAUDE_HAIKU: &str = "anthropic/claude-3.5-haiku";
pub const OR_CLAUDE_SONNET: &str = "anthropic/claude-sonnet-4";
pub const OR_GPT_4O_MINI: &str = "openai/gpt-4o-mini";
pub const OR_GPT_4O: &str = "openai/gpt-4o";

pub const GEMINI_FLASH: &str = "gemini-2.5-flash";
pub const GEMINI_FLASH_LITE: &str = "gemini-2.5-flash-lite";
pub const GEMINI_PRO: &str = "gemini-2.5-pro";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    Nvidia,
    #[serde(rename = "openrouter")]
    OpenRouter,
    Gemini,
}

impl Provider {
    #[must_use]
    pub const fn key_env_var(self) -> &'static str {
        match self {
            Self::Nvidia => "NVAPI_KEY",
            Self::OpenRouter => "OPENROUTER_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
        }
    }

    #[must_use]
    pub(crate) const fn key_user(self) -> &'static str {
        match self {
            Self::Nvidia => "nvapi",
            Self::OpenRouter => "openrouter",
            Self::Gemini => "gemini",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPreset {
    Fast,
    #[default]
    Balanced,
    Quality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoudnessPreset {
    Youtube,
    #[default]
    Podcast,
    Broadcast,
}

impl LoudnessPreset {
    #[must_use]
    pub const fn target_lufs(self) -> f32 {
        match self {
            Self::Youtube => -14.0,
            Self::Podcast => -16.0,
            Self::Broadcast => -23.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default = "default_voice_file")]
    pub voice_file: String,
    #[serde(default)]
    pub preset: ModelPreset,
    #[serde(default)]
    pub output_dir: Option<PathBuf>,
    #[serde(default)]
    pub loudness: LoudnessPreset,
    #[serde(default)]
    pub provider: Provider,
}

fn default_voice_file() -> String {
    DEFAULT_VOICE_FILE.to_string()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            voice_file: default_voice_file(),
            preset: ModelPreset::Balanced,
            output_dir: None,
            loudness: LoudnessPreset::Podcast,
            provider: Provider::Nvidia,
        }
    }
}

#[must_use]
pub fn chain_for_provider(provider: Provider, preset: ModelPreset) -> (String, Vec<String>) {
    let (primary, fallback_ids): (&str, &[&str]) = match provider {
        Provider::Nvidia => (
            MODEL_BOLT,
            match preset {
                ModelPreset::Fast => &[MODEL_GEMMA],
                ModelPreset::Balanced => &[MODEL_GEMMA, MODEL_MINIMAX, MODEL_GLIMMER],
                ModelPreset::Quality => &[MODEL_GEMMA, MODEL_MINIMAX, MODEL_GLIMMER, MODEL_ULTRA],
            },
        ),
        Provider::OpenRouter => match preset {
            ModelPreset::Fast => (OR_GEMINI_FLASH, &[OR_LLAMA_70B]),
            ModelPreset::Balanced => (OR_GEMINI_FLASH, &[OR_CLAUDE_HAIKU, OR_GPT_4O_MINI]),
            ModelPreset::Quality => (OR_CLAUDE_SONNET, &[OR_GPT_4O, OR_GEMINI_PRO]),
        },
        Provider::Gemini => match preset {
            ModelPreset::Fast => (GEMINI_FLASH, &[GEMINI_FLASH_LITE]),
            ModelPreset::Balanced => (GEMINI_FLASH, &[GEMINI_PRO, GEMINI_FLASH_LITE]),
            ModelPreset::Quality => (GEMINI_PRO, &[GEMINI_FLASH]),
        },
    };
    let mut out: Vec<String> = fallback_ids.iter().map(|s| (*s).to_string()).collect();
    if provider == Provider::Nvidia {
        out.push(MODEL_RIVA.to_string());
    }
    (primary.to_string(), out)
}

#[must_use]
pub fn settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("vidub").join("settings.json"))
}

#[must_use]
pub fn load() -> AppSettings {
    let Some(path) = settings_path() else {
        return AppSettings::default();
    };
    load_from(&path)
}

pub(crate) fn load_from(path: &Path) -> AppSettings {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return AppSettings::default(),
        Err(e) => {
            tracing::warn!(
                "vidub_config: unreadable {}: {e}; using defaults",
                path.display()
            );
            return AppSettings::default();
        }
    };
    match serde_json::from_str::<AppSettings>(&text) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                "vidub_config: corrupt {}: {e}; using defaults",
                path.display()
            );
            AppSettings::default()
        }
    }
}

pub fn save(settings: &AppSettings) -> anyhow::Result<()> {
    let Some(path) = settings_path() else {
        anyhow::bail!("config dir unavailable (dirs::config_dir() is None)");
    };
    save_to(&path, settings)
}

pub(crate) fn save_to(path: &Path, settings: &AppSettings) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("settings path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let json = serde_json::to_string_pretty(settings).context("serialize settings")?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("create temp in {}", parent.display()))?;
    tmp.write_all(json.as_bytes()).context("write settings")?;
    tmp.write_all(b"\n").context("write settings")?;
    tmp.flush().context("flush settings")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {}", tmp.path().display()))?;
    }
    tmp.persist(path)
        .map_err(|e| anyhow::anyhow!("atomic rename settings: {}", e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_balanced_podcast_ngochuyennew() {
        let s = AppSettings::default();
        assert_eq!(s.voice_file, DEFAULT_VOICE_FILE);
        assert_eq!(s.voice_file, "ngochuyennew.onnx");
        assert_eq!(s.preset, ModelPreset::Balanced);
        assert_eq!(s.output_dir, None);
        assert_eq!(s.loudness, LoudnessPreset::Podcast);
        assert_eq!(s.provider, Provider::Nvidia);
        assert_eq!(Provider::default(), Provider::Nvidia);
        assert_eq!(LoudnessPreset::Youtube.target_lufs(), -14.0);
        assert_eq!(LoudnessPreset::Podcast.target_lufs(), -16.0);
        assert_eq!(LoudnessPreset::Broadcast.target_lufs(), -23.0);
    }

    #[test]
    fn round_trip_save_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vidub").join("settings.json");
        let want = AppSettings {
            voice_file: "adam1.onnx".to_string(),
            preset: ModelPreset::Quality,
            output_dir: Some(dir.path().join("out")),
            loudness: LoudnessPreset::Broadcast,
            provider: Provider::OpenRouter,
        };
        save_to(&path, &want).expect("save");
        let got = load_from(&path);
        assert_eq!(got, want);
    }

    #[test]
    fn missing_and_corrupt_load_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            load_from(&dir.path().join("absent.json")),
            AppSettings::default()
        );
        let bad = dir.path().join("settings.json");
        std::fs::write(&bad, "{not json").expect("write corrupt");
        assert_eq!(load_from(&bad), AppSettings::default());
        std::fs::write(&bad, "").expect("write empty");
        assert_eq!(load_from(&bad), AppSettings::default());
        std::fs::write(&bad, r#"{"voice_file":"adam1.onnx","preset":"turbo"}"#).expect("write");
        assert_eq!(load_from(&bad), AppSettings::default());
    }

    #[test]
    fn old_json_without_provider_loads_as_nvidia() {
        let old = r#"{
            "voice_file": "adam1.onnx",
            "preset": "fast",
            "output_dir": null,
            "loudness": "youtube"
        }"#;
        let s: AppSettings = serde_json::from_str(old).expect("old json loads");
        assert_eq!(s.provider, Provider::Nvidia);
        assert_eq!(s.preset, ModelPreset::Fast);
        assert_eq!(s.voice_file, "adam1.onnx");

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        std::fs::write(&path, old).expect("write old");
        assert_eq!(load_from(&path).provider, Provider::Nvidia);
    }

    #[test]
    fn provider_serde_round_trip() {
        assert_eq!(
            serde_json::to_string(&Provider::Nvidia).expect("ser"),
            r#""nvidia""#
        );
        assert_eq!(
            serde_json::to_string(&Provider::OpenRouter).expect("ser"),
            r#""openrouter""#
        );
        let s = AppSettings {
            provider: Provider::OpenRouter,
            ..AppSettings::default()
        };
        let json = serde_json::to_string(&s).expect("ser");
        assert!(json.contains(r#""provider":"openrouter""#), "{json}");
        let back: AppSettings = serde_json::from_str(&json).expect("de");
        assert_eq!(back.provider, Provider::OpenRouter);
    }

    #[test]
    fn provider_env_vars_and_key_users() {
        assert_eq!(Provider::Nvidia.key_env_var(), "NVAPI_KEY");
        assert_eq!(Provider::OpenRouter.key_env_var(), "OPENROUTER_API_KEY");
        assert_eq!(Provider::Gemini.key_env_var(), "GEMINI_API_KEY");
        assert_eq!(Provider::Nvidia.key_user(), "nvapi");
        assert_eq!(Provider::OpenRouter.key_user(), "openrouter");
        assert_eq!(Provider::Gemini.key_user(), "gemini");
    }

    #[test]
    fn chain_matrix_all_providers_and_presets() {
        #[allow(clippy::type_complexity)]
        let want: &[(Provider, ModelPreset, &str, &[&str])] = &[
            (
                Provider::Nvidia,
                ModelPreset::Fast,
                "nvidia/nemotron-3.5-bolt-e4b",
                &[
                    "google/gemma-4-31b-it",
                    "nvidia/riva-translate-4b-instruct-v2",
                ],
            ),
            (
                Provider::Nvidia,
                ModelPreset::Balanced,
                "nvidia/nemotron-3.5-bolt-e4b",
                &[
                    "google/gemma-4-31b-it",
                    "minimaxai/minimax-m3.1-flash",
                    "meta/muse-glimmer-30b",
                    "nvidia/riva-translate-4b-instruct-v2",
                ],
            ),
            (
                Provider::Nvidia,
                ModelPreset::Quality,
                "nvidia/nemotron-3.5-bolt-e4b",
                &[
                    "google/gemma-4-31b-it",
                    "minimaxai/minimax-m3.1-flash",
                    "meta/muse-glimmer-30b",
                    "nvidia/nemotron-3-ultra-550b-a55b",
                    "nvidia/riva-translate-4b-instruct-v2",
                ],
            ),
            (
                Provider::OpenRouter,
                ModelPreset::Fast,
                "google/gemini-2.5-flash",
                &["meta-llama/llama-3.3-70b-instruct"],
            ),
            (
                Provider::OpenRouter,
                ModelPreset::Balanced,
                "google/gemini-2.5-flash",
                &["anthropic/claude-3.5-haiku", "openai/gpt-4o-mini"],
            ),
            (
                Provider::OpenRouter,
                ModelPreset::Quality,
                "anthropic/claude-sonnet-4",
                &["openai/gpt-4o", "google/gemini-2.5-pro"],
            ),
            (
                Provider::Gemini,
                ModelPreset::Fast,
                "gemini-2.5-flash",
                &["gemini-2.5-flash-lite"],
            ),
            (
                Provider::Gemini,
                ModelPreset::Balanced,
                "gemini-2.5-flash",
                &["gemini-2.5-pro", "gemini-2.5-flash-lite"],
            ),
            (
                Provider::Gemini,
                ModelPreset::Quality,
                "gemini-2.5-pro",
                &["gemini-2.5-flash"],
            ),
        ];
        assert_eq!(want.len(), 9);
        for (provider, preset, primary, fallbacks) in want {
            let (p, f) = chain_for_provider(*provider, *preset);
            assert_eq!(p, *primary, "{provider:?}/{preset:?} primary");
            let want_f: Vec<String> = fallbacks.iter().map(|s| (*s).to_string()).collect();
            assert_eq!(f, want_f, "{provider:?}/{preset:?} fallbacks");
            if *provider != Provider::Nvidia {
                assert!(!f.iter().any(|m| m.contains("riva")));
            }
            let mut all = vec![p];
            all.extend(f);
            let mut sorted = all.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(all.len(), sorted.len(), "duplicate id in {all:?}");
        }
    }
}
