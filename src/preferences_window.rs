use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::color_scheme;
use crate::preferences::{self, Preferences, TerminalThemeMode};

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

    let font_row =
        adw::ActionRow::builder().title("Font").subtitle(&prefs.font).activatable(true).build();
    let font_label = gtk4::Label::new(Some(&prefs.font));
    font_label.add_css_class("dim-label");
    font_row.add_suffix(&font_label);

    let win_ref = window.clone();
    let font_label_ref = font_label.clone();
    font_row.connect_activated(move |row| {
        let dialog = gtk4::FontDialog::new();
        let desc = gtk4::pango::FontDescription::from_string(&font_label_ref.label());
        let parent = win_ref.clone();
        let subtitle_row = row.clone();
        let fl = font_label_ref.clone();
        dialog.choose_font(
            Some(&parent),
            Some(&desc),
            gtk4::gio::Cancellable::NONE,
            move |result| {
                if let Ok(font_desc) = result {
                    let name = font_desc.to_string();
                    fl.set_label(&name);
                    subtitle_row.set_subtitle(&name);
                }
            },
        );
    });
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

    let opacity_row = adw::ActionRow::builder().title("Background opacity").build();
    let opacity_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 1.0, 0.05);
    opacity_scale.set_value(prefs.background_opacity);
    opacity_scale.set_hexpand(true);
    opacity_scale.set_valign(gtk4::Align::Center);
    opacity_scale.set_size_request(200, -1);
    opacity_row.add_suffix(&opacity_scale);
    appearance_group.add(&opacity_row);

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

    let smart_clipboard_row = adw::SwitchRow::new();
    smart_clipboard_row.set_title("Smart Ctrl+C / Ctrl+V");
    smart_clipboard_row.set_subtitle("Copy selected text with Ctrl+C and paste with Ctrl+V");
    smart_clipboard_row.set_active(prefs.smart_clipboard);
    terminal_group.add(&smart_clipboard_row);

    let page = adw::PreferencesPage::new();
    page.set_icon_name(Some("preferences-system-symbolic"));
    page.set_title("General");
    page.add(&appearance_group);
    page.add(&terminal_group);
    window.add(&page);

    let parent_window = parent.as_ref().clone();
    window.connect_close_request(move |_| {
        let terminal_theme_mode = match mode_row.selected() {
            1 => TerminalThemeMode::Light,
            2 => TerminalThemeMode::Dark,
            _ => TerminalThemeMode::System,
        };
        let new_prefs = Preferences {
            font: font_label.label().to_string(),
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
            smart_clipboard: smart_clipboard_row.is_active(),
            background_opacity: opacity_scale.value(),
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
