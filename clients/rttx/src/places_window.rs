use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::places::{self, Place};
use crate::window::Window;

pub fn show_form(parent: &Window, place: Option<&Place>) {
    let existing_uuid = place.map(|p| p.uuid.clone());

    let dialog = adw::Dialog::builder()
        .title(if place.is_some() { "Edit Place" } else { "New Place" })
        .content_width(440)
        .build();

    let header = adw::HeaderBar::new();
    let save_button = gtk4::Button::with_label(if place.is_some() { "Save" } else { "Add" });
    save_button.add_css_class("suggested-action");
    header.pack_end(&save_button);

    let name_row = adw::EntryRow::builder().title("Name").build();
    let path_row = adw::EntryRow::builder().title("Path").build();

    let status_label = gtk4::Label::new(None);
    status_label.set_xalign(0.0);
    status_label.add_css_class("dim-label");

    let group = adw::PreferencesGroup::new();
    group.add(&name_row);
    group.add(&path_row);

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

    if let Some(p) = place {
        name_row.set_text(&p.name);
        path_row.set_text(&p.path);
    }

    let dialog_for_save = dialog.clone();
    let parent_for_save = parent.clone();
    save_button.connect_clicked(move |_| {
        let p = match build_place(&name_row, &path_row, existing_uuid.clone()) {
            Ok(p) => p,
            Err(msg) => {
                status_label.set_text(&msg);
                return;
            }
        };

        let mut items = places::load();
        if let Some(existing) = items.iter_mut().find(|i| i.uuid == p.uuid) {
            *existing = p;
        } else {
            items.push(p);
        }
        if let Err(e) = places::save(&items) {
            status_label.set_text(&format!("Failed to save: {e}"));
            return;
        }
        parent_for_save.refresh_place_sidebar();
        dialog_for_save.close();
    });

    dialog.present(Some(parent));
}

fn build_place(
    name_row: &adw::EntryRow,
    path_row: &adw::EntryRow,
    existing_uuid: Option<String>,
) -> Result<Place, String> {
    let path = path_row.text().trim().to_string();
    if path.is_empty() {
        return Err("Path is required".into());
    }

    let name = name_row.text().trim().to_string();
    let mut place = Place::new(name, path);
    if let Some(uuid) = existing_uuid {
        place.uuid = uuid;
    }

    Ok(place)
}
