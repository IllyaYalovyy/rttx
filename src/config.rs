/// Application-wide constants and configuration.
pub const APP_ID: &str = "io.github.rttx";
pub const APP_NAME: &str = "rttx";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GETTEXT_DOMAIN: &str = "rttx";

/// GSettings schema IDs
pub const SETTINGS_ID: &str = "io.github.rttx";
pub const SETTINGS_PROFILE_BASE_PATH: &str = "/io/github/rttx/profiles/";

/// Config directory name under XDG_CONFIG_HOME
pub const CONFIG_DIR: &str = "rttx";
pub const SCHEMES_DIR: &str = "schemes";
pub const SESSIONS_DIR: &str = "sessions";

#[cfg(test)]
mod tests {
    use super::*;

    /// Requirement: APP_ID must be a valid reverse-DNS identifier for D-Bus,
    /// Flatpak, and GSettings.
    #[test]
    fn app_id_is_valid_reverse_dns() {
        let segments: Vec<&str> = APP_ID.split('.').collect();
        assert!(segments.len() >= 3, "APP_ID needs >= 3 segments for Flatpak: {APP_ID}");
        for seg in &segments {
            assert!(!seg.is_empty());
            assert!(seg.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-'));
            assert!(seg.chars().next().unwrap().is_alphabetic(),
                "Each segment must start with a letter: '{seg}'");
        }
    }

    /// Requirement: GSettings path must be derived from APP_ID.
    #[test]
    fn settings_path_derived_from_app_id() {
        let expected_prefix = format!("/{}/", APP_ID.replace('.', "/"));
        assert!(
            SETTINGS_PROFILE_BASE_PATH.starts_with(&expected_prefix),
            "Profile path '{}' must start with '{}'",
            SETTINGS_PROFILE_BASE_PATH, expected_prefix
        );
        assert!(
            SETTINGS_PROFILE_BASE_PATH.ends_with('/'),
            "Profile base path must end with '/'"
        );
    }

    /// Requirement: CONFIG_DIR must not contain path separators (it's a single
    /// directory name under XDG_CONFIG_HOME).
    #[test]
    fn config_dir_is_simple_name() {
        assert!(!CONFIG_DIR.contains('/'));
        assert!(!CONFIG_DIR.contains('\\'));
        assert!(!CONFIG_DIR.is_empty());
    }

    /// Requirement: APP_ID and SETTINGS_ID must match — they identify the same app.
    #[test]
    fn app_id_matches_settings_id() {
        assert_eq!(APP_ID, SETTINGS_ID);
    }

    /// Requirement: GETTEXT_DOMAIN should match APP_NAME for consistency.
    #[test]
    fn gettext_domain_matches_app_name() {
        assert_eq!(GETTEXT_DOMAIN, APP_NAME);
    }
}
