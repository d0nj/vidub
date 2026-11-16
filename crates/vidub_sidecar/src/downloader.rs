//! Sidecar downloader: streamed reqwest download → `tempfile` → SHA-256
//! check → atomic persist → `chmod 0755` on unix.
//!
//! Used by the in-app setup flow ([`crate::setup::setup_missing`]) to install
//! missing sidecars into the user cache; `ensure_all` never downloads.

use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Errors from the downloader.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("http download failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("sha256 mismatch: expected {expected}, got {actual}")]
    Checksum { expected: String, actual: String },
}

/// Stream `url` into a temp file in `dir`, returning the temp handle and its
/// SHA-256 hex digest.
pub async fn download_to_temp(
    client: &reqwest::Client,
    url: &str,
    dir: &Path,
) -> Result<(tempfile::NamedTempFile, String), DownloadError> {
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    let mut hasher = Sha256::new();

    let mut resp = client.get(url).send().await?;
    resp = resp.error_for_status()?;
    while let Some(chunk) = resp.chunk().await? {
        // Feed the hasher first; chunk lengths are small (network buffer).
        hasher.update(&chunk);
        tmp.write_all(&chunk)?;
    }
    tmp.flush()?;
    let digest = {
        let out = hasher.finalize();
        let mut s = String::with_capacity(out.len() * 2);
        for b in out.iter() {
            s.push_str(&format!("{b:02x}"));
        }
        s
    };
    Ok((tmp, digest))
}

/// Download `url`, verify `expected_sha256` (if given), and atomically persist
/// to `dest` with `0755` permissions on unix.
pub async fn download_and_install(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
) -> Result<PathBuf, DownloadError> {
    let dir = dest.parent().unwrap_or_else(|| Path::new("."));
    let (tmp, digest) = download_to_temp(client, url, dir).await?;
    if let Some(expected) = expected_sha256 {
        if !expected.eq_ignore_ascii_case(&digest) {
            return Err(DownloadError::Checksum {
                expected: expected.to_string(),
                actual: digest,
            });
        }
    }
    let dest_buf = dest.to_path_buf();
    // persist() atomically replaces dest on the same filesystem.
    tmp.persist(&dest_buf)
        .map_err(|e| DownloadError::Io(e.error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&dest_buf, perms)?;
    }
    // De-quarantine on macOS (no-op elsewhere).
    let _ = crate::quarantine::strip_quarantine(&dest_buf);
    Ok(dest_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vector() {
        // Downloader unit check without network: hash a fixed payload.
        let mut hasher = Sha256::new();
        hasher.update(b"hello");
        let out = hasher.finalize();
        let hex: String = out.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn checksum_mismatch_error() {
        let e = DownloadError::Checksum {
            expected: "a".into(),
            actual: "b".into(),
        };
        assert!(e.to_string().contains("sha256 mismatch"));
    }
}
