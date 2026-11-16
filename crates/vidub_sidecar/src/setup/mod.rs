mod fetch;
mod install;
mod pins;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::anyhow;

use crate::downloader::download_and_install;
use crate::locator::{ensure_all, SearchRoots, SidecarSource};

use fetch::{download_archive, extract_tar_gz, extract_tar_xz, extract_zip};
use install::{stage_exe_dir, stage_ffmpeg_xz};
use pins::{guidance_for, pins_for, Pin, PinKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupEvent {
    Checking(String),
    Downloading(String),
    Ready(String),
    Failed { name: String, msg: String },
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SetupReport {
    pub installed: Vec<String>,
    pub failed: Vec<String>,
}

pub async fn setup_missing(progress: &(dyn Fn(SetupEvent) + Send + Sync)) -> SetupReport {
    let mut report = SetupReport::default();
    let (_bins, statuses) = ensure_all().await;

    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let roots = SearchRoots::live();
    let pin_set = pins_for(os, arch);
    let pinned: HashMap<&'static str, &Pin> = pin_set.iter().map(|p| (p.name, p)).collect();
    let guided: HashMap<&'static str, &'static str> = guidance_for(os).into_iter().collect();

    enum Job {
        Install(Pin),
        Guidance(&'static str),
        Unsupported,
    }
    let mut jobs: Vec<(&'static str, Job)> = Vec::new();
    for s in &statuses {
        if s.source != SidecarSource::Missing {
            continue;
        }
        let pin_name = if s.name == "voice_onnx" {
            "voice-onnx"
        } else {
            s.name
        };
        let (display, job) = if let Some(pin) = pinned.get(pin_name) {
            (pin.name, Job::Install(**pin))
        } else if let Some(g) = guided.get(s.name) {
            (s.name, Job::Guidance(g))
        } else {
            (s.name, Job::Unsupported)
        };
        jobs.push((display, job));
    }
    let config_present = roots.cache_models.join("config.json").is_file()
        || roots.bundled_models.join("config.json").is_file();
    if !config_present {
        let job = pinned
            .get("voice-config")
            .map(|pin| Job::Install(**pin))
            .unwrap_or(Job::Unsupported);
        jobs.push(("voice-config", job));
    }

    let wants_install = jobs.iter().any(|(_, j)| matches!(j, Job::Install(_)));
    let client = if wants_install {
        Some(reqwest::Client::new())
    } else {
        None
    };
    let cache_base = roots
        .cache
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| roots.cache.clone());
    if let Err(e) = std::fs::create_dir_all(&cache_base) {
        tracing::warn!(error = %e, "setup: create cache base failed");
    }
    let workdir = tempfile::tempdir_in(&cache_base);
    let mut archive_cache: HashMap<String, PathBuf> = HashMap::new();

    for (name, job) in jobs {
        match job {
            Job::Install(pin) => {
                progress(SetupEvent::Checking(name.to_string()));
                progress(SetupEvent::Downloading(name.to_string()));
                let result = match (client.as_ref(), workdir.as_ref()) {
                    (Some(client), Ok(wd)) => {
                        install_pin(client, &pin, &roots, wd.path(), &mut archive_cache).await
                    }
                    (_, Err(e)) => Err(anyhow!("Không tạo được thư mục tạm: {e}")),
                    _ => Err(anyhow!("Không tạo được thư mục tạm")),
                };
                match result {
                    Ok(()) => {
                        progress(SetupEvent::Ready(name.to_string()));
                        report.installed.push(name.to_string());
                    }
                    Err(e) => {
                        tracing::warn!(sidecar = name, error = %e, "setup install failed");
                        progress(SetupEvent::Failed {
                            name: name.to_string(),
                            msg: format!("Không thể cài đặt {name}: {e}"),
                        });
                        report.failed.push(name.to_string());
                    }
                }
            }
            Job::Guidance(g) => {
                progress(SetupEvent::Checking(name.to_string()));
                progress(SetupEvent::Failed {
                    name: name.to_string(),
                    msg: g.to_string(),
                });
                report.failed.push(name.to_string());
            }
            Job::Unsupported => {
                progress(SetupEvent::Checking(name.to_string()));
                progress(SetupEvent::Failed {
                    name: name.to_string(),
                    msg: format!("Nền tảng {os}/{arch} chưa được hỗ trợ: chưa có bản sidecar đóng gói cho hệ điều hành này."),
                });
                report.failed.push(name.to_string());
            }
        }
    }
    report
}

async fn install_pin(
    client: &reqwest::Client,
    pin: &Pin,
    roots: &SearchRoots,
    workdir: &Path,
    archive_cache: &mut HashMap<String, PathBuf>,
) -> anyhow::Result<()> {
    match pin.kind {
        PinKind::SingleFile => {
            download_and_install(
                client,
                pin.url,
                &dest_path(roots, pin.dest),
                Some(pin.sha256),
            )
            .await?;
            Ok(())
        }
        PinKind::ZipExe { exe } => {
            let archive = download_archive(client, pin, workdir, archive_cache).await?;
            let ext_dir = extracted_dir_for(pin, workdir);
            extract_if_needed(&ext_dir, || extract_zip(&archive, &ext_dir))?;
            stage_exe_dir(&ext_dir, exe, &roots.cache)?;
            Ok(())
        }
        PinKind::TarGzExe { exe } => {
            let archive = download_archive(client, pin, workdir, archive_cache).await?;
            let ext_dir = extracted_dir_for(pin, workdir);
            extract_if_needed(&ext_dir, || extract_tar_gz(&archive, &ext_dir))?;
            stage_exe_dir(&ext_dir, exe, &roots.cache)?;
            Ok(())
        }
        PinKind::TarXzFfmpeg => {
            let archive = download_archive(client, pin, workdir, archive_cache).await?;
            let ext_dir = extracted_dir_for(pin, workdir);
            extract_if_needed(&ext_dir, || extract_tar_xz(&archive, &ext_dir))?;
            let lib_dir = roots
                .cache
                .parent()
                .map(|p| p.join("lib"))
                .unwrap_or_else(|| roots.cache.join("lib"));
            stage_ffmpeg_xz(&ext_dir, &roots.cache, &lib_dir)?;
            Ok(())
        }
    }
}

fn extract_if_needed(
    ext_dir: &Path,
    extract: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if ext_dir.exists() {
        return Ok(());
    }
    let result = extract();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(ext_dir);
    }
    result
}

fn dest_path(roots: &SearchRoots, dest: &str) -> PathBuf {
    if let Some(rest) = dest.strip_prefix("bin/") {
        roots.cache.join(rest)
    } else if let Some(rest) = dest.strip_prefix("models/") {
        roots.cache_models.join(rest)
    } else {
        roots.cache.join(dest)
    }
}

fn extracted_dir_for(pin: &Pin, workdir: &Path) -> PathBuf {
    let short = &pin.sha256[..pin.sha256.len().min(16)];
    workdir.join(format!("ext-{short}"))
}
