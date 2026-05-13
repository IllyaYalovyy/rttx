use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::commands::{CommandParameter, CommandRunMode, SavedCommand};
use crate::form_dialog::FormDialog;
use crate::host_tag_picker::HostTagPicker;
use crate::window::Window;

pub fn show_form(parent: &Window, command: Option<&SavedCommand>) {
    let existing_uuid = command.map(|c| c.uuid.clone());

    let form = FormDialog::new("Command", command.is_some(), 480);

    let title_row = adw::EntryRow::builder().title("Title").build();

    let description_row = adw::EntryRow::builder().title("Description").build();

    let labels_row = adw::EntryRow::builder().title("Labels (comma-separated)").build();

    let body_buffer = gtk4::TextBuffer::new(None);
    let body_view = gtk4::TextView::with_buffer(&body_buffer);
    body_view.set_wrap_mode(gtk4::WrapMode::WordChar);
    body_view.set_monospace(true);
    let body_scroll =
        gtk4::ScrolledWindow::builder().min_content_height(150).child(&body_view).build();

    let run_mode = gtk4::DropDown::from_strings(&["Run", "Insert", "Run in new pane"]);
    let run_mode_row = adw::ActionRow::builder().title("Default action").build();
    run_mode_row.add_suffix(&run_mode);
    run_mode_row.set_activatable_widget(Some(&run_mode));

    let selected_tags = command.map_or_else(Vec::new, |c| c.host_tags.clone());
    let host_picker = HostTagPicker::new(&selected_tags);

    let title_group = adw::PreferencesGroup::new();
    title_group.add(&title_row);
    title_group.add(&description_row);
    title_group.add(&labels_row);

    let behavior_group = adw::PreferencesGroup::new();
    behavior_group.add(&run_mode_row);

    // Parameters section
    let params_group = adw::PreferencesGroup::builder().title("Parameters").build();
    let params_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    let params_list = gtk4::ListBox::new();
    params_list.add_css_class("boxed-list");
    params_list.set_selection_mode(gtk4::SelectionMode::None);
    params_box.append(&params_list);

    let add_param_button = gtk4::Button::with_label("Add parameter");
    add_param_button.add_css_class("flat");
    params_box.append(&add_param_button);
    params_group.add(&params_box);

    // Shortcut key sequence
    let shortcut_group = adw::PreferencesGroup::builder().title("Leader Shortcut").build();
    let shortcut_row = adw::ActionRow::builder()
        .title("Key sequence")
        .subtitle("Press to capture keys after the leader")
        .build();
    let shortcut_label = gtk4::Label::new(None);
    shortcut_label.add_css_class("dim-label");
    shortcut_label.add_css_class("monospace");
    shortcut_row.add_suffix(&shortcut_label);

    let shortcut_keys: std::rc::Rc<std::cell::RefCell<Vec<String>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));

    let clear_button = gtk4::Button::builder()
        .icon_name("edit-clear-symbolic")
        .tooltip_text("Clear shortcut")
        .valign(gtk4::Align::Center)
        .build();
    clear_button.add_css_class("flat");
    shortcut_row.add_suffix(&clear_button);

    {
        let keys_ref = shortcut_keys.clone();
        let label_ref = shortcut_label.clone();
        clear_button.connect_clicked(move |_| {
            keys_ref.borrow_mut().clear();
            label_ref.set_text("");
        });
    }

    shortcut_group.add(&shortcut_row);

    let capture_button = gtk4::Button::with_label("Capture");
    capture_button.add_css_class("flat");
    shortcut_group.add(&capture_button);

    {
        let keys_ref = shortcut_keys.clone();
        let label_ref = shortcut_label.clone();
        let dialog_ref = form.dialog.clone();
        capture_button.connect_clicked(move |_| {
            show_capture_dialog(&dialog_ref, &keys_ref, &label_ref);
        });
    }

    form.content_box.append(&title_group);
    form.content_box.append(&body_scroll);
    form.content_box.append(&behavior_group);
    form.content_box.append(&shortcut_group);
    form.content_box.append(&params_group);
    form.content_box.append(&host_picker.group);
    form.finish_layout();

    if let Some(c) = command {
        title_row.set_text(&c.title);
        description_row.set_text(&c.description);
        labels_row.set_text(&c.labels.join(", "));
        body_buffer.set_text(&c.body);
        run_mode.set_selected(run_mode_index(c.default_run_mode));
        for param in &c.parameters {
            append_parameter_row(&params_list, Some(param));
        }
        if !c.shortcut_keys.is_empty() {
            shortcut_keys.borrow_mut().clone_from(&c.shortcut_keys);
            shortcut_label.set_text(&c.shortcut_keys.join(" "));
        }
    }

    // Add parameter button
    let list_for_add = params_list.clone();
    add_param_button.connect_clicked(move |_| {
        append_parameter_row(&list_for_add, None);
    });

    let dialog = form.dialog.clone();
    let status_label = form.status_label.clone();
    let parent_for_save = parent.clone();
    form.save_button.connect_clicked(move |_| {
        let cmd = match build_command(
            &title_row,
            &description_row,
            &labels_row,
            &body_buffer,
            &run_mode,
            &host_picker,
            &params_list,
            existing_uuid.clone(),
            &shortcut_keys.borrow(),
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

fn append_parameter_row(list: &gtk4::ListBox, param: Option<&CommandParameter>) {
    let row_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    row_box.set_margin_start(6);
    row_box.set_margin_end(6);
    row_box.set_margin_top(6);
    row_box.set_margin_bottom(6);

    let name_entry = adw::EntryRow::builder().title("Variable name").build();
    let label_entry = adw::EntryRow::builder().title("Label").build();
    let description_entry = adw::EntryRow::builder().title("Description").build();
    let choices_entry = adw::EntryRow::builder().title("Choices (comma-separated)").build();
    let default_entry = adw::EntryRow::builder().title("Default").build();

    if let Some(p) = param {
        name_entry.set_text(&p.name);
        label_entry.set_text(&p.label);
        description_entry.set_text(&p.description);
        choices_entry.set_text(&p.choices.join(", "));
        if let Some(ref d) = p.default {
            default_entry.set_text(d);
        }
    }

    let remove_button = gtk4::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Remove parameter")
        .halign(gtk4::Align::End)
        .build();
    remove_button.add_css_class("flat");
    remove_button.add_css_class("destructive-action");

    let entries_group = adw::PreferencesGroup::new();
    entries_group.add(&name_entry);
    entries_group.add(&label_entry);
    entries_group.add(&description_entry);
    entries_group.add(&choices_entry);
    entries_group.add(&default_entry);

    row_box.append(&entries_group);
    row_box.append(&remove_button);

    let list_row = gtk4::ListBoxRow::new();
    list_row.set_child(Some(&row_box));
    list_row.set_selectable(false);
    list_row.set_activatable(false);
    list.append(&list_row);

    let list_ref = list.clone();
    remove_button.connect_clicked(move |_| {
        list_ref.remove(&list_row);
    });
}

#[allow(clippy::too_many_arguments)] // Form fields are naturally numerous
fn build_command(
    title_row: &adw::EntryRow,
    description_row: &adw::EntryRow,
    labels_row: &adw::EntryRow,
    body_buffer: &gtk4::TextBuffer,
    run_mode: &gtk4::DropDown,
    host_picker: &HostTagPicker,
    params_list: &gtk4::ListBox,
    existing_uuid: Option<String>,
    shortcut_keys: &[String],
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

    let parameters = extract_parameters(params_list)?;

    let labels: Vec<String> = labels_row
        .text()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let mut command = SavedCommand::new(title, body);
    if let Some(uuid) = existing_uuid {
        command.uuid = uuid;
    }
    command.default_run_mode = run_mode_from_index(run_mode.selected());
    command.host_tags = host_picker.selected_tags();
    command.parameters = parameters;
    command.description = description_row.text().trim().to_string();
    command.labels = labels;
    command.shortcut_keys = shortcut_keys.to_vec();
    Ok(command)
}

fn extract_parameters(list: &gtk4::ListBox) -> Result<Vec<CommandParameter>, String> {
    let mut params = Vec::new();
    let mut idx = 0;
    while let Some(row) = list.row_at_index(idx) {
        idx += 1;
        let Some(row_box) = row.child().and_then(|c| c.downcast::<gtk4::Box>().ok()) else {
            continue;
        };
        let Some(group) =
            row_box.first_child().and_then(|c| c.downcast::<adw::PreferencesGroup>().ok())
        else {
            continue;
        };

        let entries = collect_entry_rows(&group);
        if entries.len() < 5 {
            continue;
        }

        let name = entries[0].text().trim().to_string();
        if name.is_empty() {
            return Err("Parameter variable name is required".into());
        }
        let label = entries[1].text().trim().to_string();
        if label.is_empty() {
            return Err("Parameter label is required".into());
        }
        let description = entries[2].text().trim().to_string();
        let choices_text = entries[3].text().trim().to_string();
        let choices: Vec<String> = choices_text
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let default_text = entries[4].text().trim().to_string();
        let default = if default_text.is_empty() { None } else { Some(default_text) };

        params.push(CommandParameter { name, label, choices, default, description });
    }
    Ok(params)
}

fn collect_entry_rows(group: &adw::PreferencesGroup) -> Vec<adw::EntryRow> {
    let mut entries = Vec::new();
    let listbox = group.first_child().and_then(|c| {
        // PreferencesGroup wraps content in a Box > ListBox
        let mut child = Some(c);
        while let Some(c) = child {
            if let Ok(lb) = c.clone().downcast::<gtk4::ListBox>() {
                return Some(lb);
            }
            child = c.next_sibling();
        }
        None
    });
    if let Some(lb) = listbox {
        let mut i = 0;
        while let Some(row) = lb.row_at_index(i) {
            if let Ok(entry) = row.downcast::<adw::EntryRow>() {
                entries.push(entry);
            }
            i += 1;
        }
    }
    entries
}

const fn run_mode_from_index(index: u32) -> CommandRunMode {
    match index {
        1 => CommandRunMode::Insert,
        2 => CommandRunMode::RunInNewPane,
        _ => CommandRunMode::Run,
    }
}

const fn run_mode_index(run_mode: CommandRunMode) -> u32 {
    match run_mode {
        CommandRunMode::Run => 0,
        CommandRunMode::Insert => 1,
        CommandRunMode::RunInNewPane => 2,
    }
}

fn show_capture_dialog(
    parent: &adw::Dialog,
    keys_out: &std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    label_out: &gtk4::Label,
) {
    let dialog = adw::Dialog::builder()
        .title("Capture Shortcut")
        .content_width(300)
        .content_height(150)
        .build();

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let instruction =
        gtk4::Label::new(Some("Press 1–2 keys for the sequence.\nEnter confirms, Escape cancels."));
    instruction.set_wrap(true);
    instruction.set_justify(gtk4::Justification::Center);
    content.append(&instruction);

    let preview = gtk4::Label::new(None);
    preview.add_css_class("monospace");
    preview.add_css_class("title-3");
    content.append(&preview);

    dialog.set_child(Some(&content));

    let captured: std::rc::Rc<std::cell::RefCell<Vec<String>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));

    let controller = gtk4::EventControllerKey::new();
    controller.set_propagation_phase(gtk4::PropagationPhase::Capture);

    let keys_ref = keys_out.clone();
    let label_ref = label_out.clone();
    let dialog_ref = dialog.clone();
    controller.connect_key_pressed(move |_, keyval, _keycode, _state| {
        let key_name = keyval.name().map(|n| n.to_string()).unwrap_or_default();
        if key_name.is_empty() {
            return glib::Propagation::Stop;
        }
        if key_name == "Escape" {
            dialog_ref.close();
            return glib::Propagation::Stop;
        }
        if key_name == "Return" || key_name == "KP_Enter" {
            let keys = captured.borrow().clone();
            if !keys.is_empty() {
                keys_ref.borrow_mut().clone_from(&keys);
                label_ref.set_text(&keys.join(" "));
            }
            dialog_ref.close();
            return glib::Propagation::Stop;
        }

        let mut keys = captured.borrow_mut();
        if keys.len() < 2 {
            keys.push(key_name);
            preview.set_text(&keys.join(" "));
        }
        glib::Propagation::Stop
    });

    dialog.add_controller(controller);
    dialog.present(Some(parent));
}
