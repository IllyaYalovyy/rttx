use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::host::Host;
use crate::places::{self, Place};
use crate::window::Window;

/// Show the New Workspace dialog for a specific host.
///
/// Lists built-in global places (Home, Root) plus saved places scoped to the
/// host. The user picks a place to create a new daemon-backed workspace.
pub fn show(window: &Window, host: &Host) {
    let title = format!("New Workspace: {}", host.name);
    let dialog =
        adw::Dialog::builder().title(&title).content_width(400).content_height(450).build();

    let header = adw::HeaderBar::new();

    let search_entry = gtk4::SearchEntry::new();
    search_entry.set_placeholder_text(Some("Search places…"));
    search_entry.set_margin_start(18);
    search_entry.set_margin_end(18);
    search_entry.set_margin_top(12);

    let list_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    list_box.set_margin_start(18);
    list_box.set_margin_end(18);
    list_box.set_margin_top(12);
    list_box.set_margin_bottom(18);
    list_box.update_property(&[gtk4::accessible::Property::Label("Places")]);

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .min_content_height(300)
        .child(&list_box)
        .build();

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content.append(&search_entry);
    content.append(&scroll);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&content));
    dialog.set_child(Some(&toolbar_view));

    let host_key = host.key.clone();
    populate_places(&list_box, &host_key, "", window, host, &dialog);

    let host_key_for_search = host_key;
    let win_for_search = window.clone();
    let host_for_search = host.clone();
    let dialog_for_search = dialog.clone();
    search_entry.connect_changed(move |entry| {
        let query = entry.text().to_string();
        populate_places(
            &list_box,
            &host_key_for_search,
            &query,
            &win_for_search,
            &host_for_search,
            &dialog_for_search,
        );
    });

    dialog.present(Some(window));
    search_entry.grab_focus();
}

fn populate_places(
    container: &gtk4::Box,
    host_key: &str,
    query: &str,
    window: &Window,
    host: &Host,
    dialog: &adw::Dialog,
) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let saved = crate::store::default_store().load_places();
    let visible = places::visible_for_host(&saved, host_key);

    let mut has_builtin = false;
    let mut has_saved = false;

    for place in &visible {
        if !places::matches_query(place, query) {
            continue;
        }

        let is_builtin = place.uuid.starts_with("builtin:");
        if is_builtin && !has_builtin {
            has_builtin = true;
            let label = gtk4::Label::new(Some("Suggested"));
            label.set_xalign(0.0);
            label.add_css_class("dim-label");
            label.add_css_class("caption");
            label.set_margin_start(6);
            label.set_margin_top(8);
            label.set_margin_bottom(2);
            container.append(&label);
        } else if !is_builtin && !has_saved {
            has_saved = true;
            let label = gtk4::Label::new(Some("Saved Places"));
            label.set_xalign(0.0);
            label.add_css_class("dim-label");
            label.add_css_class("caption");
            label.set_margin_start(6);
            label.set_margin_top(8);
            label.set_margin_bottom(2);
            container.append(&label);
        }

        let icon = gtk4::Image::from_icon_name(place_icon(place));
        icon.set_margin_end(8);

        let title_label = gtk4::Label::new(Some(&place.name));
        title_label.set_xalign(0.0);
        title_label.set_hexpand(true);
        title_label.add_css_class("body");

        let subtitle_label = gtk4::Label::new(Some(&place.path));
        subtitle_label.set_xalign(0.0);
        subtitle_label.add_css_class("dim-label");
        subtitle_label.add_css_class("caption");

        let text_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        text_box.append(&title_label);
        text_box.append(&subtitle_label);

        let row_content = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        row_content.set_margin_start(12);
        row_content.set_margin_end(12);
        row_content.set_margin_top(8);
        row_content.set_margin_bottom(8);
        row_content.append(&icon);
        row_content.append(&text_box);

        let button = gtk4::Button::new();
        button.set_child(Some(&row_content));
        button.add_css_class("flat");
        button.update_property(&[gtk4::accessible::Property::Label(&place.name)]);

        let win = window.clone();
        let host_clone = host.clone();
        let dialog_ref = dialog.clone();
        let path = place.path.clone();
        button.connect_clicked(move |_| {
            dialog_ref.close();
            let initial_cwd = resolve_place_path(&path);
            if host_clone.is_local() {
                win.add_managed_session_at(initial_cwd);
            } else if let Some(ssh_target) = &host_clone.ssh_target {
                win.add_remote_managed_session_at(ssh_target, initial_cwd);
            }
        });

        container.append(&button);
    }
}

fn place_icon(place: &Place) -> &'static str {
    if place.uuid == "builtin:home" {
        "user-home-symbolic"
    } else if place.uuid == "builtin:root" {
        "drive-harddisk-symbolic"
    } else {
        "folder-symbolic"
    }
}

/// Public wrapper for `resolve_place_path` used by the sidebar.
#[must_use]
pub fn resolve_place_path_public(path: &str) -> Option<String> {
    resolve_place_path(path)
}

/// Resolve a place path for use as a working directory.
///
/// Tilde prefixes are preserved so the remote shell resolves `~` on the
/// correct host.  Expanding locally would substitute the *local* home
/// directory, which does not exist on a remote machine.
fn resolve_place_path(path: &str) -> Option<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "~" {
        return None; // Home — let the shell decide
    }
    Some(trimmed.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_tilde_returns_none_for_home() {
        assert_eq!(resolve_place_path("~"), None);
    }

    #[test]
    fn resolve_empty_returns_none() {
        assert_eq!(resolve_place_path(""), None);
    }

    #[test]
    fn resolve_root_returns_root() {
        assert_eq!(resolve_place_path("/"), Some("/".into()));
    }

    #[test]
    fn resolve_absolute_path_unchanged() {
        assert_eq!(resolve_place_path("/srv/app"), Some("/srv/app".into()));
    }

    #[test]
    fn resolve_tilde_prefix_preserves_tilde() {
        assert_eq!(resolve_place_path("~/projects"), Some("~/projects".into()));
    }

    #[test]
    fn resolve_tilde_bin_preserves_tilde() {
        assert_eq!(resolve_place_path("~/bin/"), Some("~/bin/".into()));
    }

    #[test]
    fn place_icon_home_is_user_home() {
        assert_eq!(place_icon(&places::builtin_home()), "user-home-symbolic");
    }

    #[test]
    fn place_icon_root_is_drive() {
        assert_eq!(place_icon(&places::builtin_root()), "drive-harddisk-symbolic");
    }

    #[test]
    fn place_icon_saved_is_folder() {
        let place = Place::new("rttx", "~/pro/rttx");
        assert_eq!(place_icon(&place), "folder-symbolic");
    }
}
