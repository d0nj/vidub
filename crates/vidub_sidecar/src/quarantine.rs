#[cfg(target_os = "macos")]
pub fn strip_quarantine(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    strip_quarantine_impl(path)?;
    if let Ok(md) = std::fs::metadata(path) {
        if md.is_file() {
            let mut perms = md.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(path, perms)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn strip_quarantine_impl(path: &std::path::Path) -> std::io::Result<()> {
    if path.is_dir() {
        strip_tree(path)?;
    } else {
        ignore_not_found(xattr::remove(path, "com.apple.quarantine"))?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn strip_tree(root: &std::path::Path) -> std::io::Result<()> {
    let _ = ignore_not_found(xattr::remove(root, "com.apple.quarantine"));
    let rd = std::fs::read_dir(root)?;
    for entry in rd {
        let Ok(entry) = entry else { continue };
        let p = entry.path();
        if p.is_dir() {
            let _ = strip_tree(&p);
        } else {
            let _ = ignore_not_found(xattr::remove(&p, "com.apple.quarantine"));
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn ignore_not_found(r: std::io::Result<()>) -> std::io::Result<()> {
    match r {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn strip_quarantine(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}
