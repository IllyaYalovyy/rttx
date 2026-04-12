use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use rttx_proto::proto;

use crate::host::Host;
use crate::window::Window;

/// Classification of a daemon session for the Connect to Existing dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionAvailability {
    /// Can be attached by this client.
    Available,
    /// Already open in this client window.
    AlreadyOpen,
    /// Owned by another client.
    BusyElsewhere,
}

/// A session entry for display in the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    pub id: String,
    pub name: String,
    pub pane_count: u32,
    pub availability: SessionAvailability,
    pub status_label: String,
}

/// Classify daemon sessions into available/busy entries.
///
/// `open_runtime_ids` contains runtime IDs already attached by this client.
#[must_use]
pub fn classify_sessions(
    sessions: &[proto::SessionInfo],
    open_runtime_ids: &[String],
) -> Vec<SessionEntry> {
    sessions
        .iter()
        .filter_map(|info| {
            let id = rttx_proto::bytes_to_uuid(&info.id).ok()?.to_string();
            let availability = if open_runtime_ids.contains(&id) {
                SessionAvailability::AlreadyOpen
            } else if info.has_write_owner {
                SessionAvailability::BusyElsewhere
            } else {
                SessionAvailability::Available
            };
            let status_label = match &availability {
                SessionAvailability::Available => format!(
                    "{} {}",
                    info.pane_count,
                    if info.pane_count == 1 { "pane" } else { "panes" }
                ),
                SessionAvailability::AlreadyOpen => "Already open".into(),
                SessionAvailability::BusyElsewhere => "Connected elsewhere".into(),
            };
            Some(SessionEntry {
                id,
                name: info.name.clone(),
                pane_count: info.pane_count,
                availability,
                status_label,
            })
        })
        .collect()
}

/// Whether a session entry matches a search query (case-insensitive).
#[must_use]
pub fn matches_query(entry: &SessionEntry, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    entry.name.to_lowercase().contains(&query)
}

/// Show the Connect to Existing dialog for a specific host.
pub fn show(window: &Window, host: &Host, sessions: &[proto::SessionInfo]) {
    let title = format!("Connect to Existing: {}", host.name);
    let dialog = adw::Dialog::builder().title(&title).content_width(400).build();

    let header = adw::HeaderBar::new();

    let search_entry = gtk4::SearchEntry::new();
    search_entry.set_placeholder_text(Some("Search sessions…"));
    search_entry.set_margin_start(18);
    search_entry.set_margin_end(18);
    search_entry.set_margin_top(12);

    let list_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    list_box.set_margin_start(18);
    list_box.set_margin_end(18);
    list_box.set_margin_top(12);
    list_box.set_margin_bottom(18);
    list_box.update_property(&[gtk4::accessible::Property::Label("Sessions")]);

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .min_content_height(200)
        .child(&list_box)
        .build();

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content.append(&search_entry);
    content.append(&scroll);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&content));
    dialog.set_child(Some(&toolbar_view));

    let open_runtime_ids = window.open_runtime_ids_for_endpoint(host);
    let entries = classify_sessions(sessions, &open_runtime_ids);

    populate_sessions(&list_box, &entries, "", window, host, &dialog);

    let win_for_search = window.clone();
    let host_for_search = host.clone();
    let dialog_for_search = dialog.clone();
    search_entry.connect_changed(move |entry| {
        let query = entry.text().to_string();
        populate_sessions(
            &list_box,
            &entries,
            &query,
            &win_for_search,
            &host_for_search,
            &dialog_for_search,
        );
    });

    dialog.present(Some(window));
    search_entry.grab_focus();
}

fn populate_sessions(
    container: &gtk4::Box,
    entries: &[SessionEntry],
    query: &str,
    window: &Window,
    host: &Host,
    dialog: &adw::Dialog,
) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let mut has_available = false;
    let mut has_busy = false;

    for entry in entries {
        if !matches_query(entry, query) {
            continue;
        }

        let is_available = entry.availability == SessionAvailability::Available;
        if is_available && !has_available {
            has_available = true;
            append_section_label(container, "Available");
        } else if !is_available && !has_busy {
            has_busy = true;
            append_section_label(container, "Busy");
        }

        let icon_name =
            if is_available { "media-playback-start-symbolic" } else { "changes-prevent-symbolic" };
        let icon = gtk4::Image::from_icon_name(icon_name);
        icon.set_margin_end(8);
        if !is_available {
            icon.add_css_class("dim-label");
        }

        let title_label = gtk4::Label::new(Some(&entry.name));
        title_label.set_xalign(0.0);
        title_label.set_hexpand(true);
        title_label.add_css_class("body");
        if !is_available {
            title_label.add_css_class("dim-label");
        }

        let status = gtk4::Label::new(Some(&entry.status_label));
        status.set_xalign(1.0);
        status.add_css_class("dim-label");
        status.add_css_class("caption");

        let row_content = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        row_content.set_margin_start(12);
        row_content.set_margin_end(12);
        row_content.set_margin_top(8);
        row_content.set_margin_bottom(8);
        row_content.append(&icon);
        row_content.append(&title_label);
        row_content.append(&status);

        let button = gtk4::Button::new();
        button.set_child(Some(&row_content));
        button.add_css_class("flat");
        button.set_sensitive(is_available);
        button.update_property(&[gtk4::accessible::Property::Label(&entry.name)]);

        if is_available {
            let win = window.clone();
            let host_clone = host.clone();
            let dialog_ref = dialog.clone();
            let runtime_id = entry.id.clone();
            button.connect_clicked(move |_| {
                dialog_ref.close();
                win.attach_to_existing_runtime(&host_clone, &runtime_id);
            });
        }

        container.append(&button);
    }

    if !has_available && !has_busy {
        let empty = gtk4::Label::new(Some("No sessions found"));
        empty.add_css_class("dim-label");
        empty.set_margin_top(24);
        empty.set_margin_bottom(24);
        container.append(&empty);
    }
}

fn append_section_label(container: &gtk4::Box, text: &str) {
    let label = gtk4::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("dim-label");
    label.add_css_class("caption");
    label.set_margin_start(6);
    label.set_margin_top(8);
    label.set_margin_bottom(2);
    container.append(&label);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session_info(
        id: uuid::Uuid,
        name: &str,
        pane_count: u32,
        has_write_owner: bool,
    ) -> proto::SessionInfo {
        proto::SessionInfo {
            id: rttx_proto::uuid_to_bytes(id),
            name: name.into(),
            pane_count,
            has_attached_client: has_write_owner,
            active_pane_id: None,
            panes: vec![],
            policy: 0,
            attached_client_count: u32::from(has_write_owner),
            reconstructed: false,
            revision: 1,
            current_client_role: 0,
            has_write_owner,
            read_only_client_count: 0,
        }
    }

    #[test]
    fn classify_available_session() {
        let id = uuid::Uuid::new_v4();
        let sessions = vec![make_session_info(id, "workspace-1", 2, false)];
        let entries = classify_sessions(&sessions, &[]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].availability, SessionAvailability::Available);
        assert_eq!(entries[0].name, "workspace-1");
        assert_eq!(entries[0].pane_count, 2);
        assert_eq!(entries[0].status_label, "2 panes");
    }

    #[test]
    fn classify_busy_session_with_write_owner() {
        let id = uuid::Uuid::new_v4();
        let sessions = vec![make_session_info(id, "busy-ws", 1, true)];
        let entries = classify_sessions(&sessions, &[]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].availability, SessionAvailability::BusyElsewhere);
        assert_eq!(entries[0].status_label, "Connected elsewhere");
    }

    #[test]
    fn classify_already_open_session() {
        let id = uuid::Uuid::new_v4();
        let sessions = vec![make_session_info(id, "open-ws", 3, false)];
        let entries = classify_sessions(&sessions, &[id.to_string()]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].availability, SessionAvailability::AlreadyOpen);
        assert_eq!(entries[0].status_label, "Already open");
    }

    #[test]
    fn classify_already_open_takes_precedence_over_busy() {
        let id = uuid::Uuid::new_v4();
        let sessions = vec![make_session_info(id, "mine", 1, true)];
        let entries = classify_sessions(&sessions, &[id.to_string()]);

        assert_eq!(entries[0].availability, SessionAvailability::AlreadyOpen);
    }

    #[test]
    fn classify_mixed_sessions_preserves_order() {
        let avail_id = uuid::Uuid::new_v4();
        let busy_id = uuid::Uuid::new_v4();
        let open_id = uuid::Uuid::new_v4();
        let sessions = vec![
            make_session_info(avail_id, "avail", 1, false),
            make_session_info(busy_id, "busy", 2, true),
            make_session_info(open_id, "open", 1, false),
        ];
        let entries = classify_sessions(&sessions, &[open_id.to_string()]);

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].availability, SessionAvailability::Available);
        assert_eq!(entries[1].availability, SessionAvailability::BusyElsewhere);
        assert_eq!(entries[2].availability, SessionAvailability::AlreadyOpen);
    }

    #[test]
    fn classify_empty_sessions() {
        let entries = classify_sessions(&[], &[]);
        assert!(entries.is_empty());
    }

    #[test]
    fn single_pane_label_is_singular() {
        let id = uuid::Uuid::new_v4();
        let sessions = vec![make_session_info(id, "ws", 1, false)];
        let entries = classify_sessions(&sessions, &[]);
        assert_eq!(entries[0].status_label, "1 pane");
    }

    #[test]
    fn matches_query_empty_matches_all() {
        let entry = SessionEntry {
            id: "id".into(),
            name: "anything".into(),
            pane_count: 1,
            availability: SessionAvailability::Available,
            status_label: "1 pane".into(),
        };
        assert!(matches_query(&entry, ""));
        assert!(matches_query(&entry, "  "));
    }

    #[test]
    fn matches_query_case_insensitive() {
        let entry = SessionEntry {
            id: "id".into(),
            name: "My Workspace".into(),
            pane_count: 1,
            availability: SessionAvailability::Available,
            status_label: "1 pane".into(),
        };
        assert!(matches_query(&entry, "my"));
        assert!(matches_query(&entry, "WORKSPACE"));
        assert!(matches_query(&entry, "My Work"));
    }

    #[test]
    fn matches_query_no_match() {
        let entry = SessionEntry {
            id: "id".into(),
            name: "rttx".into(),
            pane_count: 1,
            availability: SessionAvailability::Available,
            status_label: "1 pane".into(),
        };
        assert!(!matches_query(&entry, "redis"));
    }
}
