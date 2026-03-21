use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{ActionRowExt, EntryRowExt, PreferencesRowExt};
use std::cell::RefCell;
use std::rc::Rc;

use crate::bookmarks::{self, Bookmark};
use crate::window::Window;

#[derive(Clone)]
struct EditorWidgets {
    name: adw::EntryRow,
    directory: adw::EntryRow,
    ssh_target: adw::EntryRow,
    tmux_session: adw::EntryRow,
    status: gtk4::Label,
}

pub fn show(parent: &Window) {
    let dialog = gtk4::Window::builder()
        .title("Bookmarks")
        .default_width(720)
        .default_height(560)
        .modal(true)
        .transient_for(parent)
        .build();
    if let Some(app) = parent.application() {
        dialog.set_application(Some(&app));
    }

    let bookmarks = Rc::new(RefCell::new(bookmarks::load()));
    let selected_uuid = Rc::new(RefCell::new(None::<String>));

    let header = adw::HeaderBar::new();
    let new_button = gtk4::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("New bookmark")
        .build();
    header.pack_start(&new_button);

    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::Single);
    list.add_css_class("boxed-list");

    let scrolled = gtk4::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&list)
        .build();

    let editor = EditorWidgets {
        name: adw::EntryRow::builder().title("Name").build(),
        directory: adw::EntryRow::builder().title("Directory").build(),
        ssh_target: adw::EntryRow::builder().title("SSH target / args").build(),
        tmux_session: adw::EntryRow::builder().title("Tmux session").build(),
        status: gtk4::Label::new(None),
    };
    editor.directory.set_show_apply_button(false);
    editor.ssh_target.set_show_apply_button(false);
    editor.tmux_session.set_show_apply_button(false);
    editor.status.set_xalign(0.0);
    editor.status.add_css_class("dim-label");

    let save_button = gtk4::Button::with_label("Save bookmark");
    save_button.add_css_class("suggested-action");
    let clear_button = gtk4::Button::with_label("Clear");

    let button_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    button_box.append(&save_button);
    button_box.append(&clear_button);

    let editor_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    editor_box.set_margin_start(18);
    editor_box.set_margin_end(18);
    editor_box.set_margin_top(18);
    editor_box.set_margin_bottom(18);
    editor_box.append(&editor.name);
    editor_box.append(&editor.directory);
    editor_box.append(&editor.ssh_target);
    editor_box.append(&editor.tmux_session);
    editor_box.append(&button_box);
    editor_box.append(&editor.status);

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&scrolled);
    content.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    content.append(&editor_box);
    dialog.set_child(Some(&content));

    let bookmarks_for_select = bookmarks.clone();
    let selected_for_select = selected_uuid.clone();
    let editor_for_select = editor.clone();
    list.connect_row_selected(move |_, row| {
        let Some(row) = row else {
            clear_editor(&editor_for_select, &selected_for_select);
            return;
        };
        let bookmark_uuid = row.widget_name();
        let bookmark = bookmarks_for_select
            .borrow()
            .iter()
            .find(|bookmark| bookmark.uuid == bookmark_uuid)
            .cloned();

        if let Some(bookmark) = bookmark {
            *selected_for_select.borrow_mut() = Some(bookmark.uuid.clone());
            fill_editor(&editor_for_select, &bookmark);
        }
    });

    let list_for_new = list.clone();
    let editor_for_new = editor.clone();
    let selected_for_new = selected_uuid.clone();
    new_button.connect_clicked(move |_| {
        list_for_new.unselect_all();
        clear_editor(&editor_for_new, &selected_for_new);
    });

    let bookmarks_for_save = bookmarks.clone();
    let list_for_save = list.clone();
    let editor_for_save = editor.clone();
    let selected_for_save = selected_uuid.clone();
    let parent_for_save = parent.clone();
    let dialog_for_save = dialog.clone();
    save_button.connect_clicked(move |_| {
        let bookmark = match bookmark_from_editor(&editor_for_save, selected_for_save.borrow().clone()) {
            Ok(bookmark) => bookmark,
            Err(message) => {
                editor_for_save.status.set_text(&message);
                return;
            }
        };

        {
            let mut items = bookmarks_for_save.borrow_mut();
            if let Some(existing) = items.iter_mut().find(|existing| existing.uuid == bookmark.uuid) {
                *existing = bookmark.clone();
            } else {
                items.push(bookmark.clone());
            }
            if let Err(error) = bookmarks::save(&items) {
                editor_for_save.status.set_text(&format!("Failed to save bookmarks: {error}"));
                return;
            }
        }

        *selected_for_save.borrow_mut() = Some(bookmark.uuid.clone());
        editor_for_save.status.set_text("");
        parent_for_save.refresh_bookmark_sidebar();
        rebuild_list(
            &list_for_save,
            &bookmarks_for_save,
            &selected_for_save,
            &editor_for_save,
            &parent_for_save,
            &dialog_for_save,
        );
    });

    let list_for_clear = list.clone();
    let editor_for_clear = editor.clone();
    let selected_for_clear = selected_uuid.clone();
    clear_button.connect_clicked(move |_| {
        list_for_clear.unselect_all();
        clear_editor(&editor_for_clear, &selected_for_clear);
    });

    rebuild_list(&list, &bookmarks, &selected_uuid, &editor, parent, &dialog);
    dialog.present();
}

fn rebuild_list(
    list: &gtk4::ListBox,
    bookmarks: &Rc<RefCell<Vec<Bookmark>>>,
    selected_uuid: &Rc<RefCell<Option<String>>>,
    editor: &EditorWidgets,
    parent: &Window,
    dialog: &gtk4::Window,
) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }

    let mut selected_row = None;
    for bookmark in bookmarks.borrow().iter().cloned() {
        let row = gtk4::ListBoxRow::new();
        row.set_widget_name(&bookmark.uuid);

        let action_row = adw::ActionRow::new();
        action_row.set_title(&bookmark.name);
        action_row.set_subtitle(&bookmark.summary());

        let open_button = gtk4::Button::builder()
            .icon_name("go-next-symbolic")
            .tooltip_text("Open bookmark")
            .valign(gtk4::Align::Center)
            .build();
        let delete_button = gtk4::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("Delete bookmark")
            .valign(gtk4::Align::Center)
            .build();

        action_row.add_suffix(&open_button);
        action_row.add_suffix(&delete_button);
        row.set_child(Some(&action_row));
        list.append(&row);

        let parent_for_open = parent.clone();
        let dialog_for_open = dialog.clone();
        let bookmark_for_open = bookmark.clone();
        open_button.connect_clicked(move |_| {
            parent_for_open.execute_bookmark(&bookmark_for_open);
            dialog_for_open.close();
        });

        let list_for_delete = list.clone();
        let bookmarks_for_delete = bookmarks.clone();
        let selected_for_delete = selected_uuid.clone();
        let editor_for_delete = editor.clone();
        let parent_for_delete = parent.clone();
        let dialog_for_delete = dialog.clone();
        let bookmark_uuid = bookmark.uuid.clone();
        delete_button.connect_clicked(move |_| {
            {
                let mut items = bookmarks_for_delete.borrow_mut();
                items.retain(|bookmark| bookmark.uuid != bookmark_uuid);
                if let Err(error) = bookmarks::save(&items) {
                    editor_for_delete.status.set_text(&format!("Failed to save bookmarks: {error}"));
                    return;
                }
            }

            if selected_for_delete.borrow().as_deref() == Some(bookmark_uuid.as_str()) {
                clear_editor(&editor_for_delete, &selected_for_delete);
            }

            parent_for_delete.refresh_bookmark_sidebar();
            rebuild_list(
                &list_for_delete,
                &bookmarks_for_delete,
                &selected_for_delete,
                &editor_for_delete,
                &parent_for_delete,
                &dialog_for_delete,
            );
        });

        if selected_uuid.borrow().as_deref() == Some(bookmark.uuid.as_str()) {
            selected_row = Some(row);
        }
    }

    if let Some(row) = selected_row {
        list.select_row(Some(&row));
    }
}

fn bookmark_from_editor(
    editor: &EditorWidgets,
    existing_uuid: Option<String>,
) -> Result<Bookmark, String> {
    let name = editor.name.text().trim().to_string();
    if name.is_empty() {
        return Err("Bookmark name is required".into());
    }

    let mut bookmark = Bookmark::new(name);
    if let Some(existing_uuid) = existing_uuid {
        bookmark.uuid = existing_uuid;
    }
    bookmark.directory = entry_value(&editor.directory);
    bookmark.ssh_target = entry_value(&editor.ssh_target);
    bookmark.tmux_session = entry_value(&editor.tmux_session);

    if !bookmark.is_actionable() {
        return Err("Add a directory, SSH target, tmux session, or a combination of them".into());
    }

    Ok(bookmark)
}

fn entry_value(row: &adw::EntryRow) -> Option<String> {
    let value = row.text();
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn fill_editor(editor: &EditorWidgets, bookmark: &Bookmark) {
    editor.name.set_text(&bookmark.name);
    editor.directory.set_text(bookmark.directory.as_deref().unwrap_or_default());
    editor.ssh_target.set_text(bookmark.ssh_target.as_deref().unwrap_or_default());
    editor.tmux_session.set_text(bookmark.tmux_session.as_deref().unwrap_or_default());
    editor.status.set_text("");
}

fn clear_editor(editor: &EditorWidgets, selected_uuid: &Rc<RefCell<Option<String>>>) {
    *selected_uuid.borrow_mut() = None;
    editor.name.set_text("");
    editor.directory.set_text("");
    editor.ssh_target.set_text("");
    editor.tmux_session.set_text("");
    editor.status.set_text("");
}
