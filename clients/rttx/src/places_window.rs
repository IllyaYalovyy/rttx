use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::form_dialog::FormDialog;
use crate::places::{self, Place};
use crate::window::Window;

pub fn show_form(parent: &Window, place: Option<&Place>) {
    let existing_uuid = place.map(|p| p.uuid.clone());

    let form = FormDialog::new("Place", place.is_some(), 480);

    let name_row = adw::EntryRow::builder().title("Name").build();
    let path_row = adw::EntryRow::builder().title("Path").build();
    let host_tags_row =
        adw::EntryRow::builder().title("Host tags (comma-separated, empty = global)").build();

    let group = adw::PreferencesGroup::new();
    group.add(&name_row);
    group.add(&path_row);
    group.add(&host_tags_row);

    form.content_box.append(&group);
    form.finish_layout();

    if let Some(p) = place {
        name_row.set_text(&p.name);
        path_row.set_text(&p.path);
        host_tags_row.set_text(&p.host_tags.join(", "));
    }

    let dialog = form.dialog.clone();
    let status_label = form.status_label.clone();
    let parent_for_save = parent.clone();
    form.save_button.connect_clicked(move |_| {
        let place = match build_place(&name_row, &path_row, &host_tags_row, existing_uuid.clone()) {
            Ok(p) => p,
            Err(msg) => {
                status_label.set_text(&msg);
                return;
            }
        };

        let mut items = places::load();
        if let Some(existing) = items.iter_mut().find(|i| i.uuid == place.uuid) {
            *existing = place;
        } else {
            items.push(place);
        }
        if let Err(e) = places::save(&items) {
            status_label.set_text(&format!("Failed to save: {e}"));
            return;
        }
        parent_for_save.refresh_place_sidebar();
        dialog.close();
    });

    form.present(parent);
}

fn build_place(
    name_row: &adw::EntryRow,
    path_row: &adw::EntryRow,
    host_tags_row: &adw::EntryRow,
    existing_uuid: Option<String>,
) -> Result<Place, String> {
    let name = name_row.text().trim().to_string();
    if name.is_empty() {
        return Err("Place name is required".into());
    }

    let path = path_row.text().trim().to_string();
    if path.is_empty() {
        return Err("Place path is required".into());
    }

    let mut place = Place::new(&name, &path);
    if let Some(uuid) = existing_uuid {
        place.uuid = uuid;
    }
    place.host_tags = crate::commands_window::parse_host_tags(&host_tags_row.text());
    Ok(place)
}
