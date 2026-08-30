use std::path::{Path, PathBuf};

/// Global configuration and constants for rttx.
pub const APP_NAME: &str = "rttx";
pub const APP_ID: &str = "io.github.IllyaYalovyy.rttx";
pub const DEV_APP_NAME: &str = "rttx (Devel)";
pub const DEV_APP_ID: &str = "io.github.IllyaYalovyy.rttx.Devel";
pub const DEV_ICON_NAME: &str = "io.github.IllyaYalovyy.rttx.Devel";
pub const DEVELOPER_NAME: &str = "Illya Yalovyy";
pub const PROJECT_WEBSITE: &str = "https://github.com/IllyaYalovyy/rttx";
pub const ISSUE_TRACKER: &str = "https://github.com/IllyaYalovyy/rttx/issues";
pub const SPONSORS_URL: &str = "https://github.com/sponsors/IllyaYalovyy";
pub const DEV_MODE_ENV: &str = "RTTX_DEV_MODE";

/// `GSettings` schema IDs
pub const SETTINGS_ID: &str = "io.github.IllyaYalovyy.rttx";
pub const SETTINGS_PATH: &str = "/io/github/IllyaYalovyy/rttx/";
pub const DEV_SETTINGS_ID: &str = DEV_APP_ID;
pub const DEV_SETTINGS_PATH: &str = "/io/github/IllyaYalovyy/rttx/Devel/";

/// Config directory name under `XDG_CONFIG_HOME`
pub const CONFIG_DIR: &str = "rttx";
pub const DEV_CONFIG_DIR: &str = "rttx-devel";
pub const SCHEMES_DIR: &str = "schemes";

/// State directory name under `$XDG_STATE_HOME`.
///
/// RFC-023 owns `client/` under this root. RFC-022 owns `daemon/`.
pub const STATE_DIR: &str = "rttx";
pub const DEV_STATE_DIR: &str = "rttx-devel";
const CLIENT_SUBDIR: &str = "client";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppProfile {
    pub app_id: &'static str,
    pub icon_name: &'static str,
    pub display_name: &'static str,
    pub config_dir: &'static str,
    pub state_dir: &'static str,
    pub settings_id: &'static str,
    pub settings_path: &'static str,
    pub badge_label: Option<&'static str>,
    pub is_development: bool,
}

#[must_use]
pub fn app_profile() -> AppProfile {
    app_profile_from_dev_mode(dev_mode_enabled())
}

#[must_use]
pub fn app_id() -> &'static str {
    app_profile().app_id
}

#[must_use]
pub fn icon_name() -> &'static str {
    app_profile().icon_name
}

#[must_use]
pub fn display_name() -> &'static str {
    app_profile().display_name
}

#[must_use]
pub fn badge_label() -> Option<&'static str> {
    app_profile().badge_label
}

#[must_use]
pub fn config_dir_name() -> &'static str {
    app_profile().config_dir
}

#[must_use]
pub fn settings_id() -> &'static str {
    app_profile().settings_id
}

#[must_use]
pub fn settings_path() -> &'static str {
    app_profile().settings_path
}

#[must_use]
pub fn is_development() -> bool {
    app_profile().is_development
}

#[must_use]
pub fn config_dir_path() -> PathBuf {
    config_dir_path_for(&glib::user_config_dir(), app_profile())
}

#[must_use]
pub fn config_dir_path_for(base: &Path, profile: AppProfile) -> PathBuf {
    base.join(profile.config_dir)
}

/// Return the client state directory under `$XDG_STATE_HOME`.
///
/// Layout: `$XDG_STATE_HOME/rttx/client/` (production) or
/// `$XDG_STATE_HOME/rttx-devel/client/` (dev mode).
#[must_use]
pub fn state_dir_path() -> PathBuf {
    let base = xdg_state_home();
    state_dir_path_for(&base, app_profile())
}

#[must_use]
pub fn state_dir_path_for(base: &Path, profile: AppProfile) -> PathBuf {
    base.join(profile.state_dir).join(CLIENT_SUBDIR)
}

fn xdg_state_home() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map_or_else(|| glib::home_dir().join(".local").join("state"), PathBuf::from)
}

#[must_use]
pub fn dev_icon_search_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("data").join("icons")
}

#[must_use]
pub const fn app_profile_from_dev_mode(is_dev: bool) -> AppProfile {
    if is_dev {
        AppProfile {
            app_id: DEV_APP_ID,
            icon_name: DEV_ICON_NAME,
            display_name: DEV_APP_NAME,
            config_dir: DEV_CONFIG_DIR,
            state_dir: DEV_STATE_DIR,
            settings_id: DEV_SETTINGS_ID,
            settings_path: DEV_SETTINGS_PATH,
            badge_label: Some("Devel"),
            is_development: true,
        }
    } else {
        AppProfile {
            app_id: APP_ID,
            icon_name: APP_ID,
            display_name: APP_NAME,
            config_dir: CONFIG_DIR,
            state_dir: STATE_DIR,
            settings_id: SETTINGS_ID,
            settings_path: SETTINGS_PATH,
            badge_label: None,
            is_development: false,
        }
    }
}

fn dev_mode_enabled() -> bool {
    std::env::var_os(DEV_MODE_ENV).is_some_and(|value| !value.is_empty() && value != "0")
}

use gtk4::glib;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
    fn production_profile_matches_existing_identity() {
        let profile = app_profile_from_dev_mode(false);
        assert_eq!(profile.app_id, APP_ID);
        assert_eq!(profile.icon_name, APP_ID);
        assert_eq!(profile.config_dir, CONFIG_DIR);
        assert_eq!(profile.state_dir, STATE_DIR);
        assert_eq!(profile.display_name, APP_NAME);
        assert!(!profile.is_development);
    }

    #[test]
    fn development_profile_uses_distinct_identity_state_and_labeling() {
        let profile = app_profile_from_dev_mode(true);
        assert_eq!(profile.app_id, "io.github.IllyaYalovyy.rttx.Devel");
        assert_eq!(profile.icon_name, "io.github.IllyaYalovyy.rttx.Devel");
        assert_eq!(profile.config_dir, "rttx-devel");
        assert_eq!(profile.state_dir, "rttx-devel");
        assert_eq!(profile.display_name, "rttx (Devel)");
        assert_eq!(profile.badge_label, Some("Devel"));
        assert!(profile.is_development);
    }

    #[test]
    fn settings_path_follows_runtime_settings_id() {
        let production = app_profile_from_dev_mode(false);
        let development = app_profile_from_dev_mode(true);

        assert_eq!(
            production.settings_path,
            format!("/{}/", production.settings_id.replace('.', "/"))
        );
        assert_eq!(
            development.settings_path,
            format!("/{}/", development.settings_id.replace('.', "/"))
        );
    }

    #[test]
    fn config_dir_path_joins_base_with_active_profile_directory() {
        let production = app_profile_from_dev_mode(false);
        let development = app_profile_from_dev_mode(true);
        let base = Path::new("/tmp/rttx-profile-test");

        assert_eq!(config_dir_path_for(base, production), Path::new("/tmp/rttx-profile-test/rttx"));
        assert_eq!(
            config_dir_path_for(base, development),
            Path::new("/tmp/rttx-profile-test/rttx-devel")
        );
    }

    #[test]
    fn state_dir_path_uses_client_subdir() {
        let production = app_profile_from_dev_mode(false);
        let development = app_profile_from_dev_mode(true);
        let base = Path::new("/tmp/state");

        assert_eq!(state_dir_path_for(base, production), Path::new("/tmp/state/rttx/client"));
        assert_eq!(
            state_dir_path_for(base, development),
            Path::new("/tmp/state/rttx-devel/client")
        );
    }

    #[test]
    fn state_and_config_dirs_are_disjoint() {
        let profile = app_profile_from_dev_mode(false);
        let config = config_dir_path_for(Path::new("/xdg/config"), profile);
        let state = state_dir_path_for(Path::new("/xdg/state"), profile);
        assert!(!state.starts_with(&config));
        assert!(!config.starts_with(&state));
    }

    #[test]
    fn development_icon_search_path_contains_dev_icon_asset() {
        let icon = dev_icon_search_path()
            .join("hicolor")
            .join("scalable")
            .join("apps")
            .join(format!("{DEV_ICON_NAME}.svg"));
        assert!(icon.exists(), "development icon asset missing at {}", icon.display());
    }

    #[test]
    fn gettext_domain_matches_app_name() {
        assert_eq!(APP_NAME, "rttx");
    }

    #[test]
    fn project_urls_target_repository() {
        assert!(PROJECT_WEBSITE.starts_with("https://github.com/"));
        assert!(ISSUE_TRACKER.starts_with(PROJECT_WEBSITE));
    }
}
