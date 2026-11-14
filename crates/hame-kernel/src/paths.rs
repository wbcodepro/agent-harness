use std::path::{Path, PathBuf};

/// Removes the `\\?\` prefix Windows puts on canonical paths, when the path
/// is an ordinary drive path. Many programs, git among them, do not accept
/// the prefixed form. Other paths, and every path on other systems, are
/// returned unchanged.
pub fn simplify_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// The path with symlinks resolved, in the form other programs accept.
pub fn real_path(path: &Path) -> std::io::Result<PathBuf> {
    path.canonicalize().map(|real| simplify_path(&real))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_drive_paths_lose_the_verbatim_prefix() {
        assert_eq!(
            simplify_path(Path::new(r"\\?\C:\Users\me\work")),
            PathBuf::from(r"C:\Users\me\work")
        );
        // A verbatim network path has no plain drive form.
        let unc = Path::new(r"\\?\UNC\server\share");
        assert_eq!(simplify_path(unc), unc);
        assert_eq!(
            simplify_path(Path::new("/home/me")),
            PathBuf::from("/home/me")
        );
    }
}
