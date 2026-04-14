use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

/// Shared scaffolding for add/edit form dialogs (places, commands, etc.).
///
/// Builds the dialog chrome — header bar, save button, status label, content
/// box, and toolbar view — so callers only supply the form-specific widgets
/// and the save callback.
#[derive(Debug)]
pub struct FormDialog {
    pub dialog: adw::Dialog,
    pub save_button: gtk4::Button,
    pub status_label: gtk4::Label,
    pub content_box: gtk4::Box,
}

impl FormDialog {
    /// Create a new form dialog.
    ///
    /// `is_edit` controls the title ("Edit …" vs "New …") and button label
    /// ("Save" vs "Add").
    #[must_use]
    pub fn new(entity_name: &str, is_edit: bool, content_width: i32) -> Self {
        let title =
            if is_edit { format!("Edit {entity_name}") } else { format!("New {entity_name}") };
        let button_label = if is_edit { "Save" } else { "Add" };

        let dialog = adw::Dialog::builder().title(title).content_width(content_width).build();

        let header = adw::HeaderBar::new();
        let save_button = gtk4::Button::with_label(button_label);
        save_button.add_css_class("suggested-action");
        header.pack_end(&save_button);

        let status_label = gtk4::Label::new(None);
        status_label.set_xalign(0.0);
        status_label.add_css_class("dim-label");

        let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        content_box.set_margin_start(18);
        content_box.set_margin_end(18);
        content_box.set_margin_top(18);
        content_box.set_margin_bottom(18);

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header);
        toolbar_view.set_content(Some(&content_box));

        dialog.set_child(Some(&toolbar_view));

        Self { dialog, save_button, status_label, content_box }
    }

    /// Append the status label to the content box. Call after adding all
    /// form-specific widgets so the label appears at the bottom.
    pub fn finish_layout(&self) {
        self.content_box.append(&self.status_label);
    }

    /// Present the dialog as a child of `parent`.
    pub fn present(&self, parent: &impl IsA<gtk4::Widget>) {
        self.dialog.present(Some(parent));
    }
}

/// Read the trimmed text from an `EntryRow`, returning `None` for blank input.
#[must_use]
pub fn entry_value(row: &adw::EntryRow) -> Option<String> {
    let text = row.text();
    let trimmed = text.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}
