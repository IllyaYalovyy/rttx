//! Atomic file I/O with symlink-based `.bak` backup (RFC-022 §6).
//!
//! Write protocol:
//! 1. Write content to `<path>.tmp`
//! 2. If `<path>` exists, copy it to `<path>.prev`
//! 3. Update `<path>.bak` symlink to point to `<path>.prev`
//! 4. Rename `<path>.tmp` → `<path>`
//!
//! On crash at any point, at least one of `<path>` or `<path>.prev`
//! (reachable via `.bak`) contains a valid copy.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Write `content` to `path` with symlink-based backup.
///
/// Creates parent directories as needed.
pub fn write_with_backup(path: &Path, content: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp_path = path.with_extension("tmp");
    let prev_path = path.with_extension("prev");
    let bak_path = path.with_extension("bak");

    // 1. Write to tmp
    fs::write(&tmp_path, content)?;

    // 2. If the live file exists, copy to .prev
    if path.exists() {
        fs::copy(path, &prev_path)?;
    }

    // 3. Update .bak symlink (remove + create)
    let _ = fs::remove_file(&bak_path);
    if let Some(prev_name) = prev_path.file_name() {
        std::os::unix::fs::symlink(prev_name, &bak_path)?;
    }

    // 4. Rename tmp → live (atomic on same filesystem)
    fs::rename(&tmp_path, path)?;

    Ok(())
}

/// Resolve the backup path for a given primary path.
///
/// Follows the `.bak` symlink if it exists, otherwise falls back to `.prev`.
fn resolve_backup_path(path: &Path) -> PathBuf {
    let bak_path = path.with_extension("bak");
    fs::read_link(&bak_path).map_or_else(
        |_| path.with_extension("prev"),
        |target| {
            if target.is_relative() {
                path.parent().map_or_else(|| target.clone(), |p| p.join(&target))
            } else {
                target
            }
        },
    )
}

/// Read `path`, falling back to the `.bak` symlink target on failure.
///
/// Returns `None` only if neither the primary nor backup file is readable
/// and non-empty.
pub fn read_with_fallback(path: &Path) -> Option<String> {
    // Try primary
    match fs::read_to_string(path) {
        Ok(content) if !content.is_empty() => return Some(content),
        Ok(_) => {
            tracing::warn!("Primary file is empty: {}", path.display());
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // Primary missing — fall through to backup
        }
        Err(e) => {
            tracing::warn!("Failed to read primary file {}: {e}", path.display());
        }
    }

    // Try backup
    let prev_path = resolve_backup_path(path);
    match fs::read_to_string(&prev_path) {
        Ok(content) if !content.is_empty() => {
            tracing::info!("Recovered from backup: {}", prev_path.display());
            Some(content)
        }
        Ok(_) => {
            tracing::warn!("Backup file is empty: {}", prev_path.display());
            None
        }
        Err(_) => None,
    }
}

/// Read the primary file only (no fallback). Returns `None` if missing.
#[must_use]
pub fn read_primary(path: &Path) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(content) if !content.is_empty() => Some(content),
        _ => None,
    }
}

/// Read the backup file only. Returns `None` if missing or empty.
#[must_use]
pub fn read_backup(path: &Path) -> Option<String> {
    let prev_path = resolve_backup_path(path);
    match fs::read_to_string(&prev_path) {
        Ok(content) if !content.is_empty() => Some(content),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_creates_file_and_backup_symlink() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.json");

        write_with_backup(&path, "first").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first");

        write_with_backup(&path, "second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");

        let prev = path.with_extension("prev");
        assert_eq!(fs::read_to_string(&prev).unwrap(), "first");

        let bak = path.with_extension("bak");
        assert!(bak.is_symlink());
    }

    #[test]
    fn read_with_fallback_returns_primary() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("data.json");
        fs::write(&path, "content").unwrap();

        let result = read_with_fallback(&path);
        assert_eq!(result.as_deref(), Some("content"));
    }

    #[test]
    fn read_with_fallback_returns_none_when_both_missing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("missing.json");

        let result = read_with_fallback(&path);
        assert!(result.is_none());
    }

    #[test]
    fn read_with_fallback_uses_prev_when_primary_empty() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("data.json");
        let prev = path.with_extension("prev");
        let bak = path.with_extension("bak");

        fs::write(&path, "").unwrap();
        fs::write(&prev, "backup content").unwrap();
        std::os::unix::fs::symlink("data.prev", &bak).unwrap();

        let result = read_with_fallback(&path);
        assert_eq!(result.as_deref(), Some("backup content"));
    }

    #[test]
    fn read_with_fallback_uses_prev_when_primary_unreadable() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("sub").join("data.json");
        let prev = path.with_extension("prev");
        let bak = path.with_extension("bak");

        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Make primary unreadable by making it a directory
        fs::create_dir_all(&path).unwrap();
        fs::write(&prev, "recovered").unwrap();
        std::os::unix::fs::symlink("data.prev", &bak).unwrap();

        let result = read_with_fallback(&path);
        assert_eq!(result.as_deref(), Some("recovered"));
    }

    #[test]
    fn write_creates_parent_directories() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("deep").join("nested").join("file.json");

        write_with_backup(&path, "nested").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "nested");
    }

    #[test]
    fn multiple_writes_keep_only_one_prev() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("data.json");

        write_with_backup(&path, "v1").unwrap();
        write_with_backup(&path, "v2").unwrap();
        write_with_backup(&path, "v3").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "v3");
        let prev = path.with_extension("prev");
        assert_eq!(fs::read_to_string(&prev).unwrap(), "v2");
    }

    #[test]
    fn crash_after_tmp_write_leaves_original_intact() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("data.json");

        // First write succeeds
        write_with_backup(&path, "original").unwrap();

        // Simulate crash: only .tmp exists from a second write attempt
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, "partial").unwrap();

        let result = read_with_fallback(&path);
        assert_eq!(result.as_deref(), Some("original"));
    }

    #[test]
    fn crash_after_prev_copy_recovers_from_backup() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("data.json");
        let prev = path.with_extension("prev");
        let bak = path.with_extension("bak");

        // Simulate: primary is gone (crash during rename), but prev exists
        fs::write(&prev, "safe copy").unwrap();
        std::os::unix::fs::symlink("data.prev", &bak).unwrap();

        let result = read_with_fallback(&path);
        assert_eq!(result.as_deref(), Some("safe copy"));
    }

    #[test]
    fn read_primary_returns_none_when_missing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("missing.json");
        assert!(read_primary(&path).is_none());
    }

    #[test]
    fn read_backup_returns_content_from_prev() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("data.json");
        let prev = path.with_extension("prev");
        let bak = path.with_extension("bak");

        fs::write(&prev, "backup").unwrap();
        std::os::unix::fs::symlink("data.prev", &bak).unwrap();

        assert_eq!(read_backup(&path).as_deref(), Some("backup"));
    }
}
