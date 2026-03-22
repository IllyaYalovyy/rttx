/// Global configuration and constants for rttx.
pub const APP_NAME: &str = "rttx";
pub const APP_ID: &str = "io.github.IllyaYalovyy.rttx";

/// `GSettings` schema IDs
pub const SETTINGS_ID: &str = "io.github.IllyaYalovyy.rttx";
pub const SETTINGS_PATH: &str = "/io/github/IllyaYalovyy/rttx/";

/// Config directory name under `XDG_CONFIG_HOME`
pub const CONFIG_DIR: &str = "rttx";
pub const SCHEMES_DIR: &str = "schemes";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_id_is_valid_reverse_dns() {
        assert!(APP_ID.contains('.'));
        assert!(APP_ID.starts_with("io.github"));
        assert!(APP_ID.contains(".rttx"));
    }

    #[test]
    fn app_id_matches_settings_id() {
        assert_eq!(APP_ID, SETTINGS_ID);
    }

    #[test]
    fn settings_path_derived_from_app_id() {
        let expected = format!("/{}/", APP_ID.replace('.', "/"));
        assert_eq!(SETTINGS_PATH, expected);
    }

    #[test]
    fn config_dir_is_simple_name() {
        assert!(!CONFIG_DIR.contains('/'));
    }

    #[test]
    fn gettext_domain_matches_app_name() {
        assert_eq!(APP_NAME, "rttx");
    }
}
