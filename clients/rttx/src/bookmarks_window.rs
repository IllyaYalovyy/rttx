use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::bookmarks::{self, Bookmark};
use crate::window::Window;

pub fn show_form(parent: &Window, bookmark: Option<&Bookmark>) {
    let existing_uuid = bookmark.map(|b| b.uuid.clone());

    let dialog = adw::Dialog::builder()
        .title(if bookmark.is_some() { "Edit Bookmark" } else { "New Bookmark" })
        .content_width(440)
        .build();

    let header = adw::HeaderBar::new();
    let save_button = gtk4::Button::with_label(if bookmark.is_some() { "Save" } else { "Add" });
    save_button.add_css_class("suggested-action");
    header.pack_end(&save_button);

    let name_row = adw::EntryRow::builder().title("Name").build();
    let directory_row = adw::EntryRow::builder().title("Directory").build();
    let ssh_target_row = adw::EntryRow::builder().title("SSH target / args").build();

    let status_label = gtk4::Label::new(None);
    status_label.set_xalign(0.0);
    status_label.add_css_class("dim-label");

    let group = adw::PreferencesGroup::new();
    group.add(&name_row);
    group.add(&directory_row);
    group.add(&ssh_target_row);

    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content_box.set_margin_start(18);
    content_box.set_margin_end(18);
    content_box.set_margin_top(18);
    content_box.set_margin_bottom(18);
    content_box.append(&group);
    content_box.append(&status_label);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&content_box));

    dialog.set_child(Some(&toolbar_view));

    if let Some(b) = bookmark {
        name_row.set_text(&b.name);
        directory_row.set_text(b.directory.as_deref().unwrap_or_default());
        ssh_target_row.set_text(b.ssh_target.as_deref().unwrap_or_default());
    }

    let dialog_for_save = dialog.clone();
    let parent_for_save = parent.clone();
    save_button.connect_clicked(move |_| {
        let b =
            match build_bookmark(&name_row, &directory_row, &ssh_target_row, existing_uuid.clone())
            {
                Ok(b) => b,
                Err(msg) => {
                    status_label.set_text(&msg);
                    return;
                }
            };

        let mut items = bookmarks::load();
        if let Some(existing) = items.iter_mut().find(|i| i.uuid == b.uuid) {
            *existing = b;
        } else {
            items.push(b);
        }
        if let Err(e) = bookmarks::save(&items) {
            status_label.set_text(&format!("Failed to save: {e}"));
            return;
        }
        parent_for_save.refresh_bookmark_sidebar();
        dialog_for_save.close();
    });

    dialog.present(Some(parent));
}

fn build_bookmark(
    name_row: &adw::EntryRow,
    directory_row: &adw::EntryRow,
    ssh_target_row: &adw::EntryRow,
    existing_uuid: Option<String>,
) -> Result<Bookmark, String> {
    let name = name_row.text().trim().to_string();
    if name.is_empty() {
        return Err("Bookmark name is required".into());
    }

    let mut bookmark = Bookmark::new(name);
    if let Some(uuid) = existing_uuid {
        bookmark.uuid = uuid;
    }
    bookmark.directory = entry_value(directory_row);
    bookmark.ssh_target = entry_value(ssh_target_row);

    if !bookmark.is_actionable() {
        return Err("Add a directory, SSH target, or both".into());
    }

    Ok(bookmark)
}

fn entry_value(row: &adw::EntryRow) -> Option<String> {
    let text = row.text();
    let trimmed = text.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}
