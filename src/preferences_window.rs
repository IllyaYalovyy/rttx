use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::color_scheme;
use crate::preferences::{self, DefaultSessionFolder, Preferences, TerminalThemeMode};

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

    let session_group = adw::PreferencesGroup::new();
    session_group.set_title("Sessions");

    let folder_mode_row = adw::ComboRow::builder().title("Default folder for new sessions").build();
    let folder_mode_names = ["Home directory", "Same as current session", "Custom path"];
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

    let page = adw::PreferencesPage::new();
    page.set_icon_name(Some("preferences-system-symbolic"));
    page.set_title("General");
    page.add(&appearance_group);
    page.add(&terminal_group);
    page.add(&session_group);
    window.add(&page);

    let parent_window = parent.as_ref().clone();
    window.connect_close_request(move |_| {
        let terminal_theme_mode = match mode_row.selected() {
            1 => TerminalThemeMode::Light,
            2 => TerminalThemeMode::Dark,
            _ => TerminalThemeMode::System,
        };
        let new_prefs = Preferences {
            font: font_button
                .font_desc()
                .map_or_else(|| prefs.font.clone(), |d| d.to_string()),
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
            default_session_folder: match folder_mode_row.selected() {
                1 => DefaultSessionFolder::CurrentSession,
                2 => DefaultSessionFolder::Custom(custom_folder_row.text().to_string()),
                _ => DefaultSessionFolder::Home,
            },
        };
        if let Err(e) = preferences::save(&new_prefs) {
            log::error!("Failed to save preferences: {e}");
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
