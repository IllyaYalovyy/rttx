use super::*;

impl Window {
    pub(super) fn set_input_sync(&self, enabled: bool) {
        let mut state = self.imp().state.borrow_mut();
        let active_idx = self.imp().sidebar_list.selected_row().map_or(0, |r| r.index() as usize);
        if let Some(session) = state.workspaces.get_mut(active_idx) {
            session.input_sync = enabled;
        }
    }

    pub(super) fn forward_input(&self, source_uuid: &str, text: &str) {
        let state = self.imp().state.borrow();
        let session = state
            .workspaces
            .iter()
            .find(|s| s.input_sync && s.layout.contains_terminal(source_uuid));
        let Some(session) = session else { return };
        let uuids = session.layout.terminal_uuids();
        drop(state);

        let terminals = self.imp().terminals.borrow();
        for uuid in &uuids {
            if uuid != source_uuid
                && let Some(term) = terminals.get(uuid)
            {
                term.vte().feed_child(text.as_bytes());
            }
        }
    }

    pub(super) fn apply_preferences_to_terminal(
        term: &TerminalWidget,
        prefs: &Preferences,
        font_desc: &gtk4::pango::FontDescription,
        scheme: Option<&color_scheme::ColorScheme>,
    ) {
        let vte = term.vte();
        vte.set_font(Some(font_desc));
        vte.set_scrollback_lines(prefs.scrollback_lines);
        vte.set_scroll_on_keystroke(prefs.scroll_on_keystroke);
        vte.set_scroll_on_output(prefs.scroll_on_output);
        vte.set_audible_bell(prefs.audible_bell);
        term.set_visual_bell(prefs.visual_bell);
        term.set_smart_clipboard(prefs.smart_clipboard);
        term.imp().header.set_visible(prefs.show_headerbar);
        if let Some(scheme) = scheme {
            term.apply_color_scheme(scheme);
        }
    }

    pub(crate) fn reapply_terminal_preferences(&self) {
        let prefs = preferences::load();

        // Reapply all keyboard shortcuts from preferences.
        if let Some(app) = self.application().and_downcast::<adw::Application>() {
            for def in crate::shortcuts::DEFAULT_SHORTCUTS {
                let accels =
                    crate::shortcuts::effective_accels(def.action, &prefs.keyboard_shortcuts);
                let accel_refs: Vec<&str> = accels.iter().map(AsRef::as_ref).collect();
                app.set_accels_for_action(&format!("win.{}", def.action), &accel_refs);
            }
        }

        let font_desc = gtk4::pango::FontDescription::from_string(&prefs.font);
        let is_dark = adw::StyleManager::default().is_dark();
        let effective_name = prefs.effective_color_scheme_name(is_dark);
        let scheme = color_scheme::load_color_scheme_by_name(effective_name).or_else(|| {
            let fallback = if is_dark {
                color_scheme::BUILTIN_DARK_SCHEME_NAME
            } else {
                color_scheme::BUILTIN_LIGHT_SCHEME_NAME
            };
            color_scheme::load_color_scheme_by_name(fallback)
        });
        let terminals: Vec<TerminalWidget> =
            self.imp().terminals.borrow().values().cloned().collect();
        for term in terminals {
            Self::apply_preferences_to_terminal(&term, &prefs, &font_desc, scheme.as_ref());
        }
        let persistent: Vec<PersistentPaneView> =
            self.imp().persistent_terminals.borrow().values().cloned().collect();
        for pane in persistent {
            self.apply_preferences_to_persistent_pane(&pane);
        }
    }

    pub(super) fn apply_preferences_to_persistent_pane(&self, pane: &PersistentPaneView) {
        let prefs = preferences::load();
        let font_desc = gtk4::pango::FontDescription::from_string(&prefs.font);
        pane.vte().set_font(Some(&font_desc));
        pane.vte().set_scrollback_lines(prefs.scrollback_lines);
        pane.vte().set_audible_bell(prefs.audible_bell);
        pane.set_visual_bell(prefs.visual_bell);
        pane.set_smart_clipboard(prefs.smart_clipboard);

        let is_dark = adw::StyleManager::default().is_dark();
        let effective_name = prefs.effective_color_scheme_name(is_dark);
        if let Some(scheme) =
            color_scheme::load_color_scheme_by_name(effective_name).or_else(|| {
                let fallback = if is_dark {
                    color_scheme::BUILTIN_DARK_SCHEME_NAME
                } else {
                    color_scheme::BUILTIN_LIGHT_SCHEME_NAME
                };
                color_scheme::load_color_scheme_by_name(fallback)
            })
        {
            pane.apply_color_scheme(&scheme);
        }
    }
}
