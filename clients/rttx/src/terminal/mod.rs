pub mod handle;
#[doc(hidden)]
pub mod links;
pub mod persistent_widget;
pub mod widget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalInputBackend {
    Direct,
    Managed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TerminalKeyAction {
    CopySelection,
    PasteClipboard,
    PassThrough,
    ForwardToPty(Vec<u8>),
}

fn normalized_shortcut_modifiers(modifiers: gtk4::gdk::ModifierType) -> gtk4::gdk::ModifierType {
    let shortcut_mask = gtk4::gdk::ModifierType::SHIFT_MASK
        | gtk4::gdk::ModifierType::CONTROL_MASK
        | gtk4::gdk::ModifierType::ALT_MASK
        | gtk4::gdk::ModifierType::SUPER_MASK
        | gtk4::gdk::ModifierType::HYPER_MASK
        | gtk4::gdk::ModifierType::META_MASK;

    modifiers & shortcut_mask
}

fn should_pass_through_window_accelerator(
    key: gtk4::gdk::Key,
    normalized: gtk4::gdk::ModifierType,
) -> bool {
    let ctrl = gtk4::gdk::ModifierType::CONTROL_MASK;
    let shift = gtk4::gdk::ModifierType::SHIFT_MASK;
    let alt = gtk4::gdk::ModifierType::ALT_MASK;
    let desktop_shortcuts = gtk4::gdk::ModifierType::SUPER_MASK
        | gtk4::gdk::ModifierType::HYPER_MASK
        | gtk4::gdk::ModifierType::META_MASK;

    if !(normalized & desktop_shortcuts).is_empty() {
        return true;
    }

    (normalized == (ctrl | shift)
        && matches!(
            key,
            gtk4::gdk::Key::c
                | gtk4::gdk::Key::C
                | gtk4::gdk::Key::v
                | gtk4::gdk::Key::V
                | gtk4::gdk::Key::w
                | gtk4::gdk::Key::W
                | gtk4::gdk::Key::e
                | gtk4::gdk::Key::E
                | gtk4::gdk::Key::o
                | gtk4::gdk::Key::O
                | gtk4::gdk::Key::f
                | gtk4::gdk::Key::F
                | gtk4::gdk::Key::n
                | gtk4::gdk::Key::N
                | gtk4::gdk::Key::b
                | gtk4::gdk::Key::B
                | gtk4::gdk::Key::i
                | gtk4::gdk::Key::I
                | gtk4::gdk::Key::t
                | gtk4::gdk::Key::T
                | gtk4::gdk::Key::Tab
                | gtk4::gdk::Key::ISO_Left_Tab
        ))
        || (normalized == (ctrl | shift | alt)
            && matches!(key, gtk4::gdk::Key::t | gtk4::gdk::Key::T))
}

const fn encode_control_character(ch: char) -> Option<u8> {
    match ch {
        'a'..='z' | 'A'..='Z' => Some((ch.to_ascii_uppercase() as u8) & 0x1f),
        ' ' | '@' | '`' | '2' => Some(0x00),
        '[' | '{' | '3' => Some(0x1b),
        '\\' | '|' | '4' => Some(0x1c),
        ']' | '}' | '5' => Some(0x1d),
        '^' | '~' | '6' => Some(0x1e),
        '_' | '7' | '/' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

fn encode_terminal_key_input(
    key: gtk4::gdk::Key,
    state: gtk4::gdk::ModifierType,
) -> Option<Vec<u8>> {
    let ctrl = state.contains(gtk4::gdk::ModifierType::CONTROL_MASK);
    let alt = state.contains(gtk4::gdk::ModifierType::ALT_MASK);
    let base = match key {
        gtk4::gdk::Key::Return | gtk4::gdk::Key::KP_Enter => Some(vec![b'\r']),
        gtk4::gdk::Key::BackSpace => Some(vec![0x7f]),
        gtk4::gdk::Key::Tab | gtk4::gdk::Key::KP_Tab => Some(vec![b'\t']),
        gtk4::gdk::Key::ISO_Left_Tab => Some(b"\x1b[Z".to_vec()),
        gtk4::gdk::Key::Escape => Some(vec![0x1b]),
        gtk4::gdk::Key::Up | gtk4::gdk::Key::KP_Up => Some(b"\x1b[A".to_vec()),
        gtk4::gdk::Key::Down | gtk4::gdk::Key::KP_Down => Some(b"\x1b[B".to_vec()),
        gtk4::gdk::Key::Right | gtk4::gdk::Key::KP_Right => Some(b"\x1b[C".to_vec()),
        gtk4::gdk::Key::Left | gtk4::gdk::Key::KP_Left => Some(b"\x1b[D".to_vec()),
        gtk4::gdk::Key::Home | gtk4::gdk::Key::KP_Home => Some(b"\x1b[H".to_vec()),
        gtk4::gdk::Key::End | gtk4::gdk::Key::KP_End => Some(b"\x1b[F".to_vec()),
        gtk4::gdk::Key::Insert | gtk4::gdk::Key::KP_Insert => Some(b"\x1b[2~".to_vec()),
        gtk4::gdk::Key::Delete | gtk4::gdk::Key::KP_Delete => Some(b"\x1b[3~".to_vec()),
        gtk4::gdk::Key::Page_Up | gtk4::gdk::Key::KP_Page_Up => Some(b"\x1b[5~".to_vec()),
        gtk4::gdk::Key::Page_Down | gtk4::gdk::Key::KP_Page_Down => Some(b"\x1b[6~".to_vec()),
        _ => {
            let ch = key.to_unicode()?;
            if ctrl {
                Some(vec![encode_control_character(ch)?])
            } else {
                Some(ch.to_string().into_bytes())
            }
        }
    }?;

    if alt {
        let mut prefixed = vec![0x1b];
        prefixed.extend(base);
        Some(prefixed)
    } else {
        Some(base)
    }
}

fn smart_clipboard_action(
    key: gtk4::gdk::Key,
    normalized: gtk4::gdk::ModifierType,
    has_selection: bool,
    smart_clipboard_enabled: bool,
) -> Option<TerminalKeyAction> {
    if smart_clipboard_enabled && normalized == gtk4::gdk::ModifierType::CONTROL_MASK {
        match key {
            gtk4::gdk::Key::c | gtk4::gdk::Key::C if has_selection => {
                return Some(TerminalKeyAction::CopySelection);
            }
            gtk4::gdk::Key::v | gtk4::gdk::Key::V => {
                return Some(TerminalKeyAction::PasteClipboard);
            }
            _ => {}
        }
    }

    None
}

fn terminal_key_action(
    backend: TerminalInputBackend,
    key: gtk4::gdk::Key,
    modifiers: gtk4::gdk::ModifierType,
    has_selection: bool,
    smart_clipboard_enabled: bool,
) -> TerminalKeyAction {
    let normalized = normalized_shortcut_modifiers(modifiers);

    if let Some(action) =
        smart_clipboard_action(key, normalized, has_selection, smart_clipboard_enabled)
    {
        return action;
    }

    // Managed terminals intercept all keys before VTE sees them, so
    // Ctrl+Shift+C/V (standard terminal copy/paste) must be handled
    // explicitly — VTE never gets a chance to process them.
    if backend == TerminalInputBackend::Managed
        && normalized
            == (gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK)
    {
        match key {
            gtk4::gdk::Key::c | gtk4::gdk::Key::C if has_selection => {
                return TerminalKeyAction::CopySelection;
            }
            gtk4::gdk::Key::v | gtk4::gdk::Key::V => {
                return TerminalKeyAction::PasteClipboard;
            }
            _ => {}
        }
    }

    if should_pass_through_window_accelerator(key, normalized) {
        return TerminalKeyAction::PassThrough;
    }

    match backend {
        TerminalInputBackend::Direct => TerminalKeyAction::PassThrough,
        TerminalInputBackend::Managed => encode_terminal_key_input(key, modifiers)
            .map_or(TerminalKeyAction::PassThrough, TerminalKeyAction::ForwardToPty),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TerminalInputBackend, TerminalKeyAction, encode_terminal_key_input, terminal_key_action,
    };

    #[test]
    fn direct_and_managed_share_clipboard_policy() {
        let modifiers = gtk4::gdk::ModifierType::CONTROL_MASK
            | gtk4::gdk::ModifierType::LOCK_MASK
            | gtk4::gdk::ModifierType::BUTTON1_MASK;

        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Direct,
                gtk4::gdk::Key::v,
                modifiers,
                false,
                true,
            ),
            TerminalKeyAction::PasteClipboard
        );
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::v,
                modifiers,
                false,
                true,
            ),
            TerminalKeyAction::PasteClipboard
        );
    }

    #[test]
    fn direct_and_managed_preserve_window_accelerators() {
        let modifiers = gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK;

        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Direct,
                gtk4::gdk::Key::F,
                modifiers,
                false,
                false,
            ),
            TerminalKeyAction::PassThrough
        );
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::F,
                modifiers,
                false,
                false,
            ),
            TerminalKeyAction::PassThrough
        );
    }

    #[test]
    fn super_modified_keys_are_left_for_window_or_desktop_shortcuts() {
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Direct,
                gtk4::gdk::Key::c,
                gtk4::gdk::ModifierType::SUPER_MASK,
                false,
                false,
            ),
            TerminalKeyAction::PassThrough
        );
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::c,
                gtk4::gdk::ModifierType::SUPER_MASK,
                false,
                false,
            ),
            TerminalKeyAction::PassThrough
        );
    }

    #[test]
    fn managed_input_still_forwards_shell_control_and_alt_sequences() {
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::c,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                false,
                true,
            ),
            TerminalKeyAction::ForwardToPty(vec![0x03])
        );
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::x,
                gtk4::gdk::ModifierType::ALT_MASK,
                false,
                false,
            ),
            TerminalKeyAction::ForwardToPty(b"\x1bx".to_vec())
        );
    }

    #[test]
    fn managed_ctrl_v_prefers_clipboard_paste_over_shell_syn() {
        let modifiers = gtk4::gdk::ModifierType::CONTROL_MASK
            | gtk4::gdk::ModifierType::LOCK_MASK
            | gtk4::gdk::ModifierType::BUTTON1_MASK;

        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::v,
                modifiers,
                false,
                true,
            ),
            TerminalKeyAction::PasteClipboard
        );
    }

    #[test]
    fn encode_terminal_key_input_maps_basic_shell_keys() {
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::a, gtk4::gdk::ModifierType::empty()),
            Some(vec![b'a'])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Return, gtk4::gdk::ModifierType::empty()),
            Some(vec![b'\r'])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::BackSpace, gtk4::gdk::ModifierType::empty()),
            Some(vec![0x7f])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Left, gtk4::gdk::ModifierType::empty()),
            Some(b"\x1b[D".to_vec())
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::c, gtk4::gdk::ModifierType::CONTROL_MASK),
            Some(vec![0x03])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::x, gtk4::gdk::ModifierType::ALT_MASK),
            Some(b"\x1bx".to_vec())
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Shift_L, gtk4::gdk::ModifierType::SHIFT_MASK,),
            None
        );
    }

    /// Ctrl+Shift+V must paste in managed terminals even with smart clipboard off.
    #[test]
    fn managed_ctrl_shift_v_pastes_without_smart_clipboard() {
        let modifiers = gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK;

        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::V,
                modifiers,
                false,
                false, // smart clipboard OFF
            ),
            TerminalKeyAction::PasteClipboard
        );
    }

    /// Ctrl+Shift+C must copy in managed terminals when text is selected.
    #[test]
    fn managed_ctrl_shift_c_copies_without_smart_clipboard() {
        let modifiers = gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK;

        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::C,
                modifiers,
                true,  // has selection
                false, // smart clipboard OFF
            ),
            TerminalKeyAction::CopySelection
        );
    }
}

#[cfg(test)]
mod search_tests {
    /// The PCRE2 literal escape pattern wraps user input safely so special
    /// regex characters are not interpreted.
    #[test]
    fn search_regex_literal_escaping() {
        let text = "hello.world*[test](foo)+";
        let pattern = format!("\\Q{text}\\E");
        assert!(pattern.starts_with("\\Q"));
        assert!(pattern.ends_with("\\E"));
        assert!(pattern.contains(text));
    }
}
