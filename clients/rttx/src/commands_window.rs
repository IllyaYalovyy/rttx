use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::commands::{CommandRunMode, SavedCommand};
use crate::form_dialog::FormDialog;
use crate::host_tag_picker::HostTagPicker;
use crate::window::Window;

pub fn show_form(parent: &Window, command: Option<&SavedCommand>) {
    let existing_uuid = command.map(|c| c.uuid.clone());

    let form = FormDialog::new("Command", command.is_some(), 480);

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

    let selected_tags = command.map_or_else(Vec::new, |c| c.host_tags.clone());
    let host_picker = HostTagPicker::new(&selected_tags);

    let title_group = adw::PreferencesGroup::new();
    title_group.add(&title_row);

    let behavior_group = adw::PreferencesGroup::new();
    behavior_group.add(&run_mode_row);

    form.content_box.append(&title_group);
    form.content_box.append(&body_scroll);
    form.content_box.append(&behavior_group);
    form.content_box.append(&host_picker.group);
    form.finish_layout();

    if let Some(c) = command {
        title_row.set_text(&c.title);
        body_buffer.set_text(&c.body);
        run_mode.set_selected(run_mode_index(c.default_run_mode));
    }

    let dialog = form.dialog.clone();
    let status_label = form.status_label.clone();
    let parent_for_save = parent.clone();
    form.save_button.connect_clicked(move |_| {
        let cmd = match build_command(
            &title_row,
            &body_buffer,
            &run_mode,
            &host_picker,
            existing_uuid.clone(),
        ) {
            Ok(c) => c,
            Err(msg) => {
                status_label.set_text(&msg);
                return;
            }
        };

        let mut items = crate::store::default_store().load_commands();
        if let Some(existing) = items.iter_mut().find(|i| i.uuid == cmd.uuid) {
            *existing = cmd;
        } else {
            items.push(cmd);
        }
        if let Err(e) = crate::store::default_store().save_commands(&items) {
            status_label.set_text(&format!("Failed to save: {e}"));
            return;
        }
        parent_for_save.refresh_command_sidebar();
        dialog.close();
    });

    form.present(parent);
}

fn build_command(
    title_row: &adw::EntryRow,
    body_buffer: &gtk4::TextBuffer,
    run_mode: &gtk4::DropDown,
    host_picker: &HostTagPicker,
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
    command.host_tags = host_picker.selected_tags();
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
