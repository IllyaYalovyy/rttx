use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use rttx_proto::v3;

use crate::host::Host;
use crate::window::Window;

pub const DIALOG_CONTENT_WIDTH: i32 = 400;
pub const DIALOG_CONTENT_HEIGHT: i32 = 450;
pub const SCROLL_MIN_CONTENT_HEIGHT: i32 = 300;

/// Classification of a daemon session for the Connect to Existing dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeAvailability {
    /// Can be attached by this client.
    Available,
    /// Already open in this client window.
    AlreadyOpen,
    /// Owned by another client.
    BusyElsewhere,
}

/// A session entry for display in the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeEntry {
    pub id: String,
    pub name: String,
    pub pane_count: u32,
    pub availability: RuntimeAvailability,
    pub status_label: String,
    /// The daemon reports this workspace's write lease as seizable by us.
    pub takeover_eligible: bool,
}

/// Whether the dialog should offer a "Take over" action for an entry.
///
/// Only a workspace held by another client can be seized, and only when the
/// daemon says so — it gates on policy and on the negotiated takeover
/// capability, so the client never has to guess.
#[must_use]
pub fn offers_takeover(entry: &RuntimeEntry) -> bool {
    entry.availability == RuntimeAvailability::BusyElsewhere && entry.takeover_eligible
}

/// Classify daemon workspaces into available/busy entries.
///
/// `open_runtime_ids` contains runtime IDs already attached by this client.
#[must_use]
pub fn classify_workspaces(
    workspaces: &[v3::WorkspaceInfo],
    open_runtime_ids: &[String],
) -> Vec<RuntimeEntry> {
    workspaces
        .iter()
        .filter_map(|info| {
            let id = rttx_proto::bytes_to_uuid(&info.id).ok()?.to_string();
            let availability = if open_runtime_ids.contains(&id) {
                RuntimeAvailability::AlreadyOpen
            } else if info.has_write_owner {
                RuntimeAvailability::BusyElsewhere
            } else {
                RuntimeAvailability::Available
            };
            let status_label = match &availability {
                RuntimeAvailability::Available => format!(
                    "{} {}",
                    info.pane_count,
                    if info.pane_count == 1 { "pane" } else { "panes" }
                ),
                RuntimeAvailability::AlreadyOpen => "Already open".into(),
                RuntimeAvailability::BusyElsewhere => "Connected elsewhere".into(),
            };
            Some(RuntimeEntry {
                id,
                name: info.name.clone(),
                pane_count: info.pane_count,
                availability,
                status_label,
                takeover_eligible: info.takeover_eligible,
            })
        })
        .collect()
}

/// Whether a session entry matches a search query (case-insensitive).
#[must_use]
pub fn matches_query(entry: &RuntimeEntry, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    entry.name.to_lowercase().contains(&query)
}

/// Show the Connect to Existing dialog for a specific host.
pub fn show(window: &Window, host: &Host, workspaces: &[v3::WorkspaceInfo]) {
    let title = format!("Connect to Existing: {}", host.name);
    let dialog = adw::Dialog::builder()
        .title(&title)
        .content_width(DIALOG_CONTENT_WIDTH)
        .content_height(DIALOG_CONTENT_HEIGHT)
        .build();

    let header = adw::HeaderBar::new();

    let search_entry = gtk4::SearchEntry::new();
    search_entry.set_placeholder_text(Some("Search workspaces…"));
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
        .min_content_height(SCROLL_MIN_CONTENT_HEIGHT)
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
    let entries = classify_workspaces(workspaces, &open_runtime_ids);

    populate_workspaces(&list_box, &entries, "", window, host, &dialog);

    let win_for_search = window.clone();
    let host_for_search = host.clone();
    let dialog_for_search = dialog.clone();
    search_entry.connect_changed(move |entry| {
        let query = entry.text().to_string();
        populate_workspaces(
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

fn populate_workspaces(
    container: &gtk4::Box,
    entries: &[RuntimeEntry],
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

        let is_available = entry.availability == RuntimeAvailability::Available;
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
                win.attach_to_existing_runtime(&host_clone, &runtime_id, false);
            });
        }

        if offers_takeover(entry) {
            let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
            button.set_hexpand(true);
            row.append(&button);
            row.append(&takeover_button(entry, window, host, dialog));
            container.append(&row);
        } else {
            container.append(&button);
        }
    }

    if !has_available && !has_busy {
        let empty = gtk4::Label::new(Some("No sessions found"));
        empty.add_css_class("dim-label");
        empty.set_margin_top(24);
        empty.set_margin_bottom(24);
        container.append(&empty);
    }
}

fn takeover_button(
    entry: &RuntimeEntry,
    window: &Window,
    host: &Host,
    dialog: &adw::Dialog,
) -> gtk4::Button {
    let button = gtk4::Button::with_label("Take over");
    button.add_css_class("flat");
    button.set_valign(gtk4::Align::Center);
    button.set_tooltip_text(Some("Disconnect the other client and take control"));
    button.update_property(&[gtk4::accessible::Property::Label(&format!(
        "Take over {}",
        entry.name
    ))]);

    let win = window.clone();
    let host = host.clone();
    let dialog = dialog.clone();
    let runtime_id = entry.id.clone();
    let name = entry.name.clone();
    button.connect_clicked(move |_| {
        let confirm = adw::AlertDialog::new(
            Some(&format!("Take over “{name}”?")),
            Some("The client using this workspace becomes read-only and loses input."),
        );
        confirm.add_response("cancel", "Cancel");
        confirm.add_response("takeover", "Take Over");
        confirm.set_response_appearance("takeover", adw::ResponseAppearance::Destructive);
        confirm.set_default_response(Some("cancel"));
        confirm.set_close_response("cancel");

        let confirmed_win = win.clone();
        let host = host.clone();
        let dialog = dialog.clone();
        let runtime_id = runtime_id.clone();
        confirm.connect_response(None, move |_, response| {
            if response == "takeover" {
                dialog.close();
                confirmed_win.attach_to_existing_runtime(&host, &runtime_id, true);
            }
        });
        confirm.present(Some(&win));
    });

    button
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
    ) -> v3::WorkspaceInfo {
        make_session_info_with_takeover(id, name, pane_count, has_write_owner, false)
    }

    fn make_session_info_with_takeover(
        id: uuid::Uuid,
        name: &str,
        pane_count: u32,
        has_write_owner: bool,
        takeover_eligible: bool,
    ) -> v3::WorkspaceInfo {
        v3::WorkspaceInfo {
            id: rttx_proto::uuid_to_bytes(id),
            name: name.into(),
            pane_count,
            has_write_owner,
            policy: 0,
            read_only_client_count: 0,
            current_client_role: 0,
            workspace_revision: 1,
            reconstructed: false,
            user_renamed: false,
            active_pane_summary: String::new(),
            takeover_eligible,
            disabled_reason: String::new(),
            panes: vec![],
        }
    }

    #[test]
    fn classify_available_session() {
        let id = uuid::Uuid::new_v4();
        let workspaces = vec![make_session_info(id, "workspace-1", 2, false)];
        let entries = classify_workspaces(&workspaces, &[]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].availability, RuntimeAvailability::Available);
        assert_eq!(entries[0].name, "workspace-1");
        assert_eq!(entries[0].pane_count, 2);
        assert_eq!(entries[0].status_label, "2 panes");
    }

    #[test]
    fn classify_busy_session_with_write_owner() {
        let id = uuid::Uuid::new_v4();
        let workspaces = vec![make_session_info(id, "busy-ws", 1, true)];
        let entries = classify_workspaces(&workspaces, &[]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].availability, RuntimeAvailability::BusyElsewhere);
        assert_eq!(entries[0].status_label, "Connected elsewhere");
    }

    #[test]
    fn classify_already_open_session() {
        let id = uuid::Uuid::new_v4();
        let workspaces = vec![make_session_info(id, "open-ws", 3, false)];
        let entries = classify_workspaces(&workspaces, &[id.to_string()]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].availability, RuntimeAvailability::AlreadyOpen);
        assert_eq!(entries[0].status_label, "Already open");
    }

    #[test]
    fn classify_already_open_takes_precedence_over_busy() {
        let id = uuid::Uuid::new_v4();
        let workspaces = vec![make_session_info(id, "mine", 1, true)];
        let entries = classify_workspaces(&workspaces, &[id.to_string()]);

        assert_eq!(entries[0].availability, RuntimeAvailability::AlreadyOpen);
    }

    #[test]
    fn classify_mixed_sessions_preserves_order() {
        let avail_id = uuid::Uuid::new_v4();
        let busy_id = uuid::Uuid::new_v4();
        let open_id = uuid::Uuid::new_v4();
        let workspaces = vec![
            make_session_info(avail_id, "avail", 1, false),
            make_session_info(busy_id, "busy", 2, true),
            make_session_info(open_id, "open", 1, false),
        ];
        let entries = classify_workspaces(&workspaces, &[open_id.to_string()]);

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].availability, RuntimeAvailability::Available);
        assert_eq!(entries[1].availability, RuntimeAvailability::BusyElsewhere);
        assert_eq!(entries[2].availability, RuntimeAvailability::AlreadyOpen);
    }

    #[test]
    fn classify_empty_sessions() {
        let entries = classify_workspaces(&[], &[]);
        assert!(entries.is_empty());
    }

    #[test]
    fn single_pane_label_is_singular() {
        let id = uuid::Uuid::new_v4();
        let workspaces = vec![make_session_info(id, "ws", 1, false)];
        let entries = classify_workspaces(&workspaces, &[]);
        assert_eq!(entries[0].status_label, "1 pane");
    }

    #[test]
    fn matches_query_empty_matches_all() {
        let entry = RuntimeEntry {
            id: "id".into(),
            name: "anything".into(),
            pane_count: 1,
            availability: RuntimeAvailability::Available,
            status_label: "1 pane".into(),
            takeover_eligible: false,
        };
        assert!(matches_query(&entry, ""));
        assert!(matches_query(&entry, "  "));
    }

    #[test]
    fn matches_query_case_insensitive() {
        let entry = RuntimeEntry {
            id: "id".into(),
            name: "My Workspace".into(),
            pane_count: 1,
            availability: RuntimeAvailability::Available,
            status_label: "1 pane".into(),
            takeover_eligible: false,
        };
        assert!(matches_query(&entry, "my"));
        assert!(matches_query(&entry, "WORKSPACE"));
        assert!(matches_query(&entry, "My Work"));
    }

    #[test]
    fn busy_session_offers_takeover_when_the_daemon_says_it_is_eligible() {
        let id = uuid::Uuid::new_v4();
        let workspaces = vec![make_session_info_with_takeover(id, "busy-ws", 1, true, true)];
        let entries = classify_workspaces(&workspaces, &[]);

        assert_eq!(entries[0].availability, RuntimeAvailability::BusyElsewhere);
        assert!(entries[0].takeover_eligible);
        assert!(offers_takeover(&entries[0]));
    }

    #[test]
    fn busy_session_without_eligibility_offers_no_takeover() {
        let id = uuid::Uuid::new_v4();
        let workspaces = vec![make_session_info_with_takeover(id, "busy-ws", 1, true, false)];
        let entries = classify_workspaces(&workspaces, &[]);

        assert_eq!(entries[0].availability, RuntimeAvailability::BusyElsewhere);
        assert!(!offers_takeover(&entries[0]));
    }

    #[test]
    fn available_session_offers_no_takeover() {
        let id = uuid::Uuid::new_v4();
        // A daemon that reports an unowned workspace as eligible would still
        // not warrant the action: a plain attach already works.
        let workspaces = vec![make_session_info_with_takeover(id, "free-ws", 1, false, true)];
        let entries = classify_workspaces(&workspaces, &[]);

        assert_eq!(entries[0].availability, RuntimeAvailability::Available);
        assert!(!offers_takeover(&entries[0]));
    }

    #[test]
    fn already_open_session_offers_no_takeover() {
        let id = uuid::Uuid::new_v4();
        let workspaces = vec![make_session_info_with_takeover(id, "mine", 1, true, true)];
        let entries = classify_workspaces(&workspaces, &[id.to_string()]);

        assert_eq!(entries[0].availability, RuntimeAvailability::AlreadyOpen);
        assert!(!offers_takeover(&entries[0]));
    }

    #[test]
    fn matches_query_no_match() {
        let entry = RuntimeEntry {
            id: "id".into(),
            name: "rttx".into(),
            pane_count: 1,
            availability: RuntimeAvailability::Available,
            status_label: "1 pane".into(),
            takeover_eligible: false,
        };
        assert!(!matches_query(&entry, "redis"));
    }
}
