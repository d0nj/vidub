use std::path::{Path, PathBuf};

pub fn bundled_root() -> PathBuf {
    let exe = std::env::current_exe()
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let exe_dir = exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    #[cfg(target_os = "macos")]
    {
        mac_bundled_root(&exe_dir)
    }
    #[cfg(not(target_os = "macos"))]
    {
        exe_dir.join("bin")
    }
}

#[cfg(target_os = "macos")]
fn mac_bundled_root(exe_dir: &Path) -> PathBuf {
    if exe_dir.file_name().map(|n| n == "MacOS").unwrap_or(false) {
        if let Some(contents) = exe_dir.parent() {
            if contents.join("Resources").is_dir() {
                return contents.join("Resources").join("bin");
            }
        }
    }
    exe_dir.join("bin")
}

pub fn cache_root() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(base) = dirs::data_local_dir() {
            return base.join("ViDub").join("bin");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(base) = dirs::data_dir() {
            return base.join("ViDub").join("bin");
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        if let Some(base) = dirs::data_local_dir().or_else(dirs::data_dir) {
            return base.join("vidub").join("bin");
        }
    }
    std::env::temp_dir().join("vidub").join("bin")
}

pub(crate) fn root_parent_join(root: &Path, leaf: &str) -> PathBuf {
    match root.parent() {
        Some(parent) => parent.join(leaf),
        None => root.join(leaf),
    }
}

pub(crate) fn system_dirs_default() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        vec![
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]
    }
    #[cfg(target_os = "linux")]
    {
        vec![PathBuf::from("/usr/bin")]
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Vec::new()
    }
}

#[cfg(debug_assertions)]
pub(crate) fn dev_dist_dir() -> Option<PathBuf> {
    let manifest = option_env!("CARGO_MANIFEST_DIR")?;
    let dist = Path::new(manifest).join("../../dist");
    dist.is_dir().then_some(dist)
}
