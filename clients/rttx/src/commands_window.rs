use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::commands::{self, CommandRunMode, SavedCommand};
use crate::window::Window;

pub fn show_form(parent: &Window, command: Option<&SavedCommand>) {
    let existing_uuid = command.map(|c| c.uuid.clone());

    let dialog = adw::Dialog::builder()
        .title(if command.is_some() { "Edit Command" } else { "New Command" })
        .content_width(480)
        .build();

    let header = adw::HeaderBar::new();
    let save_button = gtk4::Button::with_label(if command.is_some() { "Save" } else { "Add" });
    save_button.add_css_class("suggested-action");
    header.pack_end(&save_button);

    let title_row = adw::EntryRow::builder().title("Title").build();

    let body_buffer = gtk4::TextBuffer::new(None);
    let body_view = gtk4::TextView::with_buffer(&body_buffer);
    body_view.set_wrap_mode(gtk4::WrapMode::WordChar);
    body_view.set_monospace(true);
    let body_scroll =
        gtk4::ScrolledWindow::builder().min_content_height(150).child(&body_view).build();

    let run_mode = gtk4::DropDown::from_strings(&["Run", "Insert"]);
    let run_mode_row = adw::ActionRow::builder().title("Default action").build();
    run_mode_row.add_suffix(&run_mode);
    run_mode_row.set_activatable_widget(Some(&run_mode));

    let status_label = gtk4::Label::new(None);
    status_label.set_xalign(0.0);
    status_label.add_css_class("dim-label");

    let title_group = adw::PreferencesGroup::new();
    title_group.add(&title_row);

    let behavior_group = adw::PreferencesGroup::new();
    behavior_group.add(&run_mode_row);

    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content_box.set_margin_start(18);
    content_box.set_margin_end(18);
    content_box.set_margin_top(18);
    content_box.set_margin_bottom(18);
    content_box.append(&title_group);
    content_box.append(&body_scroll);
    content_box.append(&behavior_group);
    content_box.append(&status_label);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&content_box));

    dialog.set_child(Some(&toolbar_view));

    if let Some(c) = command {
        title_row.set_text(&c.title);
        body_buffer.set_text(&c.body);
        run_mode.set_selected(run_mode_index(c.default_run_mode));
    }

    let dialog_for_save = dialog.clone();
    let parent_for_save = parent.clone();
    save_button.connect_clicked(move |_| {
        let cmd = match build_command(&title_row, &body_buffer, &run_mode, existing_uuid.clone()) {
            Ok(c) => c,
            Err(msg) => {
                status_label.set_text(&msg);
                return;
            }
        };

        let mut items = commands::load();
        if let Some(existing) = items.iter_mut().find(|i| i.uuid == cmd.uuid) {
            *existing = cmd;
        } else {
            items.push(cmd);
        }
        if let Err(e) = commands::save(&items) {
            status_label.set_text(&format!("Failed to save: {e}"));
            return;
        }
        parent_for_save.refresh_command_sidebar();
        dialog_for_save.close();
    });

    dialog.present(Some(parent));
}

fn build_command(
    title_row: &adw::EntryRow,
    body_buffer: &gtk4::TextBuffer,
    run_mode: &gtk4::DropDown,
    existing_uuid: Option<String>,
) -> Result<SavedCommand, String> {
    let title = title_row.text().trim().to_string();
    if title.is_empty() {
        return Err("Command title is required".into());
    }

    let body =
        body_buffer.text(&body_buffer.start_iter(), &body_buffer.end_iter(), false).to_string();
    if body.trim().is_empty() {
        return Err("Command body is required".into());
    }

    let mut command = SavedCommand::new(title, body);
    if let Some(uuid) = existing_uuid {
        command.uuid = uuid;
    }
    command.default_run_mode = run_mode_from_index(run_mode.selected());
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
