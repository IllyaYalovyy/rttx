//! Client-side document store with versioned envelopes and atomic writes (RFC-023 §2, §4, §6).
//!
//! Every persisted JSON document uses a self-describing envelope with `schema`,
//! `version`, and diagnostic fields. Writes are atomic (temp + fsync + rename).
//! Loads recover from malformed files by falling back to the last-good backup.

mod envelope;
mod io;
pub mod models;
mod paths;

pub use envelope::{DocumentEnvelope, Schema};
pub use io::{LoadOutcome, atomic_load, atomic_save};
pub use paths::StorePaths;

use crate::commands::SavedCommand;
use crate::host::Host;
use crate::places::Place;
use crate::preferences::Preferences;

/// Build a `ClientStore` from the active application profile.
///
/// Uses `config_dir_path()` for config, `state_dir_path()` for state,
/// and the XDG cache directory for cache.
#[must_use]
pub fn default_store() -> ClientStore {
    let profile = crate::config::app_profile();
    let config = crate::config::config_dir_path();
    let state = crate::config::state_dir_path();
    let cache_dir_name = if profile.is_development { "rttx-devel" } else { "rttx" };
    let cache = gtk4::glib::user_cache_dir().join(cache_dir_name);
    ClientStore::new(StorePaths::new(config, state, cache))
}

use gtk4;

/// Client store providing typed document persistence with atomic I/O.
#[derive(Debug, Clone)]
pub struct ClientStore {
    paths: StorePaths,
}

impl ClientStore {
    #[must_use]
    pub const fn new(paths: StorePaths) -> Self {
        Self { paths }
    }

    #[must_use]
    pub const fn paths(&self) -> &StorePaths {
        &self.paths
    }

    // ── Preferences ─────────────────────────────────────────

    /// Load preferences through the envelope-aware loader with malformed-file recovery.
    #[must_use]
    pub fn load_preferences(&self) -> LoadOutcome<Preferences> {
        let path = self.paths.config().join("preferences.json");
        let outcome: LoadOutcome<models::preferences::PreferencesV1> = atomic_load(
            &path,
            models::preferences::SCHEMA,
            models::preferences::CURRENT_VERSION,
            &self.paths.backups(),
        );
        outcome.map(Into::into)
    }

    /// Save preferences atomically with an envelope wrapper.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the atomic write fails.
    pub fn save_preferences(&self, prefs: &Preferences) -> std::io::Result<()> {
        let path = self.paths.config().join("preferences.json");
        let v1: models::preferences::PreferencesV1 = prefs.into();
        let envelope = DocumentEnvelope::new(
            models::preferences::SCHEMA,
            models::preferences::CURRENT_VERSION,
            v1,
        );
        atomic_save(&path, &envelope)
    }

    // ── Hosts ───────────────────────────────────────────────

    /// Load hosts through the envelope-aware loader with malformed-file recovery.
    #[must_use]
    pub fn load_hosts(&self) -> LoadOutcome<Vec<Host>> {
        let path = self.paths.config().join("hosts.json");
        let outcome: LoadOutcome<models::hosts::HostCatalog> = atomic_load(
            &path,
            models::hosts::SCHEMA,
            models::hosts::CURRENT_VERSION,
            &self.paths.backups(),
        );
        outcome.map(|catalog| catalog.hosts.into_iter().map(Into::into).collect())
    }

    /// Save hosts atomically with an envelope wrapper.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the atomic write fails.
    pub fn save_hosts(&self, hosts: &[Host]) -> std::io::Result<()> {
        let path = self.paths.config().join("hosts.json");
        let catalog = models::hosts::HostCatalog { hosts: hosts.iter().map(Into::into).collect() };
        let envelope =
            DocumentEnvelope::new(models::hosts::SCHEMA, models::hosts::CURRENT_VERSION, catalog);
        atomic_save(&path, &envelope)
    }

    // ── Library (places + commands) ─────────────────────────

    /// Load the combined library (places and commands) through the envelope-aware loader.
    #[must_use]
    pub fn load_library(&self) -> LoadOutcome<(Vec<Place>, Vec<SavedCommand>)> {
        let path = self.paths.config().join("library.json");
        let outcome: LoadOutcome<models::library::Library> = atomic_load(
            &path,
            models::library::SCHEMA,
            models::library::CURRENT_VERSION,
            &self.paths.backups(),
        );
        outcome.map(|lib| {
            let places = lib.places.into_iter().map(Into::into).collect();
            let commands = lib.commands.into_iter().map(Into::into).collect();
            (places, commands)
        })
    }

    /// Save places and commands as a single library document.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the atomic write fails.
    pub fn save_library(&self, places: &[Place], commands: &[SavedCommand]) -> std::io::Result<()> {
        let path = self.paths.config().join("library.json");
        let library = models::library::Library {
            places: places.iter().map(Into::into).collect(),
            commands: commands.iter().map(Into::into).collect(),
        };
        let envelope = DocumentEnvelope::new(
            models::library::SCHEMA,
            models::library::CURRENT_VERSION,
            library,
        );
        atomic_save(&path, &envelope)
    }

    // ── Convenience: load/save just places or commands ──────

    /// Load only the places from the library document.
    #[must_use]
    pub fn load_places(&self) -> Vec<Place> {
        self.load_library().into_value().map_or_else(Vec::new, |(p, _)| p)
    }

    /// Load only the commands from the library document.
    #[must_use]
    pub fn load_commands(&self) -> Vec<SavedCommand> {
        self.load_library().into_value().map_or_else(Vec::new, |(_, c)| c)
    }

    /// Save places, preserving the current commands.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the atomic write fails.
    pub fn save_places(&self, places: &[Place]) -> std::io::Result<()> {
        let commands = self.load_commands();
        self.save_library(places, &commands)
    }

    /// Save commands, preserving the current places.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the atomic write fails.
    pub fn save_commands(&self, commands: &[SavedCommand]) -> std::io::Result<()> {
        let places = self.load_places();
        self.save_library(&places, commands)
    }

    // ── Workspaces ──────────────────────────────────────────

    /// Load workspaces through the envelope-aware loader with malformed-file recovery.
    #[must_use]
    pub fn load_workspaces(&self) -> LoadOutcome<models::workspaces::WorkspaceStore> {
        let path = self.paths.state().join("workspaces.json");
        atomic_load(
            &path,
            models::workspaces::SCHEMA,
            models::workspaces::CURRENT_VERSION,
            &self.paths.backups(),
        )
    }

    /// Save workspaces atomically with an envelope wrapper.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the atomic write fails.
    pub fn save_workspaces(
        &self,
        store: &models::workspaces::WorkspaceStore,
    ) -> std::io::Result<()> {
        let path = self.paths.state().join("workspaces.json");
        let envelope = DocumentEnvelope::new(
            models::workspaces::SCHEMA,
            models::workspaces::CURRENT_VERSION,
            store.clone(),
        );
        atomic_save(&path, &envelope)
    }

    // ── UI State ────────────────────────────────────────────

    /// Load UI state through the envelope-aware loader with malformed-file recovery.
    #[must_use]
    pub fn load_ui_state(&self) -> LoadOutcome<models::ui::UiState> {
        let path = self.paths.state().join("ui.json");
        atomic_load(
            &path,
            models::ui::SCHEMA,
            models::ui::CURRENT_VERSION,
            &self.paths.backups(),
        )
    }

    /// Save UI state atomically with an envelope wrapper.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the atomic write fails.
    pub fn save_ui_state(&self, ui: &models::ui::UiState) -> std::io::Result<()> {
        let path = self.paths.state().join("ui.json");
        let envelope =
            DocumentEnvelope::new(models::ui::SCHEMA, models::ui::CURRENT_VERSION, ui.clone());
        atomic_save(&path, &envelope)
    }

    // ── Runtime Cache ───────────────────────────────────────

    /// Load runtime cache through the envelope-aware loader with malformed-file recovery.
    #[must_use]
    pub fn load_runtime_cache(&self) -> LoadOutcome<models::runtime_cache::RuntimeCache> {
        let path = self.paths.cache().join("runtime-cache.json");
        atomic_load(
            &path,
            models::runtime_cache::SCHEMA,
            models::runtime_cache::CURRENT_VERSION,
            &self.paths.backups(),
        )
    }

    /// Save runtime cache atomically with an envelope wrapper.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the atomic write fails.
    pub fn save_runtime_cache(
        &self,
        cache: &models::runtime_cache::RuntimeCache,
    ) -> std::io::Result<()> {
        let path = self.paths.cache().join("runtime-cache.json");
        let envelope = DocumentEnvelope::new(
            models::runtime_cache::SCHEMA,
            models::runtime_cache::CURRENT_VERSION,
            cache.clone(),
        );
        atomic_save(&path, &envelope)
    }
}

impl<T> LoadOutcome<T> {
    /// Transform the contained value, preserving the outcome variant.
    #[must_use]
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> LoadOutcome<U> {
        match self {
            Self::Loaded(v) => LoadOutcome::Loaded(f(v)),
            Self::Recovered(v) => LoadOutcome::Recovered(f(v)),
            Self::Default(v) => LoadOutcome::Default(f(v)),
            Self::DefaultAfterFailure(v) => LoadOutcome::DefaultAfterFailure(f(v)),
            Self::UnsupportedVersion { found, max_supported } => {
                LoadOutcome::UnsupportedVersion { found, max_supported }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_store() -> (TempDir, ClientStore) {
        let tmp = TempDir::new().unwrap();
        let paths = StorePaths::new(
            tmp.path().join("config"),
            tmp.path().join("state"),
            tmp.path().join("cache"),
        );
        (tmp, ClientStore::new(paths))
    }

    // ── Preferences ─────────────────────────────────────────

    #[test]
    fn preferences_round_trip_through_store() {
        let (_tmp, store) = test_store();
        let prefs = Preferences {
            font: "JetBrains Mono 14".into(),
            scrollback_lines: 5000,
            ..Default::default()
        };
        store.save_preferences(&prefs).unwrap();
        let loaded = store.load_preferences().into_value().unwrap();
        assert_eq!(loaded.font, "JetBrains Mono 14");
        assert_eq!(loaded.scrollback_lines, 5000);
    }

    #[test]
    fn preferences_missing_file_returns_default() {
        let (_tmp, store) = test_store();
        let outcome = store.load_preferences();
        assert!(matches!(outcome, LoadOutcome::Default(_)));
    }

    #[test]
    fn preferences_malformed_file_recovers_to_default() {
        let (_tmp, store) = test_store();
        let path = store.paths.config().join("preferences.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        let outcome = store.load_preferences();
        assert!(matches!(outcome, LoadOutcome::DefaultAfterFailure(_)));
    }

    #[test]
    fn preferences_malformed_primary_recovers_from_backup() {
        let (_tmp, store) = test_store();
        // Save a good version first
        let prefs = Preferences { font: "Backup Font 12".into(), ..Default::default() };
        store.save_preferences(&prefs).unwrap();
        // Save again to create .bak
        let prefs2 = Preferences { font: "Current Font 12".into(), ..Default::default() };
        store.save_preferences(&prefs2).unwrap();
        // Corrupt the primary
        let path = store.paths.config().join("preferences.json");
        std::fs::write(&path, "corrupted").unwrap();
        let outcome = store.load_preferences();
        assert!(matches!(outcome, LoadOutcome::Recovered(_)));
        assert_eq!(outcome.into_value().unwrap().font, "Backup Font 12");
    }

    // ── Hosts ───────────────────────────────────────────────

    #[test]
    fn hosts_round_trip_through_store() {
        let (_tmp, store) = test_store();
        let hosts = vec![Host::remote("deploy@example.com")];
        store.save_hosts(&hosts).unwrap();
        let loaded = store.load_hosts().into_value().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].key, "example.com");
        assert_eq!(loaded[0].ssh_target.as_deref(), Some("deploy@example.com"));
    }

    #[test]
    fn hosts_missing_file_returns_empty() {
        let (_tmp, store) = test_store();
        let outcome = store.load_hosts();
        assert!(matches!(outcome, LoadOutcome::Default(_)));
        assert!(outcome.into_value().unwrap().is_empty());
    }

    #[test]
    fn hosts_malformed_file_recovers_to_default() {
        let (_tmp, store) = test_store();
        let path = store.paths.config().join("hosts.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "bad json").unwrap();
        let outcome = store.load_hosts();
        assert!(matches!(outcome, LoadOutcome::DefaultAfterFailure(_)));
    }

    // ── Library ─────────────────────────────────────────────

    #[test]
    fn library_round_trip_through_store() {
        let (_tmp, store) = test_store();
        let mut place = Place::new("rttx", "~/pro/rttx");
        place.host_tags = vec!["local".into()];
        let mut cmd = SavedCommand::new("Build", "cargo build");
        cmd.host_tags = vec!["local".into(), "example.com".into()];

        store.save_library(&[place], &[cmd]).unwrap();
        let (places, commands) = store.load_library().into_value().unwrap();
        assert_eq!(places.len(), 1);
        assert_eq!(places[0].name, "rttx");
        assert_eq!(places[0].host_tags, vec!["local"]);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].title, "Build");
        assert_eq!(commands[0].host_tags, vec!["local", "example.com"]);
    }

    #[test]
    fn library_missing_file_returns_empty() {
        let (_tmp, store) = test_store();
        let outcome = store.load_library();
        assert!(matches!(outcome, LoadOutcome::Default(_)));
        let (places, commands) = outcome.into_value().unwrap();
        assert!(places.is_empty());
        assert!(commands.is_empty());
    }

    #[test]
    fn library_malformed_file_recovers_to_default() {
        let (_tmp, store) = test_store();
        let path = store.paths.config().join("library.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "garbage").unwrap();
        let outcome = store.load_library();
        assert!(matches!(outcome, LoadOutcome::DefaultAfterFailure(_)));
    }

    #[test]
    fn library_preserves_uuid_and_run_mode() {
        let (_tmp, store) = test_store();
        let place = Place {
            uuid: "fixed-uuid-1".into(),
            name: "Test".into(),
            path: "/test".into(),
            host_tags: vec![],
        };
        let cmd = SavedCommand {
            uuid: "fixed-uuid-2".into(),
            title: "Insert".into(),
            body: "echo hi".into(),
            default_run_mode: crate::commands::CommandRunMode::Insert,
            host_tags: vec![],
        };
        store.save_library(&[place], &[cmd]).unwrap();
        let (places, commands) = store.load_library().into_value().unwrap();
        assert_eq!(places[0].uuid, "fixed-uuid-1");
        assert_eq!(commands[0].uuid, "fixed-uuid-2");
        assert_eq!(commands[0].default_run_mode, crate::commands::CommandRunMode::Insert);
    }

    // ── LoadOutcome::map ────────────────────────────────────

    #[test]
    fn load_outcome_map_preserves_variant() {
        let loaded: LoadOutcome<i32> = LoadOutcome::Loaded(42);
        let mapped = loaded.map(|v| v.to_string());
        assert!(matches!(mapped, LoadOutcome::Loaded(ref s) if s == "42"));

        let recovered: LoadOutcome<i32> = LoadOutcome::Recovered(7);
        let mapped = recovered.map(|v| v * 2);
        assert!(matches!(mapped, LoadOutcome::Recovered(14)));

        let unsupported: LoadOutcome<i32> =
            LoadOutcome::UnsupportedVersion { found: 5, max_supported: 1 };
        let mapped = unsupported.map(|v| v + 1);
        assert!(matches!(mapped, LoadOutcome::UnsupportedVersion { found: 5, max_supported: 1 }));
    }

    // ── Workspaces ──────────────────────────────────────────

    #[test]
    fn workspaces_round_trip_through_store() {
        let (_tmp, store) = test_store();
        let ws = models::workspaces::WorkspaceStore {
            active_workspace_id: Some("ws-1".into()),
            workspaces: vec![models::workspaces::WorkspaceRecord {
                id: "ws-1".into(),
                name: "Editor".into(),
                user_renamed: true,
                endpoint_key: "local".into(),
                policy: models::workspaces::WorkspacePolicy::Persistent,
                runtime_ref: Some(models::workspaces::RuntimeRef {
                    runtime_id: "rt-1".into(),
                    attachment_kind: models::workspaces::RuntimeAttachmentKind::Created,
                }),
                layout: models::workspaces::LayoutNode::Terminal {
                    uuid: "t-1".into(),
                    profile: None,
                    cwd: Some("/home/user".into()),
                    custom_title: Some("vim".into()),
                },
                active_pane_id: Some("t-1".into()),
                zoomed_pane_id: None,
                input_sync: models::workspaces::InputSyncState::Off,
                color: models::workspaces::WorkspaceColor::Green,
                pane_recovery: std::collections::BTreeMap::new(),
            }],
        };
        store.save_workspaces(&ws).unwrap();
        let loaded = store.load_workspaces().into_value().unwrap();
        assert_eq!(loaded.active_workspace_id, Some("ws-1".into()));
        assert_eq!(loaded.workspaces.len(), 1);
        assert_eq!(loaded.workspaces[0].name, "Editor");
        assert!(loaded.workspaces[0].user_renamed);
        assert_eq!(loaded.workspaces[0].runtime_ref.as_ref().unwrap().runtime_id, "rt-1");
    }

    #[test]
    fn workspaces_missing_file_returns_default() {
        let (_tmp, store) = test_store();
        let outcome = store.load_workspaces();
        assert!(matches!(outcome, LoadOutcome::Default(_)));
        let ws = outcome.into_value().unwrap();
        assert!(ws.workspaces.is_empty());
    }

    #[test]
    fn workspaces_malformed_file_recovers_to_default() {
        let (_tmp, store) = test_store();
        let path = store.paths.state().join("workspaces.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "bad json").unwrap();
        let outcome = store.load_workspaces();
        assert!(matches!(outcome, LoadOutcome::DefaultAfterFailure(_)));
    }

    // ── UI State ────────────────────────────────────────────

    #[test]
    fn ui_state_round_trip_through_store() {
        let (_tmp, store) = test_store();
        let ui = models::ui::UiState {
            window_width: 1920,
            window_height: 1080,
            is_maximized: true,
            left_sidebar_width: 300,
            right_sidebar_width: 400,
            ..models::ui::UiState::default()
        };
        store.save_ui_state(&ui).unwrap();
        let loaded = store.load_ui_state().into_value().unwrap();
        assert_eq!(loaded.window_width, 1920);
        assert_eq!(loaded.window_height, 1080);
        assert!(loaded.is_maximized);
        assert_eq!(loaded.left_sidebar_width, 300);
        assert_eq!(loaded.right_sidebar_width, 400);
    }

    #[test]
    fn ui_state_missing_file_returns_default() {
        let (_tmp, store) = test_store();
        let outcome = store.load_ui_state();
        assert!(matches!(outcome, LoadOutcome::Default(_)));
        let ui = outcome.into_value().unwrap();
        assert_eq!(ui.window_width, 900);
        assert_eq!(ui.window_height, 600);
    }

    #[test]
    fn ui_state_malformed_file_recovers_to_default() {
        let (_tmp, store) = test_store();
        let path = store.paths.state().join("ui.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        let outcome = store.load_ui_state();
        assert!(matches!(outcome, LoadOutcome::DefaultAfterFailure(_)));
    }

    // ── Runtime Cache ───────────────────────────────────────

    #[test]
    fn runtime_cache_round_trip_through_store() {
        let (_tmp, store) = test_store();
        let cache = models::runtime_cache::RuntimeCache {
            dismissed_runtime_ids: ["rt-1".into(), "rt-2".into()].into_iter().collect(),
        };
        store.save_runtime_cache(&cache).unwrap();
        let loaded = store.load_runtime_cache().into_value().unwrap();
        assert_eq!(loaded.dismissed_runtime_ids.len(), 2);
        assert!(loaded.dismissed_runtime_ids.contains("rt-1"));
        assert!(loaded.dismissed_runtime_ids.contains("rt-2"));
    }

    #[test]
    fn runtime_cache_missing_file_returns_default() {
        let (_tmp, store) = test_store();
        let outcome = store.load_runtime_cache();
        assert!(matches!(outcome, LoadOutcome::Default(_)));
        assert!(outcome.into_value().unwrap().dismissed_runtime_ids.is_empty());
    }

    #[test]
    fn runtime_cache_deletion_is_non_fatal() {
        let (_tmp, store) = test_store();
        let cache = models::runtime_cache::RuntimeCache {
            dismissed_runtime_ids: std::iter::once("rt-1".into()).collect(),
        };
        store.save_runtime_cache(&cache).unwrap();
        // Delete the file
        let path = store.paths.cache().join("runtime-cache.json");
        std::fs::remove_file(&path).unwrap();
        // Load should return default, not error
        let outcome = store.load_runtime_cache();
        assert!(matches!(outcome, LoadOutcome::Default(_)));
        assert!(outcome.into_value().unwrap().dismissed_runtime_ids.is_empty());
    }

    // ── Domain conversion round-trips ───────────────────────

    #[test]
    fn workspace_state_round_trips_through_store_model() {
        use crate::workspace::state::WorkspaceState;

        let ws = WorkspaceState::new("Editor".into());
        let record: models::workspaces::WorkspaceRecord = (&ws).into();
        let restored = record.to_workspace_state();

        assert_eq!(restored.uuid, ws.uuid);
        assert_eq!(restored.name, ws.name);
        assert_eq!(restored.layout.terminal_uuids(), ws.layout.terminal_uuids());
    }

    #[test]
    fn managed_workspace_round_trips_through_store_model() {
        use crate::runtime::{RuntimeEndpoint, WorkspacePolicy};
        use crate::workspace::state::WorkspaceState;

        let ws = WorkspaceState::new_managed_remote(
            "Remote".into(),
            "deploy@example.com",
            WorkspacePolicy::Persistent,
            Some("/srv/app".into()),
        );
        let record: models::workspaces::WorkspaceRecord = (&ws).into();

        assert_eq!(record.endpoint_key, "example.com");
        assert!(record.runtime_ref.is_none()); // no runtime_id yet
        assert_eq!(record.policy, models::workspaces::WorkspacePolicy::Persistent);

        let restored = record.to_workspace_state();
        assert!(restored.runtime.is_managed());
        assert_eq!(
            restored.runtime.endpoint,
            RuntimeEndpoint::Remote { host: "example.com".into() }
        );
    }

    #[test]
    fn window_state_splits_into_three_store_documents() {
        use crate::workspace::state::WindowState;

        let mut state = WindowState {
            width: 1920,
            height: 1080,
            is_maximized: true,
            left_sidebar_width: 250,
            right_sidebar_width: 350,
            ..WindowState::default()
        };
        state.dismissed_runtime_ids.insert("dismissed-1".into());

        let ws_store: models::workspaces::WorkspaceStore = (&state).into();
        let ui: models::ui::UiState = (&state).into();
        let cache: models::runtime_cache::RuntimeCache = (&state).into();

        assert_eq!(ws_store.workspaces.len(), 1);
        assert_eq!(ui.window_width, 1920);
        assert_eq!(ui.window_height, 1080);
        assert!(ui.is_maximized);
        assert_eq!(ui.left_sidebar_width, 250);
        assert_eq!(ui.right_sidebar_width, 350);
        assert!(cache.dismissed_runtime_ids.contains("dismissed-1"));
    }

    #[test]
    fn workspace_with_recovery_round_trips_through_store() {
        use crate::workspace::recovery::{PaneRecovery, PaneSource, PaneTarget, StartupStep};
        use crate::workspace::state::WorkspaceState;

        let mut ws = WorkspaceState::new("Ops".into());
        let terminal_uuid = ws.layout.terminal_uuids().into_iter().next().unwrap();
        ws.set_recovery(
            &terminal_uuid,
            PaneRecovery {
                source: PaneSource::Command { title: "Deploy".into() },
                target: Some(PaneTarget::RemoteShell {
                    ssh_target: "deploy@prod".into(),
                    remote_folder: Some("/srv/app".into()),
                }),
                startup: vec![StartupStep::SendText {
                    text: "make deploy".into(),
                    execute: true,
                }],
            },
        );

        let record: models::workspaces::WorkspaceRecord = (&ws).into();
        let restored = record.to_workspace_state();

        let recovery = restored.recovery_for(&terminal_uuid).unwrap();
        assert!(matches!(recovery.source, PaneSource::Command { ref title } if title == "Deploy"));
        assert!(matches!(
            recovery.target,
            Some(PaneTarget::RemoteShell { ref ssh_target, .. }) if ssh_target == "deploy@prod"
        ));
        assert_eq!(recovery.startup.len(), 1);
    }

    #[test]
    fn split_layout_round_trips_through_store_model() {
        use crate::test_helpers::{hsplit, term};
        use crate::workspace::state::WorkspaceState;

        let mut ws = WorkspaceState::new("Split".into());
        ws.layout = hsplit(term("t1"), term("t2"));

        let record: models::workspaces::WorkspaceRecord = (&ws).into();
        let restored = record.to_workspace_state();

        assert_eq!(restored.layout.terminal_count(), 2);
        assert!(restored.layout.contains_terminal("t1"));
        assert!(restored.layout.contains_terminal("t2"));
    }
}
