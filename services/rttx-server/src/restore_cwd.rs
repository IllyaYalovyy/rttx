//! Where a restored pane's shell starts when its saved directory is unusable.
//!
//! Only reads the filesystem: the saved directory is never created, changed or
//! removed, and a shell or PTY that fails to start is the caller's to report.

use std::io::ErrorKind;
use std::path::Path;

/// A replacement working directory and the pane text explaining it.
#[derive(Debug, PartialEq, Eq)]
pub struct Fallback {
    /// Directory the restored shell starts in instead of the saved one.
    pub cwd: String,
    /// Terminal output naming the saved path and why it was not used.
    pub notice: String,
}

/// Decide whether a restored pane has to leave its saved directory.
///
/// Returns `None` when the shell can start in `saved`. Otherwise the shell
/// belongs in `home`, or in `/` if home cannot be entered either.
#[must_use]
pub fn fallback_for(saved: &str, home: Option<&Path>) -> Option<Fallback> {
    let reason = why_unenterable(&crate::pty::resolve_cwd(Path::new(saved)))?;
    let usable_home = home
        .filter(|home| home.is_absolute() && why_unenterable(home).is_none())
        .and_then(Path::to_str);
    let (cwd, destination) = usable_home.map_or_else(
        || ("/", "/ because the home directory is unavailable".to_string()),
        |home| (home, format!("your home directory {}", printable(home))),
    );
    Some(Fallback {
        cwd: cwd.to_string(),
        notice: format!(
            "\x1b[33m[rttx] Saved directory {} is unavailable: {reason}. \
             This shell started in {destination}.\x1b[0m\r\n",
            printable(saved),
        ),
    })
}

/// Why a process could not make `dir` its working directory, if it could not.
fn why_unenterable(dir: &Path) -> Option<String> {
    // Looking up "." inside a directory needs the same search permission as
    // changing into it, which stat of the directory itself does not.
    let error = std::fs::metadata(dir.join(".")).err()?;
    Some(match error.kind() {
        ErrorKind::NotFound => "it does not exist".to_string(),
        ErrorKind::NotADirectory => "it is not a directory".to_string(),
        ErrorKind::PermissionDenied => "permission denied".to_string(),
        _ => error.to_string(),
    })
}

/// A path as terminal text: a file name must not be able to send escapes.
fn printable(path: &str) -> String {
    path.replace(char::is_control, "\u{fffd}")
}

#[cfg(test)]
mod tests {
    // Permission denial is covered by tests/inaccessible_cwd_restart.rs, which
    // runs the daemon unprivileged even when the test runner is root.
    use super::*;

    #[test]
    fn enterable_directory_needs_no_fallback() {
        let dir = tempfile::TempDir::new().unwrap();

        assert_eq!(fallback_for(dir.path().to_str().unwrap(), None), None);
    }

    #[test]
    fn missing_directory_falls_back_to_home_without_being_created() {
        let tmp = tempfile::TempDir::new().unwrap();
        let saved = tmp.path().join("gone");

        let fallback = fallback_for(saved.to_str().unwrap(), Some(tmp.path())).unwrap();

        assert_eq!(fallback.cwd, tmp.path().to_str().unwrap());
        assert!(fallback.notice.contains(saved.to_str().unwrap()), "{}", fallback.notice);
        assert!(fallback.notice.contains("it does not exist"), "{}", fallback.notice);
        assert!(fallback.notice.contains("your home directory"), "{}", fallback.notice);
        assert!(!saved.exists());
    }

    #[test]
    fn file_in_place_of_directory_falls_back_to_home() {
        let tmp = tempfile::TempDir::new().unwrap();
        let saved = tmp.path().join("file");
        std::fs::write(&saved, "kept").unwrap();

        let fallback = fallback_for(saved.to_str().unwrap(), Some(tmp.path())).unwrap();

        assert_eq!(fallback.cwd, tmp.path().to_str().unwrap());
        assert!(fallback.notice.contains("it is not a directory"), "{}", fallback.notice);
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), "kept");
    }

    #[test]
    fn unavailable_home_falls_back_to_root_and_says_so() {
        let tmp = tempfile::TempDir::new().unwrap();
        let gone = tmp.path().join("gone");

        for home in [None, Some(gone.as_path()), Some(Path::new(""))] {
            let fallback = fallback_for(gone.to_str().unwrap(), home).unwrap();

            assert_eq!(fallback.cwd, "/", "home: {home:?}");
            assert!(
                fallback.notice.contains("the home directory is unavailable"),
                "{}",
                fallback.notice
            );
        }
    }

    #[test]
    fn notice_does_not_replay_control_characters_from_the_path() {
        let fallback = fallback_for("/nonexistent/\x1b]0;owned\x07", None).unwrap();

        assert!(!fallback.notice.contains("\x1b]0;"), "{:?}", fallback.notice);
        assert!(!fallback.notice.contains('\x07'), "{:?}", fallback.notice);
    }
}
