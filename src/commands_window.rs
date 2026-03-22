use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{ActionRowExt, PreferencesRowExt};
use std::cell::RefCell;
use std::rc::Rc;

use crate::commands::{self, CommandRunMode, SavedCommand};
use crate::window::Window;

#[derive(Clone)]
struct EditorWidgets {
    title: adw::EntryRow,
    body_buffer: gtk4::TextBuffer,
    run_mode: gtk4::DropDown,
    status: gtk4::Label,
}

pub fn show(parent: &Window) {
    let dialog = gtk4::Window::builder()
        .title("Commands")
        .default_width(760)
        .default_height(620)
        .modal(true)
        .transient_for(parent)
        .build();
    if let Some(app) = parent.application() {
        dialog.set_application(Some(&app));
    }

    let commands = Rc::new(RefCell::new(commands::load()));
    let selected_uuid = Rc::new(RefCell::new(None::<String>));

    let header = adw::HeaderBar::new();
    let new_button =
        gtk4::Button::builder().icon_name("list-add-symbolic").tooltip_text("New command").build();
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

    let body_buffer = gtk4::TextBuffer::new(None);
    let body_view = gtk4::TextView::with_buffer(&body_buffer);
    body_view.set_wrap_mode(gtk4::WrapMode::WordChar);
    body_view.set_monospace(true);
    let body_scroll =
        gtk4::ScrolledWindow::builder().min_content_height(180).child(&body_view).build();

    let run_mode = gtk4::DropDown::from_strings(&["Run", "Insert"]);
    let run_mode_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    let run_mode_label = gtk4::Label::new(Some("Default action"));
    run_mode_label.set_xalign(0.0);
    run_mode_label.set_hexpand(true);
    run_mode_row.append(&run_mode_label);
    run_mode_row.append(&run_mode);

    let editor = EditorWidgets {
        title: adw::EntryRow::builder().title("Title").build(),
        body_buffer,
        run_mode,
        status: gtk4::Label::new(None),
    };
    editor.status.set_xalign(0.0);
    editor.status.add_css_class("dim-label");

    let save_button = gtk4::Button::with_label("Save command");
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
    editor_box.append(&editor.title);
    editor_box.append(&body_scroll);
    editor_box.append(&run_mode_row);
    editor_box.append(&button_box);
    editor_box.append(&editor.status);

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&scrolled);
    content.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    content.append(&editor_box);
    dialog.set_child(Some(&content));

    let commands_for_select = commands.clone();
    let selected_for_select = selected_uuid.clone();
    let editor_for_select = editor.clone();
    list.connect_row_selected(move |_, row| {
        let Some(row) = row else {
            clear_editor(&editor_for_select, &selected_for_select);
            return;
        };
        let command_uuid = row.widget_name();
        let command = commands_for_select
            .borrow()
            .iter()
            .find(|command| command.uuid == command_uuid)
            .cloned();

        if let Some(command) = command {
            *selected_for_select.borrow_mut() = Some(command.uuid.clone());
            fill_editor(&editor_for_select, &command);
        }
    });

    let list_for_new = list.clone();
    let editor_for_new = editor.clone();
    let selected_for_new = selected_uuid.clone();
    new_button.connect_clicked(move |_| {
        list_for_new.unselect_all();
        clear_editor(&editor_for_new, &selected_for_new);
    });

    let commands_for_save = commands.clone();
    let list_for_save = list.clone();
    let editor_for_save = editor.clone();
    let selected_for_save = selected_uuid.clone();
    let parent_for_save = parent.clone();
    let dialog_for_save = dialog.clone();
    save_button.connect_clicked(move |_| {
        let command =
            match command_from_editor(&editor_for_save, selected_for_save.borrow().clone()) {
                Ok(command) => command,
                Err(message) => {
                    editor_for_save.status.set_text(&message);
                    return;
                }
            };

        {
            let mut items = commands_for_save.borrow_mut();
            if let Some(existing) = items.iter_mut().find(|existing| existing.uuid == command.uuid)
            {
                *existing = command.clone();
            } else {
                items.push(command.clone());
            }
            if let Err(error) = commands::save(&items) {
                editor_for_save.status.set_text(&format!("Failed to save commands: {error}"));
                return;
            }
        }

        *selected_for_save.borrow_mut() = Some(command.uuid);
        editor_for_save.status.set_text("");
        parent_for_save.refresh_command_sidebar();
        rebuild_list(
            &list_for_save,
            &commands_for_save,
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

    rebuild_list(&list, &commands, &selected_uuid, &editor, parent, &dialog);
    dialog.present();
}

fn rebuild_list(
    list: &gtk4::ListBox,
    commands: &Rc<RefCell<Vec<SavedCommand>>>,
    selected_uuid: &Rc<RefCell<Option<String>>>,
    editor: &EditorWidgets,
    parent: &Window,
    dialog: &gtk4::Window,
) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }

    let mut selected_row = None;
    for command in commands.borrow().iter().cloned() {
        let row = gtk4::ListBoxRow::new();
        row.set_widget_name(&command.uuid);

        let action_row = adw::ActionRow::new();
        action_row.set_title(&command.title);
        action_row.set_subtitle(&command.preview());

        let run_button = gtk4::Button::builder()
            .icon_name("go-next-symbolic")
            .tooltip_text("Run command")
            .valign(gtk4::Align::Center)
            .build();
        let insert_button = gtk4::Button::builder()
            .icon_name("insert-text-symbolic")
            .tooltip_text("Insert command")
            .valign(gtk4::Align::Center)
            .build();
        let delete_button = gtk4::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("Delete command")
            .valign(gtk4::Align::Center)
            .build();

        action_row.add_suffix(&run_button);
        action_row.add_suffix(&insert_button);
        action_row.add_suffix(&delete_button);
        row.set_child(Some(&action_row));
        list.append(&row);

        let parent_for_run = parent.clone();
        let command_for_run = command.clone();
        run_button.connect_clicked(move |_| {
            parent_for_run.execute_saved_command(&command_for_run, CommandRunMode::Run);
        });

        let parent_for_insert = parent.clone();
        let command_for_insert = command.clone();
        insert_button.connect_clicked(move |_| {
            parent_for_insert.execute_saved_command(&command_for_insert, CommandRunMode::Insert);
        });

        let list_for_delete = list.clone();
        let commands_for_delete = commands.clone();
        let selected_for_delete = selected_uuid.clone();
        let editor_for_delete = editor.clone();
        let parent_for_delete = parent.clone();
        let dialog_for_delete = dialog.clone();
        let command_uuid = command.uuid.clone();
        delete_button.connect_clicked(move |_| {
            {
                let mut items = commands_for_delete.borrow_mut();
                items.retain(|command| command.uuid != command_uuid);
                if let Err(error) = commands::save(&items) {
                    editor_for_delete.status.set_text(&format!("Failed to save commands: {error}"));
                    return;
                }
            }

            if selected_for_delete.borrow().as_deref() == Some(command_uuid.as_str()) {
                clear_editor(&editor_for_delete, &selected_for_delete);
            }

            parent_for_delete.refresh_command_sidebar();
            rebuild_list(
                &list_for_delete,
                &commands_for_delete,
                &selected_for_delete,
                &editor_for_delete,
                &parent_for_delete,
                &dialog_for_delete,
            );
        });

        if selected_uuid.borrow().as_deref() == Some(command.uuid.as_str()) {
            selected_row = Some(row);
        }
    }

    if let Some(row) = selected_row {
        list.select_row(Some(&row));
    }
}

fn command_from_editor(
    editor: &EditorWidgets,
    existing_uuid: Option<String>,
) -> Result<SavedCommand, String> {
    let title = editor.title.text().trim().to_string();
    if title.is_empty() {
        return Err("Command title is required".into());
    }

    let body = editor
        .body_buffer
        .text(&editor.body_buffer.start_iter(), &editor.body_buffer.end_iter(), false)
        .to_string();
    if body.trim().is_empty() {
        return Err("Command body is required".into());
    }

    let mut command = SavedCommand::new(title, body);
    if let Some(existing_uuid) = existing_uuid {
        command.uuid = existing_uuid;
    }
    command.default_run_mode = run_mode_from_index(editor.run_mode.selected());
    Ok(command)
}

const fn run_mode_from_index(index: u32) -> CommandRunMode {
    match index {
        1 => CommandRunMode::Insert,
        _ => CommandRunMode::Run,
    }
}

const fn run_mode_index(run_mode: CommandRunMode) -> u32 {
    match run_mode {
        CommandRunMode::Run => 0,
        CommandRunMode::Insert => 1,
    }
}

fn fill_editor(editor: &EditorWidgets, command: &SavedCommand) {
    editor.title.set_text(&command.title);
    editor.body_buffer.set_text(&command.body);
    editor.run_mode.set_selected(run_mode_index(command.default_run_mode));
    editor.status.set_text("");
}

fn clear_editor(editor: &EditorWidgets, selected_uuid: &Rc<RefCell<Option<String>>>) {
    *selected_uuid.borrow_mut() = None;
    editor.title.set_text("");
    editor.body_buffer.set_text("");
    editor.run_mode.set_selected(0);
    editor.status.set_text("");
}
