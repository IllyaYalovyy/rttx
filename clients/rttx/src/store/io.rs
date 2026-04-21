//! Atomic file I/O with backup recovery for client documents (RFC-023 §4).
//!
//! Write protocol:
//! 1. Serialize to `<path>.tmp` in the same directory.
//! 2. Flush and fsync the temporary file.
//! 3. Rename the current file to `<path>.bak` (last-good backup).
//! 4. Rename `<path>.tmp` into place.
//! 5. Fsync the parent directory.
//!
//! Load protocol:
//! - Missing file → return default.
//! - Malformed primary → move to `backups/`, try `.bak`, report recovery.
//! - Malformed primary and no usable backup → preserve bad file, return default.
//! - Unsupported future version → refuse (do not overwrite).

use crate::store::envelope::{
    DocumentEnvelope, EnvelopeError, Schema, peek_header, validate_header,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Outcome of loading a document.
#[derive(Debug, PartialEq, Eq)]
pub enum LoadOutcome<T> {
    /// Loaded successfully from the primary file.
    Loaded(T),
    /// Primary was malformed; recovered from backup.
    Recovered(T),
    /// File did not exist; returned the default.
    Default(T),
    /// Both primary and backup were unusable; returned the default.
    /// The malformed file was preserved in `backups/`.
    DefaultAfterFailure(T),
    /// The document version is newer than this client supports.
    /// The file was not modified.
    UnsupportedVersion { found: u32, max_supported: u32 },
}

impl<T> LoadOutcome<T> {
    /// Extract the loaded value, if any.
    #[must_use]
    pub fn into_value(self) -> Option<T> {
        match self {
            Self::Loaded(v)
            | Self::Recovered(v)
            | Self::Default(v)
            | Self::DefaultAfterFailure(v) => Some(v),
            Self::UnsupportedVersion { .. } => None,
        }
    }
}

/// Write a document envelope atomically.
///
/// Creates parent directories as needed.
///
/// # Errors
///
/// Returns an I/O error if the write, fsync, or rename fails.
pub fn atomic_save<T: Serialize>(path: &Path, envelope: &DocumentEnvelope<T>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp_path = path.with_extension("tmp");
    let bak_path = path.with_extension("bak");

    write_and_sync(&tmp_path, envelope)?;

    if path.exists() {
        fs::rename(path, &bak_path)?;
    }

    fs::rename(&tmp_path, path)?;
    fsync_dir(path);

    Ok(())
}

/// Load a document with envelope validation and backup recovery.
///
/// `backups_dir` is the directory where malformed files are preserved.
#[must_use]
pub fn atomic_load<T: for<'de> Deserialize<'de> + Default>(
    path: &Path,
    expected_schema: Schema,
    max_version: u32,
    backups_dir: &Path,
) -> LoadOutcome<T> {
    let Ok(primary_json) = fs::read_to_string(path) else {
        // Distinguish "not found" (normal first run) from other read errors.
        return if path.exists() {
            try_backup_or_default(path, expected_schema, max_version)
        } else {
            LoadOutcome::Default(T::default())
        };
    };

    match try_parse::<T>(&primary_json, expected_schema, max_version) {
        ParseResult::Ok(data) => LoadOutcome::Loaded(data),
        ParseResult::UnsupportedVersion { found, max_supported } => {
            LoadOutcome::UnsupportedVersion { found, max_supported }
        }
        ParseResult::Malformed => {
            preserve_malformed(path, backups_dir);
            try_backup_or_default(path, expected_schema, max_version)
        }
    }
}

enum ParseResult<T> {
    Ok(T),
    UnsupportedVersion { found: u32, max_supported: u32 },
    Malformed,
}

fn try_parse<T: for<'de> Deserialize<'de>>(
    json: &str,
    expected_schema: Schema,
    max_version: u32,
) -> ParseResult<T> {
    let Ok(header) = peek_header(json) else {
        return ParseResult::Malformed;
    };

    if let Err(e) = validate_header(&header, expected_schema, max_version) {
        return match e {
            EnvelopeError::UnsupportedVersion { found, max_supported, .. } => {
                ParseResult::UnsupportedVersion { found, max_supported }
            }
            _ => ParseResult::Malformed,
        };
    }

    match serde_json::from_str::<DocumentEnvelope<T>>(json) {
        Ok(env) => ParseResult::Ok(env.data),
        Err(_) => ParseResult::Malformed,
    }
}

fn try_backup_or_default<T: for<'de> Deserialize<'de> + Default>(
    path: &Path,
    expected_schema: Schema,
    max_version: u32,
) -> LoadOutcome<T> {
    if let Ok(bak_json) = fs::read_to_string(path.with_extension("bak"))
        && let ParseResult::Ok(data) = try_parse::<T>(&bak_json, expected_schema, max_version)
    {
        return LoadOutcome::Recovered(data);
    }
    LoadOutcome::DefaultAfterFailure(T::default())
}

/// Move a malformed primary file into the backups directory.
fn preserve_malformed(path: &Path, backups_dir: &Path) {
    let file_name =
        path.file_name().map_or_else(|| "unknown".into(), |n| n.to_string_lossy().into_owned());

    if fs::create_dir_all(backups_dir).is_err() {
        tracing::error!("Failed to create backups directory: {}", backups_dir.display());
        return;
    }

    if let Err(e) = fs::rename(path, backups_dir.join(&file_name)) {
        tracing::error!("Failed to move malformed file to backups: {e}");
    } else {
        tracing::warn!("Moved malformed {file_name} to {}", backups_dir.display());
    }
}

fn write_and_sync<T: Serialize>(path: &Path, envelope: &DocumentEnvelope<T>) -> io::Result<()> {
    let json = serde_json::to_string_pretty(envelope).map_err(io::Error::other)?;
    let mut file = fs::File::create(path)?;
    file.write_all(json.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

fn fsync_dir(file_path: &Path) {
    if let Some(parent) = file_path.parent()
        && let Ok(dir) = fs::File::open(parent)
    {
        let _ = dir.sync_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::envelope::DocumentEnvelope;
    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;

    #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
    struct TestData {
        #[serde(default)]
        value: String,
    }

    fn make_envelope(schema: Schema, version: u32, data: TestData) -> DocumentEnvelope<TestData> {
        DocumentEnvelope {
            schema,
            version,
            app_version: "0.4.0".into(),
            written_at: "2026-01-01T00:00:00Z".into(),
            data,
        }
    }

    #[test]
    fn save_creates_file_with_valid_envelope() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("prefs.json");
        let env = make_envelope(Schema::Preferences, 1, TestData { value: "hello".into() });

        atomic_save(&path, &env).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let loaded: DocumentEnvelope<TestData> = serde_json::from_str(&content).unwrap();
        assert_eq!(loaded.schema, Schema::Preferences);
        assert_eq!(loaded.data.value, "hello");
    }

    #[test]
    fn save_creates_parent_directories() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("deep").join("nested").join("doc.json");
        let env = make_envelope(Schema::Hosts, 1, TestData::default());

        atomic_save(&path, &env).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn save_creates_bak_on_overwrite() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("doc.json");

        let v1 = make_envelope(Schema::Preferences, 1, TestData { value: "v1".into() });
        atomic_save(&path, &v1).unwrap();

        let v2 = make_envelope(Schema::Preferences, 1, TestData { value: "v2".into() });
        atomic_save(&path, &v2).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("v2"));

        let bak_content = fs::read_to_string(path.with_extension("bak")).unwrap();
        assert!(bak_content.contains("v1"));
    }

    #[test]
    fn save_no_tmp_file_left_behind() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("doc.json");
        let env = make_envelope(Schema::Preferences, 1, TestData::default());

        atomic_save(&path, &env).unwrap();
        assert!(!path.with_extension("tmp").exists());
    }

    #[test]
    fn load_missing_file_returns_default() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("missing.json");
        let backups = tmp.path().join("backups");

        let outcome: LoadOutcome<TestData> = atomic_load(&path, Schema::Preferences, 1, &backups);
        assert!(matches!(outcome, LoadOutcome::Default(_)));
        assert_eq!(outcome.into_value().unwrap(), TestData::default());
    }

    #[test]
    fn load_valid_file_returns_loaded() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("prefs.json");
        let backups = tmp.path().join("backups");
        let env = make_envelope(Schema::Preferences, 1, TestData { value: "loaded".into() });
        atomic_save(&path, &env).unwrap();

        let outcome: LoadOutcome<TestData> = atomic_load(&path, Schema::Preferences, 1, &backups);
        assert!(matches!(outcome, LoadOutcome::Loaded(_)));
        assert_eq!(outcome.into_value().unwrap().value, "loaded");
    }

    #[test]
    fn load_malformed_primary_recovers_from_backup() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("doc.json");
        let backups = tmp.path().join("backups");

        let good = make_envelope(Schema::Preferences, 1, TestData { value: "backup".into() });
        fs::write(path.with_extension("bak"), serde_json::to_string_pretty(&good).unwrap())
            .unwrap();

        fs::write(&path, "not valid json").unwrap();

        let outcome: LoadOutcome<TestData> = atomic_load(&path, Schema::Preferences, 1, &backups);
        assert!(matches!(outcome, LoadOutcome::Recovered(_)));
        assert_eq!(outcome.into_value().unwrap().value, "backup");
    }

    #[test]
    fn load_malformed_primary_preserves_bad_file_in_backups() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("doc.json");
        let backups = tmp.path().join("backups");

        fs::write(&path, "corrupted content").unwrap();

        let _: LoadOutcome<TestData> = atomic_load(&path, Schema::Preferences, 1, &backups);

        let preserved = backups.join("doc.json");
        assert!(preserved.exists());
        assert_eq!(fs::read_to_string(&preserved).unwrap(), "corrupted content");
    }

    #[test]
    fn load_malformed_primary_and_backup_returns_default_after_failure() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("doc.json");
        let backups = tmp.path().join("backups");

        fs::write(&path, "bad primary").unwrap();
        fs::write(path.with_extension("bak"), "bad backup").unwrap();

        let outcome: LoadOutcome<TestData> = atomic_load(&path, Schema::Preferences, 1, &backups);
        assert!(matches!(outcome, LoadOutcome::DefaultAfterFailure(_)));
    }

    #[test]
    fn load_unsupported_future_version_refuses() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("doc.json");
        let backups = tmp.path().join("backups");

        let env = make_envelope(Schema::Preferences, 99, TestData { value: "future".into() });
        atomic_save(&path, &env).unwrap();

        let outcome: LoadOutcome<TestData> = atomic_load(&path, Schema::Preferences, 1, &backups);
        assert!(matches!(outcome, LoadOutcome::UnsupportedVersion { found: 99, max_supported: 1 }));

        // File must NOT be modified or deleted
        assert!(path.exists());
        assert!(fs::read_to_string(&path).unwrap().contains("future"));
    }

    #[test]
    fn load_wrong_schema_treats_as_malformed() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("doc.json");
        let backups = tmp.path().join("backups");

        let env = make_envelope(Schema::Hosts, 1, TestData { value: "hosts".into() });
        atomic_save(&path, &env).unwrap();

        let outcome: LoadOutcome<TestData> = atomic_load(&path, Schema::Preferences, 1, &backups);
        assert!(matches!(outcome, LoadOutcome::DefaultAfterFailure(_)));
    }

    #[test]
    fn load_older_version_succeeds() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("doc.json");
        let backups = tmp.path().join("backups");

        let env = make_envelope(Schema::Preferences, 1, TestData { value: "old".into() });
        atomic_save(&path, &env).unwrap();

        let outcome: LoadOutcome<TestData> = atomic_load(&path, Schema::Preferences, 3, &backups);
        assert!(matches!(outcome, LoadOutcome::Loaded(_)));
        assert_eq!(outcome.into_value().unwrap().value, "old");
    }

    #[test]
    fn save_then_load_round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("round.json");
        let backups = tmp.path().join("backups");

        let data = TestData { value: "round-trip".into() };
        let env = DocumentEnvelope::new(Schema::Library, 1, data.clone());
        atomic_save(&path, &env).unwrap();

        let outcome: LoadOutcome<TestData> = atomic_load(&path, Schema::Library, 1, &backups);
        assert_eq!(outcome.into_value().unwrap(), data);
    }

    #[test]
    fn load_outcome_into_value_returns_none_for_unsupported() {
        let outcome: LoadOutcome<TestData> =
            LoadOutcome::UnsupportedVersion { found: 5, max_supported: 1 };
        assert!(outcome.into_value().is_none());
    }

    #[test]
    fn multiple_saves_keep_only_one_bak() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("doc.json");

        for i in 1..=3 {
            let env = make_envelope(Schema::Preferences, 1, TestData { value: format!("v{i}") });
            atomic_save(&path, &env).unwrap();
        }

        assert!(fs::read_to_string(&path).unwrap().contains("v3"));
        assert!(fs::read_to_string(path.with_extension("bak")).unwrap().contains("v2"));
    }

    #[test]
    fn crash_after_tmp_write_leaves_original_intact() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("doc.json");
        let backups = tmp.path().join("backups");

        let env = make_envelope(Schema::Preferences, 1, TestData { value: "original".into() });
        atomic_save(&path, &env).unwrap();

        // Simulate crash: only .tmp exists from a second write attempt
        fs::write(path.with_extension("tmp"), "partial write").unwrap();

        let outcome: LoadOutcome<TestData> = atomic_load(&path, Schema::Preferences, 1, &backups);
        assert!(matches!(outcome, LoadOutcome::Loaded(_)));
        assert_eq!(outcome.into_value().unwrap().value, "original");
    }
}
