//! Locating bundled resource directories (`print-overrides/`) in a packaged
//! install, before any development fallback.
//!
//! On macOS a resource lives at `Contents/Resources/<name>`, beside the
//! `Contents/MacOS` executable; on Windows and Linux it sits next to the
//! executable as `<name>/`. The deb/rpm/pacman packages instead keep immutable
//! resources under the FHS share directory `usr/share/ebirforms`.

use std::path::{Path, PathBuf};

/// Whether a directory exists. The predicate for plain resource directories
/// such as `print-overrides/`.
pub(crate) fn directory_present(dir: &Path) -> bool {
    dir.is_dir()
}

/// Candidate locations for a packaged directory `name`, relative to the
/// directory the running executable sits in, in lookup order.
fn candidates(executable_dir: &Path, name: &str) -> Vec<PathBuf> {
    let mut candidates = vec![
        // `eBIRForms.app/Contents/MacOS/bir` -> `eBIRForms.app/Contents/Resources/<name>`.
        executable_dir.join("../Resources").join(name),
        // Windows installer/MSIX layout and the unpacked Linux tarball:
        // `<name>/` beside the executable.
        executable_dir.join(name),
    ];
    // deb/rpm/pacman installs: `usr/bin/ebirforms` reads
    // `usr/share/ebirforms/<name>`.
    // `cfg!` keeps `mut` used on every platform.
    if cfg!(target_os = "linux") {
        candidates.push(PathBuf::from("/usr/share/ebirforms").join(name));
    }
    candidates
}

/// The first packaged location for `name` that `present` accepts, relative to
/// `executable` (the binary itself, whose parent directory is searched).
/// `None` when there is no executable path or no candidate qualifies.
pub(crate) fn dir(
    name: &str,
    executable: Option<&Path>,
    present: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let executable = executable?;
    let executable = executable
        .canonicalize()
        .unwrap_or_else(|_| executable.into());
    let executable_dir = executable.parent()?;
    candidates(executable_dir, name)
        .into_iter()
        .find(|candidate| present(candidate))
        // Resolve the `..` in the bundle candidate to a canonical answer.
        .map(|candidate| candidate.canonicalize().unwrap_or(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "bir-print-packaged-{}-{}",
                std::process::id(),
                name
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).expect("temp tree");
            // `dir` canonicalizes the executable path, so compare against the
            // resolved root (/var -> /private/var on macOS).
            let root = root.canonicalize().unwrap_or(root);
            Self(root)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn beside_the_executable_resolves_for_windows_and_tarball_layouts() {
        let tree = Tree::new("beside-exe");
        let bin = tree.0.join("install-dir");
        let overrides = bin.join("print-overrides");
        fs::create_dir_all(&overrides).expect("overrides dir");

        let exe = bin.join("bir");
        fs::write(&exe, "").expect("exe");
        assert_eq!(
            dir("print-overrides", Some(&exe), directory_present),
            Some(overrides)
        );
    }

    #[test]
    fn bundle_location_wins_over_the_beside_candidate() {
        let tree = Tree::new("bundle-first");
        let macos = tree.0.join("Contents/MacOS");
        let bundled = tree.0.join("Contents/Resources/print-overrides");
        let beside = macos.join("print-overrides");
        fs::create_dir_all(&bundled).expect("bundled overrides");
        fs::create_dir_all(&beside).expect("beside overrides");

        let exe = macos.join("bir");
        fs::write(&exe, "").expect("exe");
        assert_eq!(
            dir("print-overrides", Some(&exe), directory_present),
            Some(bundled)
        );
    }

    #[test]
    fn print_overrides_resolve_in_the_same_locations() {
        let tree = Tree::new("overrides");
        let macos = tree.0.join("eBIRForms.app/Contents/MacOS");
        let overrides = tree
            .0
            .join("eBIRForms.app/Contents/Resources/print-overrides");
        fs::create_dir_all(&macos).expect("MacOS dir");
        fs::create_dir_all(&overrides).expect("overrides dir");

        let exe = macos.join("bir");
        fs::write(&exe, "").expect("exe");
        assert_eq!(
            dir("print-overrides", Some(&exe), directory_present),
            Some(overrides)
        );
    }

    #[test]
    fn nothing_packaged_and_no_executable_means_none() {
        let tree = Tree::new("empty");
        let bin = tree.0.join("install-dir");
        fs::create_dir_all(&bin).expect("bin dir");
        let exe = bin.join("bir");
        fs::write(&exe, "").expect("exe");
        assert_eq!(dir("print-overrides", Some(&exe), directory_present), None);
        assert_eq!(dir("print-overrides", None, directory_present), None);
    }
}
