use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};

use crate::quarantine::strip_quarantine;

pub(crate) fn stage_exe_dir(
    extracted: &Path,
    exe: &str,
    bin_dir: &Path,
) -> anyhow::Result<PathBuf> {
    let exe_path = find_file_named(extracted, exe)?
        .ok_or_else(|| anyhow!("archive does not contain {exe}"))?;
    let exe_dir = exe_path.parent().unwrap_or(extracted);
    copy_tree(exe_dir, bin_dir)?;
    Ok(bin_dir.join(exe))
}

pub(crate) fn stage_ffmpeg_xz(
    extracted: &Path,
    bin_dir: &Path,
    lib_dir: &Path,
) -> anyhow::Result<()> {
    for exe in ["ffmpeg", "ffprobe"] {
        let src = find_file_named(extracted, exe)?
            .ok_or_else(|| anyhow!("ffmpeg archive does not contain {exe}"))?;
        copy_file_atomic(&src, &bin_dir.join(exe))?;
    }
    let libs = collect_lib_files(extracted)?;
    if !libs.is_empty() {
        std::fs::create_dir_all(lib_dir)?;
        for src in libs {
            if let Some(name) = src.file_name() {
                copy_file_atomic(&src, &lib_dir.join(name))?;
            }
        }
    }
    Ok(())
}

fn find_file_named(root: &Path, name: &str) -> anyhow::Result<Option<PathBuf>> {
    let mut hits = Vec::new();
    collect_matching(root, &mut hits, &|path| {
        path.file_name().is_some_and(|n| n == name)
    })?;
    hits.sort();
    Ok(hits.into_iter().next())
}

fn collect_lib_files(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut libs = Vec::new();
    collect_matching(root, &mut libs, &|path| {
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("libav") && n.contains(".so"))
    })?;
    libs.sort();
    Ok(libs)
}

fn collect_matching(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    matches: &dyn Fn(&Path) -> bool,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_matching(&path, out, matches)?;
        } else if matches(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn copy_tree(src: &Path, dest: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dest).with_context(|| format!("create {}", dest.display()))?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let p = entry.path();
        let d = dest.join(entry.file_name());
        if p.is_dir() {
            copy_tree(&p, &d)?;
        } else {
            copy_file_atomic(&p, &d)?;
        }
    }
    Ok(())
}

fn copy_file_atomic(src: &Path, dest: &Path) -> anyhow::Result<()> {
    let dir = dest
        .parent()
        .ok_or_else(|| anyhow!("staged path has no parent: {}", dest.display()))?;
    std::fs::create_dir_all(dir)?;
    let tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("temp in {}", dir.display()))?;
    std::fs::copy(src, tmp.path())
        .with_context(|| format!("copy {} -> {}", src.display(), dest.display()))?;
    tmp.persist(dest)
        .map_err(|e| anyhow!("persist {}: {}", dest.display(), e.error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))?;
    }
    if let Err(e) = strip_quarantine(dest) {
        tracing::warn!(path = %dest.display(), error = %e, "strip quarantine failed");
    }
    Ok(())
}
