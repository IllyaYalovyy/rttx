use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::commands::{CommandRunMode, SavedCommand, render_env_block};
use crate::window::Window;

/// Show the runtime parameter dialog for a parameterized command.
///
/// Presents one `adw::ComboRow` per declared parameter with a live preview
/// of the rendered shell block. The primary action matches `run_mode`.
pub fn show(parent: &Window, command: &SavedCommand, run_mode: CommandRunMode) {
    let dialog = adw::Dialog::builder().title(&command.title).content_width(480).build();

    let header = adw::HeaderBar::new();

    let action_label = match run_mode {
        CommandRunMode::Run => "Run",
        CommandRunMode::Insert => "Insert",
    };
    let action_button = gtk4::Button::with_label(action_label);
    action_button.add_css_class("suggested-action");
    header.pack_end(&action_button);

    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content_box.set_margin_start(18);
    content_box.set_margin_end(18);
    content_box.set_margin_top(18);
    content_box.set_margin_bottom(18);

    let params_group = adw::PreferencesGroup::builder().title("Parameters").build();

    let combo_rows: Vec<(String, adw::ComboRow)> = command
        .parameters
        .iter()
        .map(|param| {
            let choices: Vec<&str> = param.choices.iter().map(String::as_str).collect();
            let string_list = gtk4::StringList::new(&choices);
            let row = adw::ComboRow::builder().title(&param.label).model(&string_list).build();

            let default_idx = resolve_default_index(param);
            row.set_selected(default_idx);

            params_group.add(&row);
            (param.name.clone(), row)
        })
        .collect();

    let preview_label = gtk4::Label::builder().label("Preview").xalign(0.0).build();
    preview_label.add_css_class("dim-label");
    preview_label.add_css_class("caption");

    let preview_buffer = gtk4::TextBuffer::new(None);
    let preview_view = gtk4::TextView::with_buffer(&preview_buffer);
    preview_view.set_editable(false);
    preview_view.set_monospace(true);
    preview_view.set_wrap_mode(gtk4::WrapMode::WordChar);
    preview_view.add_css_class("card");
    let preview_scroll =
        gtk4::ScrolledWindow::builder().min_content_height(100).child(&preview_view).build();

    content_box.append(&params_group);
    content_box.append(&preview_label);
    content_box.append(&preview_scroll);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&content_box));
    dialog.set_child(Some(&toolbar_view));

    // Initial preview render
    let body = command.body.clone();
    let parameters = command.parameters.clone();
    update_preview(&preview_buffer, &body, &parameters, &combo_rows);

    // Update preview on every combo change
    for (_, row) in &combo_rows {
        let buf = preview_buffer.clone();
        let body = body.clone();
        let params = parameters.clone();
        let rows = combo_rows.clone();
        row.connect_selected_notify(move |_| {
            update_preview(&buf, &body, &params, &rows);
        });
    }

    // Action button
    let dialog_ref = dialog.clone();
    let parent_win = parent.clone();
    let command_clone = command.clone();
    let rows_for_action = combo_rows;
    action_button.connect_clicked(move |_| {
        let values = collect_values(&command_clone.parameters, &rows_for_action);
        let rendered = render_env_block(&command_clone.body, &values);
        let text = match run_mode {
            CommandRunMode::Run => format!("{rendered}\n"),
            CommandRunMode::Insert => rendered,
        };
        parent_win.execute_command_text(&command_clone, run_mode, &text);
        dialog_ref.close();
    });

    dialog.present(Some(parent));
}

fn update_preview(
    buffer: &gtk4::TextBuffer,
    body: &str,
    parameters: &[crate::commands::CommandParameter],
    combo_rows: &[(String, adw::ComboRow)],
) {
    let values = collect_values(parameters, combo_rows);
    let rendered = render_env_block(body, &values);
    buffer.set_text(&rendered);
}

fn collect_values(
    parameters: &[crate::commands::CommandParameter],
    combo_rows: &[(String, adw::ComboRow)],
) -> Vec<(String, String)> {
    parameters
        .iter()
        .zip(combo_rows.iter())
        .map(|(param, (name, row))| {
            let selected = row.selected() as usize;
            let value = param.choices.get(selected).cloned().unwrap_or_default();
            (name.clone(), value)
        })
        .collect()
}

fn resolve_default_index(param: &crate::commands::CommandParameter) -> u32 {
    if let Some(ref d) = param.default
        && let Some(idx) = param.choices.iter().position(|c| c == d)
    {
        return idx as u32;
    }
    0
}
