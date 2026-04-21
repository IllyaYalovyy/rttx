//! Document envelope with schema identity and version (RFC-023 §2).

use serde::{Deserialize, Serialize};

/// Known document schemas. Unknown values are rejected on load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Schema {
    #[serde(rename = "rttx.client.preferences")]
    Preferences,
    #[serde(rename = "rttx.client.hosts")]
    Hosts,
    #[serde(rename = "rttx.client.library")]
    Library,
    #[serde(rename = "rttx.client.workspaces")]
    Workspaces,
    #[serde(rename = "rttx.client.ui")]
    Ui,
    #[serde(rename = "rttx.client.runtime_cache")]
    RuntimeCache,
    #[serde(rename = "rttx.client.migrations")]
    Migrations,
}

/// Self-describing envelope wrapping every persisted client document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentEnvelope<T> {
    pub schema: Schema,
    pub version: u32,
    pub app_version: String,
    pub written_at: String,
    pub data: T,
}

impl<T> DocumentEnvelope<T> {
    /// Create a new envelope stamped with the current app version and time.
    pub fn new(schema: Schema, version: u32, data: T) -> Self {
        Self {
            schema,
            version,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            written_at: now_iso8601(),
            data,
        }
    }
}

/// Minimal envelope for peeking at schema and version without deserializing data.
#[derive(Debug, Deserialize)]
pub struct EnvelopeHeader {
    pub schema: Schema,
    pub version: u32,
}

/// Peek at the envelope header without deserializing the `data` payload.
///
/// # Errors
///
/// Returns an error if the JSON is malformed or the `schema` value is unknown.
pub fn peek_header(json: &str) -> Result<EnvelopeHeader, EnvelopeError> {
    serde_json::from_str(json).map_err(EnvelopeError::Parse)
}

/// Validate that a header matches the expected schema and a supported version range.
///
/// # Errors
///
/// - `SchemaMismatch` if the header schema differs from `expected`.
/// - `UnsupportedVersion` if the header version exceeds `max_supported`.
pub fn validate_header(
    header: &EnvelopeHeader,
    expected: Schema,
    max_supported: u32,
) -> Result<(), EnvelopeError> {
    if header.schema != expected {
        return Err(EnvelopeError::SchemaMismatch { expected, found: header.schema });
    }
    if header.version > max_supported {
        return Err(EnvelopeError::UnsupportedVersion {
            schema: expected,
            found: header.version,
            max_supported,
        });
    }
    Ok(())
}

#[derive(Debug)]
pub enum EnvelopeError {
    Parse(serde_json::Error),
    SchemaMismatch { expected: Schema, found: Schema },
    UnsupportedVersion { schema: Schema, found: u32, max_supported: u32 },
}

impl std::fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "envelope parse error: {e}"),
            Self::SchemaMismatch { expected, found } => {
                write!(f, "schema mismatch: expected {expected:?}, found {found:?}")
            }
            Self::UnsupportedVersion { schema, found, max_supported } => {
                write!(f, "{schema:?}: version {found} is newer than max supported {max_supported}")
            }
        }
    }
}

impl std::error::Error for EnvelopeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(e) => Some(e),
            Self::SchemaMismatch { .. } | Self::UnsupportedVersion { .. } => None,
        }
    }
}

fn now_iso8601() -> String {
    // Use SystemTime for a simple UTC timestamp without adding chrono.
    use std::time::SystemTime;
    let duration = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();

    // Manual UTC formatting: good enough for a diagnostic timestamp.
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;

    // Days since epoch to Y-M-D (simplified Gregorian).
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

const fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Algorithm from Howard Hinnant's `civil_from_days`.
    days += 719_468;
    let era = days / 146_097;
    let doe = days % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn envelope_round_trips_through_json() {
        let env = DocumentEnvelope::new(Schema::Preferences, 1, json!({"font_size": 14}));
        let json = serde_json::to_string(&env).unwrap();
        let loaded: DocumentEnvelope<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.schema, Schema::Preferences);
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.data["font_size"], 14);
    }

    #[test]
    fn peek_header_extracts_schema_and_version() {
        let json = r#"{"schema":"rttx.client.hosts","version":3,"app_version":"0.4.0","written_at":"2026-01-01T00:00:00Z","data":{}}"#;
        let header = peek_header(json).unwrap();
        assert_eq!(header.schema, Schema::Hosts);
        assert_eq!(header.version, 3);
    }

    #[test]
    fn peek_header_rejects_unknown_schema() {
        let json = r#"{"schema":"rttx.client.unknown","version":1,"app_version":"0.4.0","written_at":"2026-01-01T00:00:00Z","data":{}}"#;
        let err = peek_header(json).unwrap_err();
        assert!(matches!(err, EnvelopeError::Parse(_)));
    }

    #[test]
    fn peek_header_rejects_malformed_json() {
        let err = peek_header("not json").unwrap_err();
        assert!(matches!(err, EnvelopeError::Parse(_)));
    }

    #[test]
    fn validate_header_accepts_current_version() {
        let header = EnvelopeHeader { schema: Schema::Preferences, version: 1 };
        assert!(validate_header(&header, Schema::Preferences, 1).is_ok());
    }

    #[test]
    fn validate_header_accepts_older_version() {
        let header = EnvelopeHeader { schema: Schema::Preferences, version: 1 };
        assert!(validate_header(&header, Schema::Preferences, 3).is_ok());
    }

    #[test]
    fn validate_header_rejects_future_version() {
        let header = EnvelopeHeader { schema: Schema::Preferences, version: 5 };
        let err = validate_header(&header, Schema::Preferences, 2).unwrap_err();
        assert!(matches!(
            err,
            EnvelopeError::UnsupportedVersion { found: 5, max_supported: 2, .. }
        ));
    }

    #[test]
    fn validate_header_rejects_schema_mismatch() {
        let header = EnvelopeHeader { schema: Schema::Hosts, version: 1 };
        let err = validate_header(&header, Schema::Preferences, 1).unwrap_err();
        assert!(matches!(err, EnvelopeError::SchemaMismatch { .. }));
    }

    #[test]
    fn schema_serializes_to_dotted_string() {
        let json = serde_json::to_string(&Schema::Preferences).unwrap();
        assert_eq!(json, r#""rttx.client.preferences""#);
    }

    #[test]
    fn all_schemas_round_trip() {
        let schemas = [
            Schema::Preferences,
            Schema::Hosts,
            Schema::Library,
            Schema::Workspaces,
            Schema::Ui,
            Schema::RuntimeCache,
            Schema::Migrations,
        ];
        for schema in schemas {
            let json = serde_json::to_string(&schema).unwrap();
            let loaded: Schema = serde_json::from_str(&json).unwrap();
            assert_eq!(loaded, schema);
        }
    }

    #[test]
    fn envelope_new_stamps_app_version() {
        let env = DocumentEnvelope::new(Schema::Ui, 1, ());
        assert_eq!(env.app_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn envelope_new_stamps_written_at_as_iso8601() {
        let env = DocumentEnvelope::new(Schema::Ui, 1, ());
        // Should look like YYYY-MM-DDTHH:MM:SSZ
        assert!(env.written_at.ends_with('Z'), "timestamp should end with Z");
        assert_eq!(env.written_at.len(), 20, "ISO 8601 UTC should be 20 chars");
    }

    #[test]
    fn days_to_ymd_epoch() {
        assert_eq!(super::days_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn days_to_ymd_known_date() {
        // 2026-04-13 is day 20_556 since epoch
        assert_eq!(super::days_to_ymd(20_556), (2026, 4, 13));
    }
}
