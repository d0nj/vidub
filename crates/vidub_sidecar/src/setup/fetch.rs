use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::downloader::download_and_install;

use super::pins::Pin;

pub(crate) async fn download_archive(
    client: &reqwest::Client,
    pin: &Pin,
    workdir: &Path,
    cache: &mut HashMap<String, PathBuf>,
) -> anyhow::Result<PathBuf> {
    if let Some(p) = cache.get(pin.sha256) {
        return Ok(p.clone());
    }
    let leaf = pin
        .url
        .rsplit('/')
        .next()
        .unwrap_or("archive")
        .split('?')
        .next()
        .unwrap_or("archive");
    let archive = workdir.join(leaf);
    download_and_install(client, pin.url, &archive, Some(pin.sha256)).await?;
    cache.insert(pin.sha256.to_string(), archive.clone());
    Ok(archive)
}

pub(crate) fn extract_zip(path: &Path, dest: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dest).with_context(|| format!("create {}", dest.display()))?;
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).with_context(|| format!("open zip {}", path.display()))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out = dest.join(&rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut f)?;
    }
    Ok(())
}

pub(crate) fn extract_tar_gz(path: &Path, dest: &Path) -> anyhow::Result<()> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    extract_tar(flate2::read::GzDecoder::new(file), dest, path)
}

pub(crate) fn extract_tar_xz(path: &Path, dest: &Path) -> anyhow::Result<()> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    extract_tar(xz2::read::XzDecoder::new(file), dest, path)
}

fn extract_tar<R: std::io::Read>(archive: R, dest: &Path, path: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dest).with_context(|| format!("create {}", dest.display()))?;
    tar::Archive::new(archive)
        .unpack(dest)
        .with_context(|| format!("extract {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::install::stage_exe_dir;
    use std::io::Write as _;

    #[test]
    fn zip_exe_extract_and_stage() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("a.zip");
        {
            let f = std::fs::File::create(&archive).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("ffmpeg-N/bin/ffmpeg.exe", opts).unwrap();
            zw.write_all(b"fake-ffmpeg").unwrap();
            zw.start_file("ffmpeg-N/bin/avformat-62.dll", opts).unwrap();
            zw.write_all(b"fake-dll").unwrap();
            zw.finish().unwrap();
        }
        let ext = tmp.path().join("ext");
        extract_zip(&archive, &ext).unwrap();
        let bin = tmp.path().join("bin");
        let staged = stage_exe_dir(&ext, "ffmpeg.exe", &bin).unwrap();
        assert_eq!(staged, bin.join("ffmpeg.exe"));
        assert_eq!(std::fs::read(&staged).unwrap(), b"fake-ffmpeg");
        assert!(
            bin.join("avformat-62.dll").is_file(),
            "DLLs staged wholesale"
        );
    }

    fn tar_append<B: std::io::Write>(
        builder: &mut tar::Builder<B>,
        path: &str,
        data: &[u8],
    ) -> std::io::Result<()> {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        builder.append_data(&mut header, path, data)
    }

    #[test]
    fn tar_gz_exe_extract_and_stage() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("piper.tar.gz");
        {
            let f = std::fs::File::create(&archive).unwrap();
            let gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
            let mut tb = tar::Builder::new(gz);
            tar_append(&mut tb, "piper", b"fake-piper").unwrap();
            tar_append(&mut tb, "libpiper/data.txt", b"fake-data").unwrap();
            tb.finish().unwrap();
            let gz = tb.into_inner().unwrap();
            gz.finish().unwrap();
        }
        let ext = tmp.path().join("ext");
        extract_tar_gz(&archive, &ext).unwrap();
        let bin = tmp.path().join("bin");
        let staged = stage_exe_dir(&ext, "piper", &bin).unwrap();
        assert_eq!(staged, bin.join("piper"));
        assert_eq!(std::fs::read(&staged).unwrap(), b"fake-piper");
        assert!(
            bin.join("libpiper").join("data.txt").is_file(),
            "data staged wholesale"
        );
    }

    #[test]
    fn tar_xz_ffmpeg_extract_and_stage() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("ffmpeg.tar.xz");
        {
            let f = std::fs::File::create(&archive).unwrap();
            let xz = xz2::write::XzEncoder::new(f, 6);
            let mut tb = tar::Builder::new(xz);
            tar_append(&mut tb, "ffmpeg-N/bin/ffmpeg", b"fake-ffmpeg").unwrap();
            tar_append(&mut tb, "ffmpeg-N/bin/ffprobe", b"fake-ffprobe").unwrap();
            tar_append(&mut tb, "ffmpeg-N/lib/libavcodec.so.61", b"fake-so").unwrap();
            tar_append(&mut tb, "ffmpeg-N/lib/libunrelated.a", b"not a libav so").unwrap();
            tb.finish().unwrap();
            let xz = tb.into_inner().unwrap();
            xz.finish().unwrap();
        }
        let ext = tmp.path().join("ext");
        extract_tar_xz(&archive, &ext).unwrap();
        let bin = tmp.path().join("bin");
        let lib = tmp.path().join("lib");
        crate::setup::install::stage_ffmpeg_xz(&ext, &bin, &lib).unwrap();
        assert_eq!(std::fs::read(bin.join("ffmpeg")).unwrap(), b"fake-ffmpeg");
        assert_eq!(std::fs::read(bin.join("ffprobe")).unwrap(), b"fake-ffprobe");
        assert_eq!(
            std::fs::read(lib.join("libavcodec.so.61")).unwrap(),
            b"fake-so"
        );
        assert!(
            !lib.join("libunrelated.a").exists(),
            "non-libav files not staged"
        );
    }
}
