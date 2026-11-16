mod paths;
mod probe;

use std::path::{Path, PathBuf};

use crate::versions;

pub use paths::{bundled_root, cache_root};

use paths::{root_parent_join, system_dirs_default};
use probe::{find_in_dir, probe_piper, probe_version};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidecarSource {
    Bundled,
    Cache,
    System,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarStatus {
    pub name: &'static str,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub source: SidecarSource,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BinPaths {
    pub yt_dlp: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
    pub mpv: Option<PathBuf>,
    pub piper: Option<PathBuf>,
    pub qjs: Option<PathBuf>,
    pub voice_onnx: Option<PathBuf>,
}

pub const VOICE_FILE_NAME: &str = "ngochuyennew.onnx";

type BinSlot = fn(&mut BinPaths) -> &mut Option<PathBuf>;

const BINARIES: [(&str, BinSlot); 6] = [
    ("yt-dlp", |bins| &mut bins.yt_dlp),
    ("ffmpeg", |bins| &mut bins.ffmpeg),
    ("ffprobe", |bins| &mut bins.ffprobe),
    ("mpv", |bins| &mut bins.mpv),
    ("piper", |bins| &mut bins.piper),
    ("qjs", |bins| &mut bins.qjs),
];

#[derive(Debug, Clone)]
pub struct SearchRoots {
    pub bundled: PathBuf,
    pub bundled_models: PathBuf,
    pub cache: PathBuf,
    pub cache_models: PathBuf,
    pub system_dirs: Vec<PathBuf>,
}

impl SearchRoots {
    pub fn live() -> SearchRoots {
        let cache = cache_root();
        let cache_models = root_parent_join(&cache, "models");
        #[cfg(debug_assertions)]
        if let Some(dist) = paths::dev_dist_dir() {
            return SearchRoots {
                bundled: dist.join("bin"),
                bundled_models: dist.join("models"),
                cache,
                cache_models,
                system_dirs: system_dirs_default(),
            };
        }
        let bundled = bundled_root();
        let bundled_models = root_parent_join(&bundled, "models");
        SearchRoots {
            bundled,
            bundled_models,
            cache,
            cache_models,
            system_dirs: system_dirs_default(),
        }
    }
}

pub(crate) fn locate_in_dirs(name: &str, roots: &SearchRoots) -> (Option<PathBuf>, SidecarSource) {
    if let Some(p) = find_in_dir(&roots.bundled, name) {
        return (Some(p), SidecarSource::Bundled);
    }
    if let Some(p) = find_in_dir(&roots.cache, name) {
        return (Some(p), SidecarSource::Cache);
    }
    for dir in &roots.system_dirs {
        if let Some(p) = find_in_dir(dir, name) {
            return (Some(p), SidecarSource::System);
        }
    }
    (None, SidecarSource::Missing)
}

pub(crate) fn locate_with_path(
    name: &str,
    roots: &SearchRoots,
) -> (Option<PathBuf>, SidecarSource) {
    let (path, source) = locate_in_dirs(name, roots);
    if path.is_some() {
        return (path, source);
    }
    match which::which(name) {
        Ok(p) => (Some(p), SidecarSource::System),
        Err(_) => (None, SidecarSource::Missing),
    }
}

pub(crate) fn locate_voice_in_roots(roots: &SearchRoots) -> (Option<PathBuf>, SidecarSource) {
    let bundled_cand = roots.bundled_models.join(VOICE_FILE_NAME);
    if bundled_cand.is_file() {
        return (Some(bundled_cand), SidecarSource::Bundled);
    }
    let cache_cand = roots.cache_models.join(VOICE_FILE_NAME);
    if cache_cand.is_file() {
        return (Some(cache_cand), SidecarSource::Cache);
    }
    for dir in &roots.system_dirs {
        let cand = dir.join(VOICE_FILE_NAME);
        if cand.is_file() {
            return (Some(cand), SidecarSource::System);
        }
    }
    (None, SidecarSource::Missing)
}

fn manifest_path_for_root(root: &Path) -> PathBuf {
    match root.parent() {
        Some(parent) => parent.join("versions.json"),
        None => root.join("versions.json"),
    }
}

fn manifest_version(
    name: &str,
    bundled_manifest: &[versions::ManifestEntry],
    cache_manifest: &[versions::ManifestEntry],
) -> Option<String> {
    versions::lockfile_version(cache_manifest, name)
        .or_else(|| versions::lockfile_version(bundled_manifest, name))
}

pub async fn ensure_all() -> (BinPaths, Vec<SidecarStatus>) {
    let roots = SearchRoots::live();
    let bundled_manifest = versions::read_manifest(&manifest_path_for_root(&roots.bundled));
    let cache_manifest = versions::read_manifest(&manifest_path_for_root(&roots.cache));

    let mut bins = BinPaths::default();
    let mut statuses: Vec<SidecarStatus> = Vec::with_capacity(BINARIES.len() + 1);

    for (name, slot) in BINARIES {
        let (path, source) = locate_with_path(name, &roots);
        let version = match &path {
            Some(p) if name == "piper" => {
                let tag = manifest_version("piper", &bundled_manifest, &cache_manifest);
                probe_piper(p, tag).await
            }
            Some(p) => probe_version(p).await,
            None => None,
        };
        *slot(&mut bins) = path.clone();
        statuses.push(SidecarStatus {
            name,
            path,
            version,
            source,
        });
    }

    let (voice_path, voice_source) = locate_voice_in_roots(&roots);
    bins.voice_onnx = voice_path.clone();
    statuses.push(SidecarStatus {
        name: "voice_onnx",
        path: voice_path,
        version: None,
        source: voice_source,
    });

    for s in &statuses {
        tracing::info!(
            sidecar = s.name,
            source = ?s.source,
            path = ?s.path,
            version = ?s.version,
            "sidecar probe"
        );
    }
    (bins, statuses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locator::probe::exe_file_names;

    fn test_roots(bundled: &Path, cache: &Path, system: &Path) -> SearchRoots {
        SearchRoots {
            bundled: bundled.to_path_buf(),
            bundled_models: bundled.join("models"),
            cache: cache.to_path_buf(),
            cache_models: cache.join("models"),
            system_dirs: vec![system.to_path_buf()],
        }
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"fake").unwrap();
    }

    #[test]
    fn locator_order_bundled_wins() {
        let b = tempfile::tempdir().unwrap();
        let c = tempfile::tempdir().unwrap();
        let s = tempfile::tempdir().unwrap();
        let fname = exe_file_names("ffmpeg")[0].clone();
        touch(&b.path().join(&fname));
        touch(&c.path().join(&fname));
        touch(&s.path().join(&fname));
        let roots = test_roots(b.path(), c.path(), s.path());
        let (path, source) = locate_in_dirs("ffmpeg", &roots);
        assert_eq!(source, SidecarSource::Bundled);
        assert_eq!(path, Some(b.path().join(&fname)));
    }

    #[test]
    fn locator_order_cache_beats_system() {
        let b = tempfile::tempdir().unwrap();
        let c = tempfile::tempdir().unwrap();
        let s = tempfile::tempdir().unwrap();
        let fname = exe_file_names("ffprobe")[0].clone();
        touch(&c.path().join(&fname));
        touch(&s.path().join(&fname));
        let roots = test_roots(b.path(), c.path(), s.path());
        let (path, source) = locate_in_dirs("ffprobe", &roots);
        assert_eq!(source, SidecarSource::Cache);
        assert_eq!(path, Some(c.path().join(&fname)));
    }

    #[test]
    fn locator_order_system_last() {
        let b = tempfile::tempdir().unwrap();
        let c = tempfile::tempdir().unwrap();
        let s = tempfile::tempdir().unwrap();
        let fname = exe_file_names("yt-dlp")[0].clone();
        touch(&s.path().join(&fname));
        let roots = test_roots(b.path(), c.path(), s.path());
        let (path, source) = locate_in_dirs("yt-dlp", &roots);
        assert_eq!(source, SidecarSource::System);
        assert_eq!(path, Some(s.path().join(&fname)));
    }

    #[test]
    fn locator_order_qjs_bundled_cache_system() {
        let b = tempfile::tempdir().unwrap();
        let c = tempfile::tempdir().unwrap();
        let s = tempfile::tempdir().unwrap();
        let fname = exe_file_names("qjs")[0].clone();
        touch(&b.path().join(&fname));
        touch(&c.path().join(&fname));
        touch(&s.path().join(&fname));
        let roots = test_roots(b.path(), c.path(), s.path());
        let (path, source) = locate_in_dirs("qjs", &roots);
        assert_eq!(source, SidecarSource::Bundled);
        assert_eq!(path, Some(b.path().join(&fname)));

        let empty = tempfile::tempdir().unwrap();
        let roots2 = test_roots(empty.path(), c.path(), s.path());
        let (path2, source2) = locate_in_dirs("qjs", &roots2);
        assert_eq!(source2, SidecarSource::Cache);
        assert_eq!(path2, Some(c.path().join(&fname)));

        let roots3 = test_roots(empty.path(), empty.path(), s.path());
        let (path3, source3) = locate_in_dirs("qjs", &roots3);
        assert_eq!(source3, SidecarSource::System);
        assert_eq!(path3, Some(s.path().join(&fname)));
    }

    #[test]
    fn locator_missing() {
        let b = tempfile::tempdir().unwrap();
        let c = tempfile::tempdir().unwrap();
        let s = tempfile::tempdir().unwrap();
        let roots = test_roots(b.path(), c.path(), s.path());
        let (path, source) = locate_in_dirs("piper", &roots);
        assert_eq!(path, None);
        assert_eq!(source, SidecarSource::Missing);
    }

    #[test]
    fn mpv_app_descent() {
        let b = tempfile::tempdir().unwrap();
        let c = tempfile::tempdir().unwrap();
        let s = tempfile::tempdir().unwrap();
        let fname = exe_file_names("mpv")[0].clone();
        let inner = b
            .path()
            .join("mpv.app")
            .join("Contents")
            .join("MacOS")
            .join(&fname);
        touch(&inner);
        let roots = test_roots(b.path(), c.path(), s.path());
        let (path, source) = locate_in_dirs("mpv", &roots);
        assert_eq!(source, SidecarSource::Bundled);
        assert_eq!(path, Some(inner));
    }

    #[test]
    fn voice_models_order() {
        let b = tempfile::tempdir().unwrap();
        let c = tempfile::tempdir().unwrap();
        let s = tempfile::tempdir().unwrap();
        touch(&b.path().join("models").join(VOICE_FILE_NAME));
        touch(&c.path().join("models").join(VOICE_FILE_NAME));
        let roots = test_roots(b.path(), c.path(), s.path());
        let (path, source) = locate_voice_in_roots(&roots);
        assert_eq!(source, SidecarSource::Bundled);
        assert_eq!(path, Some(b.path().join("models").join(VOICE_FILE_NAME)));

        let tmp2 = tempfile::tempdir().unwrap();
        touch(&tmp2.path().join("models").join(VOICE_FILE_NAME));
        let empty = tempfile::tempdir().unwrap();
        let roots2 = test_roots(empty.path(), tmp2.path(), s.path());
        let (path2, source2) = locate_voice_in_roots(&roots2);
        assert_eq!(source2, SidecarSource::Cache);
        assert_eq!(
            path2,
            Some(tmp2.path().join("models").join(VOICE_FILE_NAME))
        );
    }

    #[test]
    fn voice_missing() {
        let b = tempfile::tempdir().unwrap();
        let c = tempfile::tempdir().unwrap();
        let s = tempfile::tempdir().unwrap();
        let roots = test_roots(b.path(), c.path(), s.path());
        let (path, source) = locate_voice_in_roots(&roots);
        assert_eq!(path, None);
        assert_eq!(source, SidecarSource::Missing);
    }

    #[test]
    fn unknown_binary_is_missing() {
        let roots = SearchRoots::live();
        let (path, source) = locate_with_path("vidub-nonexistent-binary-xyz", &roots);
        assert_eq!(path, None);
        assert_eq!(source, SidecarSource::Missing);
    }

    #[test]
    fn roots_end_with_bin() {
        assert_eq!(
            bundled_root().file_name().and_then(|n| n.to_str()),
            Some("bin")
        );
        assert_eq!(
            cache_root().file_name().and_then(|n| n.to_str()),
            Some("bin")
        );
    }
}
