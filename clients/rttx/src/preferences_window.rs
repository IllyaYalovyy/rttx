use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::color_scheme;
use crate::preferences::{self, DefaultSessionFolder, Preferences, TerminalThemeMode};
use crate::shortcuts::{self, DEFAULT_SHORTCUTS};

/// Build and present the preferences window.
pub fn show(parent: &impl IsA<gtk4::Window>) {
    let prefs = preferences::load();
    let window = adw::PreferencesWindow::new();
    window.set_transient_for(Some(parent.as_ref()));
    window.set_modal(true);
    window.set_title(Some("Preferences"));

    let appearance_group = adw::PreferencesGroup::new();
    appearance_group.set_title("Appearance");
    let legacy_color_scheme = prefs.color_scheme.clone();

    let font_desc = gtk4::pango::FontDescription::from_string(&prefs.font);
    let font_dialog = gtk4::FontDialog::new();
    let font_button = gtk4::FontDialogButton::new(Some(font_dialog));
    font_button.set_font_desc(&font_desc);
    font_button.set_valign(gtk4::Align::Center);

    let font_row = adw::ActionRow::builder().title("Font").build();
    font_row.add_suffix(&font_button);
    appearance_group.add(&font_row);

    let mode_row = adw::ComboRow::builder().title("Terminal theme mode").build();
    let mode_names = ["Follow system", "Always light", "Always dark"];
    let mode_model = gtk4::StringList::new(&mode_names);
    mode_row.set_model(Some(&mode_model));
    mode_row.set_selected(match prefs.terminal_theme_mode {
        TerminalThemeMode::System => 0,
        TerminalThemeMode::Light => 1,
        TerminalThemeMode::Dark => 2,
    });
    appearance_group.add(&mode_row);

    let scheme_names = load_scheme_names();
    let scheme_model =
        gtk4::StringList::new(&scheme_names.iter().map(AsRef::as_ref).collect::<Vec<_>>());

    let light_scheme_row = adw::ComboRow::builder().title("Light terminal palette").build();
    light_scheme_row.set_model(Some(&scheme_model));
    if let Some(pos) = scheme_names.iter().position(|s| s == &prefs.light_color_scheme) {
        light_scheme_row.set_selected(pos as u32);
    }
    appearance_group.add(&light_scheme_row);

    let dark_scheme_row = adw::ComboRow::builder().title("Dark terminal palette").build();
    dark_scheme_row.set_model(Some(&scheme_model));
    if let Some(pos) = scheme_names.iter().position(|s| s == &prefs.dark_color_scheme) {
        dark_scheme_row.set_selected(pos as u32);
    }
    appearance_group.add(&dark_scheme_row);

    let terminal_group = adw::PreferencesGroup::new();
    terminal_group.set_title("Terminal");

    let scrollback_row = adw::SpinRow::with_range(0.0, 1_000_000.0, 1000.0);
    scrollback_row.set_title("Scrollback lines");
    scrollback_row.set_value(prefs.scrollback_lines as f64);
    terminal_group.add(&scrollback_row);

    let headerbar_row = adw::SwitchRow::new();
    headerbar_row.set_title("Show terminal header");
    headerbar_row.set_active(prefs.show_headerbar);
    terminal_group.add(&headerbar_row);

    let keystroke_row = adw::SwitchRow::new();
    keystroke_row.set_title("Scroll on keystroke");
    keystroke_row.set_active(prefs.scroll_on_keystroke);
    terminal_group.add(&keystroke_row);

    let output_row = adw::SwitchRow::new();
    output_row.set_title("Scroll on output");
    output_row.set_active(prefs.scroll_on_output);
    terminal_group.add(&output_row);

    let bell_row = adw::SwitchRow::new();
    bell_row.set_title("Audible bell");
    bell_row.set_active(prefs.audible_bell);
    terminal_group.add(&bell_row);

    let visual_bell_row = adw::SwitchRow::new();
    visual_bell_row.set_title("Visual bell");
    visual_bell_row.set_subtitle("Flash the terminal header when a bell character is received");
    visual_bell_row.set_active(prefs.visual_bell);
    terminal_group.add(&visual_bell_row);

    let smart_clipboard_row = adw::SwitchRow::new();
    smart_clipboard_row.set_title("Smart Ctrl+C / Ctrl+V");
    smart_clipboard_row.set_subtitle("Copy selected text with Ctrl+C and paste with Ctrl+V");
    smart_clipboard_row.set_active(prefs.smart_clipboard);
    terminal_group.add(&smart_clipboard_row);

    let paste_guard_row = adw::SwitchRow::new();
    paste_guard_row.set_title("Paste guard");
    paste_guard_row.set_subtitle("Confirm before pasting multiline or large text");
    paste_guard_row.set_active(prefs.paste_guard);
    terminal_group.add(&paste_guard_row);

    let trim_whitespace_row = adw::SwitchRow::new();
    trim_whitespace_row.set_title("Trim trailing whitespace on copy");
    trim_whitespace_row.set_subtitle("Remove trailing spaces from each line when copying text");
    trim_whitespace_row.set_active(prefs.trim_trailing_whitespace_on_copy);
    terminal_group.add(&trim_whitespace_row);

    let session_group = adw::PreferencesGroup::new();
    session_group.set_title("Workspaces");

    let folder_mode_row =
        adw::ComboRow::builder().title("Default folder for new workspaces").build();
    let folder_mode_names = ["Home directory", "Same as current workspace", "Custom path"];
    let folder_mode_model = gtk4::StringList::new(&folder_mode_names);
    folder_mode_row.set_model(Some(&folder_mode_model));

    let custom_folder_row =
        adw::EntryRow::builder().title("Custom folder path").show_apply_button(false).build();

    match &prefs.default_session_folder {
        DefaultSessionFolder::Home => {
            folder_mode_row.set_selected(0);
            custom_folder_row.set_visible(false);
        }
        DefaultSessionFolder::CurrentSession => {
            folder_mode_row.set_selected(1);
            custom_folder_row.set_visible(false);
        }
        DefaultSessionFolder::Custom(path) => {
            folder_mode_row.set_selected(2);
            custom_folder_row.set_text(path);
            custom_folder_row.set_visible(true);
        }
    }

    let custom_row_ref = custom_folder_row.clone();
    folder_mode_row.connect_selected_notify(move |row| {
        custom_row_ref.set_visible(row.selected() == 2);
    });

    session_group.add(&folder_mode_row);
    session_group.add(&custom_folder_row);

    // --- Keyboard shortcuts ---
    let keyboard_group = adw::PreferencesGroup::new();
    keyboard_group.set_title("Keyboard Shortcuts");
    keyboard_group
        .set_description(Some("Click a shortcut to change it. Press Backspace to clear."));

    let shortcut_overrides: Rc<RefCell<BTreeMap<String, Vec<String>>>> =
        Rc::new(RefCell::new(prefs.keyboard_shortcuts.clone()));

    for def in DEFAULT_SHORTCUTS {
        let accels = shortcuts::effective_accels(def.action, &prefs.keyboard_shortcuts);
        let is_custom = prefs.keyboard_shortcuts.contains_key(def.action);

        let row = adw::ActionRow::builder().title(def.label).activatable(true).build();

        let label_text = format_accel_label(&accels);
        let accel_label = gtk4::Label::new(Some(&label_text));
        accel_label.add_css_class("dim-label");
        if is_custom {
            accel_label.add_css_class("accent");
        }
        accel_label.set_valign(gtk4::Align::Center);
        row.add_suffix(&accel_label);

        let reset_button = gtk4::Button::from_icon_name("edit-undo-symbolic");
        reset_button.set_valign(gtk4::Align::Center);
        reset_button.set_tooltip_text(Some("Reset to default"));
        reset_button.add_css_class("flat");
        reset_button.set_visible(is_custom);
        row.add_suffix(&reset_button);

        let action_name = def.action.to_string();
        let overrides_ref = shortcut_overrides.clone();
        let accel_label_ref = accel_label.clone();
        let reset_ref = reset_button.clone();
        let window_ref = window.clone();
        let label_text_for_dialog = def.label.to_string();
        row.connect_activated(move |_row| {
            let action = action_name.clone();
            let overrides = overrides_ref.clone();
            let label = accel_label_ref.clone();
            let reset = reset_ref.clone();
            show_shortcut_capture_dialog(&window_ref, &label_text_for_dialog, move |new_accels| {
                overrides.borrow_mut().insert(action.clone(), new_accels.clone());
                label.set_label(&format_accel_label(&new_accels));
                label.add_css_class("accent");
                reset.set_visible(true);
            });
        });

        let action_name = def.action.to_string();
        let overrides_ref = shortcut_overrides.clone();
        let accel_label_ref = accel_label;
        reset_button.connect_clicked(move |btn| {
            overrides_ref.borrow_mut().remove(&action_name);
            let defaults = shortcuts::default_accels(&action_name);
            accel_label_ref.set_label(&format_accel_label(&defaults));
            accel_label_ref.remove_css_class("accent");
            btn.set_visible(false);
        });

        keyboard_group.add(&row);
    }

    let page = adw::PreferencesPage::new();
    page.set_icon_name(Some("preferences-system-symbolic"));
    page.set_title("General");
    page.add(&appearance_group);
    page.add(&terminal_group);
    page.add(&session_group);
    page.add(&keyboard_group);
    window.add(&page);

    let parent_window = parent.as_ref().clone();
    window.connect_close_request(move |_| {
        let terminal_theme_mode = match mode_row.selected() {
            1 => TerminalThemeMode::Light,
            2 => TerminalThemeMode::Dark,
            _ => TerminalThemeMode::System,
        };
        let new_prefs = Preferences {
            font: font_button.font_desc().map_or_else(|| prefs.font.clone(), |d| d.to_string()),
            color_scheme: legacy_color_scheme.clone(),
            terminal_theme_mode,
            light_color_scheme: light_scheme_row
                .selected_item()
                .and_then(|o| o.downcast::<gtk4::StringObject>().ok())
                .map_or_else(
                    || color_scheme::BUILTIN_LIGHT_SCHEME_NAME.into(),
                    |s| s.string().to_string(),
                ),
            dark_color_scheme: dark_scheme_row
                .selected_item()
                .and_then(|o| o.downcast::<gtk4::StringObject>().ok())
                .map_or_else(
                    || color_scheme::BUILTIN_DARK_SCHEME_NAME.into(),
                    |s| s.string().to_string(),
                ),
            scrollback_lines: scrollback_row.value() as i64,
            show_headerbar: headerbar_row.is_active(),
            scroll_on_keystroke: keystroke_row.is_active(),
            scroll_on_output: output_row.is_active(),
            audible_bell: bell_row.is_active(),
            visual_bell: visual_bell_row.is_active(),
            smart_clipboard: smart_clipboard_row.is_active(),
            trim_trailing_whitespace_on_copy: trim_whitespace_row.is_active(),
            default_session_folder: match folder_mode_row.selected() {
                1 => DefaultSessionFolder::CurrentSession,
                2 => DefaultSessionFolder::Custom(custom_folder_row.text().to_string()),
                _ => DefaultSessionFolder::Home,
            },
            pane_navigation_keys: prefs.pane_navigation_keys,
            keyboard_shortcuts: shortcut_overrides.borrow().clone(),
            auto_start_daemon: prefs.auto_start_daemon,
            reconnect_delay_secs: prefs.reconnect_delay_secs,
            paste_guard: paste_guard_row.is_active(),
            paste_guard_threshold: prefs.paste_guard_threshold,
        };
        if let Err(e) = preferences::save(&new_prefs) {
            tracing::error!("Failed to save preferences: {e}");
        }
        if let Ok(win) = parent_window.clone().downcast::<crate::window::Window>() {
            win.reapply_terminal_preferences();
        }
        glib::Propagation::Proceed
    });

    window.present();
}

fn load_scheme_names() -> Vec<String> {
    let mut names: Vec<String> =
        color_scheme::load_color_schemes().into_iter().map(|scheme| scheme.name).collect();
    names.sort();
    names.dedup();
    names
}

/// Format accelerator strings into a human-readable label.
fn format_accel_label(accels: &[String]) -> String {
    if accels.is_empty() {
        return "Disabled".into();
    }
    accels
        .iter()
        .filter_map(|a| {
            gtk4::accelerator_parse(a)
                .map(|(key, mods)| gtk4::accelerator_get_label(key, mods).to_string())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Show a dialog that captures a key combination from the user.
fn show_shortcut_capture_dialog(
    parent: &adw::PreferencesWindow,
    action_label: &str,
    on_captured: impl Fn(Vec<String>) + 'static,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(format!("Set shortcut for \u{201c}{action_label}\u{201d}"))
        .body("Press the desired key combination, or Backspace to disable.")
        .close_response("cancel")
        .build();
    dialog.add_response("cancel", "Cancel");

    let controller = gtk4::EventControllerKey::new();
    let dialog_ref = dialog.clone();
    controller.connect_key_pressed(move |_controller, keyval, _keycode, state| {
        // Ignore lone modifier presses.
        if matches!(
            keyval,
            gtk4::gdk::Key::Shift_L
                | gtk4::gdk::Key::Shift_R
                | gtk4::gdk::Key::Control_L
                | gtk4::gdk::Key::Control_R
                | gtk4::gdk::Key::Alt_L
                | gtk4::gdk::Key::Alt_R
                | gtk4::gdk::Key::Super_L
                | gtk4::gdk::Key::Super_R
                | gtk4::gdk::Key::Meta_L
                | gtk4::gdk::Key::Meta_R
                | gtk4::gdk::Key::Hyper_L
                | gtk4::gdk::Key::Hyper_R
        ) {
            return glib::Propagation::Proceed;
        }

        let mods = state
            & (gtk4::gdk::ModifierType::CONTROL_MASK
                | gtk4::gdk::ModifierType::SHIFT_MASK
                | gtk4::gdk::ModifierType::ALT_MASK
                | gtk4::gdk::ModifierType::SUPER_MASK);

        if keyval == gtk4::gdk::Key::BackSpace && mods.is_empty() {
            on_captured(vec![]);
            dialog_ref.close();
            return glib::Propagation::Stop;
        }

        if keyval == gtk4::gdk::Key::Escape && mods.is_empty() {
            dialog_ref.close();
            return glib::Propagation::Stop;
        }

        let accel = gtk4::accelerator_name(keyval, mods);
        on_captured(vec![accel.to_string()]);
        dialog_ref.close();
        glib::Propagation::Stop
    });

    dialog.add_controller(controller);
    dialog.present(Some(parent));
}
