//! `models.toml` `[whisper] function_id` resolution (config, not code).

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Extract `function_id` from a `models.toml` document's `[whisper]` table.
pub fn parse_function_id(toml: &str) -> Option<String> {
    let mut in_whisper = false;
    for line in toml.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_whisper = line.trim_end_matches(']').trim_start_matches('[').trim() == "whisper";
            continue;
        }
        if in_whisper {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == "function_id" {
                    let v = v.trim().trim_matches('"').trim_matches('\'');
                    if !v.is_empty() {
                        return Some(v.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Read `models.toml` for the whisper `function_id` (config, not code).
/// Resolved at runtime, NOT compile time: an installed exe is not under the
/// crate dir. Candidates: `<exe-dir>/models.toml` (bundled install), then
/// `<exe-dir>/../../models.toml` (dev `target/<profile>/` layout).
pub fn whisper_function_id() -> Result<String> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let candidates = [
        exe_dir.join("models.toml"),
        exe_dir.join("..").join("..").join("models.toml"),
    ];
    let mut last_err: Option<std::io::Error> = None;
    for path in &candidates {
        match std::fs::read_to_string(path) {
            Ok(toml) => {
                return parse_function_id(&toml)
                    .with_context(|| format!("no [whisper] function_id in {}", path.display()));
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap()).with_context(|| {
        format!(
            "reading models.toml ({}, {})",
            candidates[0].display(),
            candidates[1].display()
        )
    })
}
