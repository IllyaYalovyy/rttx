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
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    #[rstest]
    #[case(APP_ID)]
    #[case(APP_NAME)]
    #[case(APP_VERSION)]
    #[case(GETTEXT_DOMAIN)]
    #[case(SETTINGS_ID)]
    #[case(CONFIG_DIR)]
    #[case(SCHEMES_DIR)]
    #[case(SESSIONS_DIR)]
    fn constants_are_non_empty(#[case] value: &str) {
        assert!(!value.is_empty(), "Constant must not be empty: {value}");
    }

    #[rstest]
    #[case(APP_ID)]
    #[case(SETTINGS_ID)]
    fn valid_dbus_name(#[case] id: &str) {
        let segments: Vec<&str> = id.split('.').collect();
        assert!(
            segments.len() >= 2,
            "D-Bus name '{id}' must have >= 2 dot-separated segments"
        );
        for seg in &segments {
            assert!(!seg.is_empty(), "Segment must not be empty in '{id}'");
            assert!(
                seg.chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-'),
                "Invalid char in segment '{seg}' of '{id}'"
            );
        }
    }

    #[test]
    fn settings_path_matches_app_id() {
        // Ensure the GSettings path is consistent with the app ID
        let expected_prefix = format!("/{}/", APP_ID.replace('.', "/"));
        assert!(
            SETTINGS_PROFILE_BASE_PATH.starts_with(&expected_prefix),
            "Profile path '{}' should start with '{}'",
            SETTINGS_PROFILE_BASE_PATH,
            expected_prefix
        );
    }
}
