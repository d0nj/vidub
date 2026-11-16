use std::cmp::Ordering;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub name: String,
    pub version: String,
    pub sha256: String,
    pub source_url: String,
}

pub fn parse_version_output(output: &str) -> Option<String> {
    let line = output.lines().find(|l| !l.trim().is_empty())?;
    for raw in line.split_whitespace() {
        let tok = raw.trim_matches(|c| c == '(' || c == ')' || c == ',' || c == ';');
        for cand in [tok.to_string(), strip_v(tok).unwrap_or(tok).to_string()] {
            if let Some(v) = normalize_version_token(&cand) {
                return Some(v);
            }
        }
    }
    None
}

fn strip_v(s: &str) -> Option<&str> {
    let rest = s.strip_prefix('v').or_else(|| s.strip_prefix('V'))?;
    if rest.starts_with(|c: char| c.is_ascii_digit()) {
        Some(rest)
    } else {
        None
    }
}

fn normalize_version_token(tok: &str) -> Option<String> {
    if is_calver(tok) {
        return Some(tok.to_string());
    }
    if semver::Version::parse(tok).is_ok() {
        return Some(tok.to_string());
    }
    if let Some((core, _suffix)) = tok.split_once('-') {
        if is_calver(core) || semver::Version::parse(core).is_ok() {
            return Some(core.to_string());
        }
    }
    None
}

fn is_calver(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    parts[0].len() == 4
        && parts[0].chars().all(|c| c.is_ascii_digit())
        && !parts[1].is_empty()
        && parts[1].chars().all(|c| c.is_ascii_digit())
        && !parts[2].is_empty()
        && parts[2].chars().all(|c| c.is_ascii_digit())
}

pub fn is_newer(a: &str, b: &str) -> bool {
    compare_versions(a, b) == Ordering::Greater
}

fn compare_versions(a: &str, b: &str) -> Ordering {
    let plain = !a.contains('-') && !b.contains('-') && !is_calver(a) && !is_calver(b);
    if plain {
        if let (Ok(va), Ok(vb)) = (semver::Version::parse(a), semver::Version::parse(b)) {
            return va.cmp(&vb);
        }
    }
    let na = numeric_components(a);
    let nb = numeric_components(b);
    let len = na.len().max(nb.len());
    for i in 0..len {
        let x = na.get(i).copied().unwrap_or(0);
        let y = nb.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            Ordering::Equal => continue,
            ord => return ord,
        }
    }
    Ordering::Equal
}

fn numeric_components(s: &str) -> Vec<u64> {
    s.split(['.', '-'])
        .map(|part| {
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u64>().unwrap_or(0)
        })
        .collect()
}

pub fn read_manifest(path: &Path) -> Vec<ManifestEntry> {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Vec<ManifestEntry>>(&text) {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!(path = ?path, "unparseable versions.json: {e}");
                Vec::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            tracing::warn!(path = ?path, "cannot read versions.json: {e}");
            Vec::new()
        }
    }
}

pub fn write_manifest(path: &Path, entries: &[ManifestEntry]) -> anyhow::Result<()> {
    use std::io::Write;

    let text = serde_json::to_string_pretty(entries)?;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(text.as_bytes())?;
    tmp.write_all(b"\n")?;
    tmp.persist(path)
        .map_err(|e| anyhow::anyhow!("persist versions.json failed: {e}"))?;
    Ok(())
}

pub fn lockfile_version(entries: &[ManifestEntry], name: &str) -> Option<String> {
    entries
        .iter()
        .find(|e| e.name == name)
        .map(|e| e.version.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calver_compare() {
        assert!(is_newer("2026.10.26", "2026.10.25"));
        assert!(is_newer("2026.10.26", "2024.01.01"));
        assert!(!is_newer("2024.01.01", "2026.10.26"));
        assert!(!is_newer("2026.10.26", "2026.10.26"));
        assert!(is_newer("2026.10.1", "2026.9.30"));
        assert!(!is_newer("2026.9.30", "2026.10.1"));
    }

    #[test]
    fn semver_compare() {
        assert!(is_newer("7.1.1", "6.1.1"));
        assert!(is_newer("0.42.0", "0.37.0"));
        assert!(!is_newer("6.0.0", "7.1.1"));
        assert!(!is_newer("1.2.3", "1.2.3"));
        assert!(is_newer("2023.11.14-2", "2023.11.14"));
        assert!(!is_newer("2023.11.14", "2023.11.14-2"));
    }

    #[test]
    fn parse_version_outputs() {
        assert_eq!(
            parse_version_output("2026.10.26\n"),
            Some("2026.10.26".to_string())
        );
        assert_eq!(
            parse_version_output("ffmpeg version 7.1.1-full_build-www.gyan.dev Copyright\n"),
            Some("7.1.1".to_string())
        );
        assert_eq!(
            parse_version_output("mpv 0.42.0 Copyright\n"),
            Some("0.42.0".to_string())
        );
        assert_eq!(
            parse_version_output("mpv v0.42.0\n"),
            Some("0.42.0".to_string())
        );
        assert_eq!(parse_version_output("no version here\n"), None);
        assert_eq!(parse_version_output(""), None);
    }

    #[test]
    fn manifest_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("versions.json");
        let entries = vec![
            ManifestEntry {
                name: "yt-dlp".into(),
                version: "2026.10.26".into(),
                sha256: "abc123".into(),
                source_url: "https://example.invalid/yt-dlp".into(),
            },
            ManifestEntry {
                name: "ffmpeg".into(),
                version: "7.1.1".into(),
                sha256: "def456".into(),
                source_url: "https://example.invalid/ffmpeg".into(),
            },
        ];
        write_manifest(&path, &entries).unwrap();
        let back = read_manifest(&path);
        assert_eq!(back, entries);
        assert_eq!(lockfile_version(&back, "ffmpeg").as_deref(), Some("7.1.1"));
        assert_eq!(lockfile_version(&back, "mpv"), None);
    }

    #[test]
    fn manifest_missing_is_empty() {
        assert!(read_manifest(Path::new("/nonexistent/vidub/versions.json")).is_empty());
    }
}
