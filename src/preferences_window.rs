use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use rttx::preferences::{self, Preferences};

/// Build and present the preferences window.
pub fn show(parent: &impl IsA<gtk4::Window>) {
    let prefs = preferences::load();
    let window = adw::PreferencesWindow::new();
    window.set_transient_for(Some(parent.as_ref()));
    window.set_modal(true);
    window.set_title(Some("Preferences"));

    // ── Appearance group ─────────────────────────────────────────
    let appearance_group = adw::PreferencesGroup::new();
    appearance_group.set_title("Appearance");

    // Font
    let font_row = adw::ActionRow::builder()
        .title("Font")
        .subtitle(&prefs.font)
        .activatable(true)
        .build();
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

    // Color scheme
    let scheme_row = adw::ComboRow::builder()
        .title("Color scheme")
        .build();
    let schemes = load_scheme_names();
    let model = gtk4::StringList::new(&schemes.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    scheme_row.set_model(Some(&model));
    if let Some(pos) = schemes.iter().position(|s| s == &prefs.color_scheme) {
        scheme_row.set_selected(pos as u32);
    }
    appearance_group.add(&scheme_row);

    // Background opacity
    let opacity_row = adw::ActionRow::builder()
        .title("Background opacity")
        .build();
    let opacity_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 1.0, 0.05);
    opacity_scale.set_value(prefs.background_opacity);
    opacity_scale.set_hexpand(true);
    opacity_scale.set_valign(gtk4::Align::Center);
    opacity_scale.set_size_request(200, -1);
    opacity_row.add_suffix(&opacity_scale);
    appearance_group.add(&opacity_row);

    // ── Terminal group ───────────────────────────────────────────
    let terminal_group = adw::PreferencesGroup::new();
    terminal_group.set_title("Terminal");

    // Scrollback
    let scrollback_row = adw::SpinRow::with_range(0.0, 1_000_000.0, 1000.0);
    scrollback_row.set_title("Scrollback lines");
    scrollback_row.set_value(prefs.scrollback_lines as f64);
    terminal_group.add(&scrollback_row);

    // Toggle rows
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

    // ── Page ─────────────────────────────────────────────────────
    let page = adw::PreferencesPage::new();
    page.set_icon_name(Some("preferences-system-symbolic"));
    page.set_title("General");
    page.add(&appearance_group);
    page.add(&terminal_group);
    window.add(&page);

    // ── Save on close ────────────────────────────────────────────
    window.connect_close_request(move |_| {
        let new_prefs = Preferences {
            font: font_label.label().to_string(),
            color_scheme: scheme_row
                .selected_item()
                .and_then(|o| o.downcast::<gtk4::StringObject>().ok())
                .map(|s| s.string().to_string())
                .unwrap_or_else(|| "default".into()),
            scrollback_lines: scrollback_row.value() as i64,
            show_headerbar: headerbar_row.is_active(),
            scroll_on_keystroke: keystroke_row.is_active(),
            scroll_on_output: output_row.is_active(),
            audible_bell: bell_row.is_active(),
            background_opacity: opacity_scale.value(),
        };
        if let Err(e) = preferences::save(&new_prefs) {
            log::error!("Failed to save preferences: {e}");
        }
        glib::Propagation::Proceed
    });

    window.present();
}

fn load_scheme_names() -> Vec<String> {
    let mut names = vec!["default".to_string()];
    let mut dir = glib::user_config_dir();
    dir.push(rttx::config::CONFIG_DIR);
    dir.push(rttx::config::SCHEMES_DIR);
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if entry.path().extension().is_some_and(|e| e == "json") {
                if let Some(stem) = entry.path().file_stem() {
                    names.push(stem.to_string_lossy().into_owned());
                }
            }
        }
    }
    names.sort();
    names.dedup();
    names
}
