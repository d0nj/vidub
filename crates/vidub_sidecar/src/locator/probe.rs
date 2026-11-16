use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use vidub_process::hide_console;

use crate::versions;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn exe_file_names(name: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        vec![format!("{name}.exe"), name.to_string()]
    }
    #[cfg(not(windows))]
    {
        vec![name.to_string()]
    }
}

pub(super) fn find_in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    for file_name in exe_file_names(name) {
        let cand = dir.join(&file_name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    if name == "mpv" {
        let macos_bin = dir.join("mpv.app").join("Contents").join("MacOS");
        for file_name in exe_file_names(name) {
            let cand = macos_bin.join(&file_name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

pub(super) async fn run_probe(path: &Path, arg: &str) -> Option<(bool, Vec<u8>, Vec<u8>)> {
    let mut cmd = tokio::process::Command::new(path);
    cmd.arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    hide_console(&mut cmd);
    let output = tokio::time::timeout(PROBE_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    Some((output.status.success(), output.stdout, output.stderr))
}

pub(super) async fn probe_version(path: &Path) -> Option<String> {
    let (_ok, stdout, stderr) = run_probe(path, "--version").await?;
    let out_text = String::from_utf8_lossy(&stdout);
    if let Some(v) = versions::parse_version_output(&out_text) {
        return Some(v);
    }
    let err_text = String::from_utf8_lossy(&stderr);
    versions::parse_version_output(&err_text)
}

pub(super) async fn probe_piper(path: &Path, lockfile_tag: Option<String>) -> Option<String> {
    let (ok, _stdout, _stderr) = run_probe(path, "--help").await?;
    if ok {
        lockfile_tag
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_names_windows_prefers_exe() {
        let names = exe_file_names("ffmpeg");
        #[cfg(windows)]
        assert_eq!(names, ["ffmpeg.exe", "ffmpeg"]);
        #[cfg(not(windows))]
        assert_eq!(names, ["ffmpeg"]);
    }
}
