use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::bookmarks::{self, Bookmark};
use crate::form_dialog::{self, FormDialog};
use crate::window::Window;

pub fn show_form(parent: &Window, bookmark: Option<&Bookmark>) {
    let existing_uuid = bookmark.map(|b| b.uuid.clone());

    let form = FormDialog::new("Bookmark", bookmark.is_some(), 440);

    let name_row = adw::EntryRow::builder().title("Name").build();
    let directory_row = adw::EntryRow::builder().title("Directory").build();
    let ssh_target_row = adw::EntryRow::builder().title("SSH target / args").build();

    let group = adw::PreferencesGroup::new();
    group.add(&name_row);
    group.add(&directory_row);
    group.add(&ssh_target_row);

    form.content_box.append(&group);
    form.finish_layout();

    if let Some(b) = bookmark {
        name_row.set_text(&b.name);
        directory_row.set_text(b.directory.as_deref().unwrap_or_default());
        ssh_target_row.set_text(b.ssh_target.as_deref().unwrap_or_default());
    }

    let dialog = form.dialog.clone();
    let status_label = form.status_label.clone();
    let parent_for_save = parent.clone();
    form.save_button.connect_clicked(move |_| {
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
        dialog.close();
    });

    form.present(parent);
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
    bookmark.directory = form_dialog::entry_value(directory_row);
    bookmark.ssh_target = form_dialog::entry_value(ssh_target_row);

    if !bookmark.is_actionable() {
        return Err("Add a directory, SSH target, or both".into());
    }

    Ok(bookmark)
}
