use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::host;

/// A checkbox-based host selector for command and place editors.
///
/// Shows one row per known host (local + saved remotes) with a checkbox.
/// When no hosts are checked the item is global (visible everywhere).
pub struct HostTagPicker {
    pub group: adw::PreferencesGroup,
    checks: Vec<(String, gtk4::CheckButton)>,
}

impl std::fmt::Debug for HostTagPicker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let keys: Vec<&str> = self.checks.iter().map(|(k, _)| k.as_str()).collect();
        f.debug_struct("HostTagPicker")
            .field("group", &"<PreferencesGroup>")
            .field("keys", &keys)
            .finish()
    }
}

impl HostTagPicker {
    /// Build a picker from saved hosts, pre-checking `selected_tags`.
    #[must_use]
    pub fn new(selected_tags: &[String]) -> Self {
        let hosts = crate::store::default_store()
            .load_hosts()
            .into_value()
            .unwrap_or_default();
        Self::with_hosts(&hosts, selected_tags)
    }

    /// Build a picker from an explicit host list, pre-checking `selected_tags`.
    #[must_use]
    pub fn with_hosts(saved_hosts: &[host::Host], selected_tags: &[String]) -> Self {
        let group = adw::PreferencesGroup::builder()
            .title("Hosts")
            .description("No selection means global (visible on all hosts)")
            .build();

        let mut checks: Vec<(String, gtk4::CheckButton)> = Vec::new();

        // Local host — always first
        let local_check = Self::add_row(&group, host::LOCAL_KEY, "Local");
        local_check.set_active(selected_tags.contains(&host::LOCAL_KEY.to_string()));
        checks.push((host::LOCAL_KEY.into(), local_check));

        for h in saved_hosts {
            if h.key == host::LOCAL_KEY {
                continue;
            }
            let check = Self::add_row(&group, &h.key, &h.name);
            check.set_active(selected_tags.contains(&h.key));
            checks.push((h.key.clone(), check));
        }

        Self { group, checks }
    }

    /// Collect the host keys whose checkboxes are active.
    #[must_use]
    pub fn selected_tags(&self) -> Vec<String> {
        self.checks
            .iter()
            .filter(|(_, check)| check.is_active())
            .map(|(key, _)| key.clone())
            .collect()
    }

    fn add_row(group: &adw::PreferencesGroup, key: &str, label: &str) -> gtk4::CheckButton {
        let check = gtk4::CheckButton::new();
        let row =
            adw::ActionRow::builder().title(label).subtitle(key).activatable_widget(&check).build();
        row.add_prefix(&check);
        group.add(&row);
        check
    }
}

/// Extract selected host tags from a list of `(key, is_active)` pairs.
///
/// Pure logic helper for testing without GTK.
#[must_use]
pub fn collect_selected(pairs: &[(String, bool)]) -> Vec<String> {
    pairs.iter().filter(|(_, active)| *active).map(|(key, _)| key.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_selected_returns_active_keys() {
        let pairs = vec![
            ("local".to_string(), true),
            ("example.com".to_string(), false),
            ("other.com".to_string(), true),
        ];
        assert_eq!(collect_selected(&pairs), vec!["local", "other.com"]);
    }

    #[test]
    fn collect_selected_empty_when_none_active() {
        let pairs = vec![("local".to_string(), false), ("example.com".to_string(), false)];
        assert!(collect_selected(&pairs).is_empty());
    }

    #[test]
    fn collect_selected_all_when_all_active() {
        let pairs = vec![("local".to_string(), true), ("example.com".to_string(), true)];
        assert_eq!(collect_selected(&pairs), vec!["local", "example.com"]);
    }

    #[test]
    fn collect_selected_empty_input() {
        assert!(collect_selected(&[]).is_empty());
    }
}
