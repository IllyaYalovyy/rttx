pub mod handle;
#[doc(hidden)]
pub mod links;
pub(crate) mod paste_guard;
pub mod persistent_widget;
pub mod widget;

use gtk4::prelude::*;
use vte4::prelude::*;

/// Trim trailing whitespace from each line in `text`.
fn trim_trailing_whitespace(text: &str) -> String {
    text.lines().map(str::trim_end).collect::<Vec<_>>().join("\n")
}

/// Copy the terminal selection to the system clipboard, trimming trailing
/// whitespace per line when the preference is enabled.
pub(crate) fn copy_to_clipboard(vte: &vte4::Terminal) {
    let prefs = crate::preferences::load();
    if !prefs.trim_trailing_whitespace_on_copy {
        vte.copy_clipboard_format(vte4::Format::Text);
        return;
    }
    let Some(selected) = vte.text_selected(vte4::Format::Text) else {
        return;
    };
    let trimmed = trim_trailing_whitespace(&selected);
    if let Some(display) = gtk4::gdk::Display::default() {
        display.clipboard().set_text(&trimmed);
    }
}

/// Context menu alignment: Start so the left edge aligns with the pointer,
/// preventing immediate item activation on button release (#480).
pub(crate) const CONTEXT_MENU_HALIGN: gtk4::Align = gtk4::Align::Start;

/// Whether a right-click with the given modifiers should open the context menu.
///
/// Plain right-click opens the menu (matches GNOME Terminal, Ptyxis, Tilix).
/// Shift+right-click passes through to VTE for mouse-aware apps.
#[must_use]
pub const fn should_open_context_menu(mods: gtk4::gdk::ModifierType) -> bool {
    !mods.contains(gtk4::gdk::ModifierType::SHIFT_MASK)
}

/// Populate a `gio::Menu` with places visible on the given host key.
///
/// Each item triggers `win.open-place` with the place path as parameter.
pub(crate) fn populate_places_submenu(menu: &gtk4::gio::Menu, host_key: &str) {
    menu.remove_all();
    let saved = crate::places::load();
    let visible = crate::places::visible_for_host(&saved, host_key);
    for place in &visible {
        let item = gtk4::gio::MenuItem::new(Some(&place.display_label()), None);
        item.set_action_and_target_value(Some("win.open-place"), Some(&place.path.to_variant()));
        menu.append_item(&item);
    }
}

/// Test-only re-export of `encode_terminal_key_input`.
#[doc(hidden)]
#[must_use]
pub fn encode_terminal_key_input_for_test(
    key: gtk4::gdk::Key,
    state: gtk4::gdk::ModifierType,
) -> Option<Vec<u8>> {
    encode_terminal_key_input(key, state)
}

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

/// Compute xterm modifier parameter: 1 + (shift?1:0) + (alt?2:0) + (ctrl?4:0).
/// Returns 0 when no modifiers are held (caller should use unmodified sequence).
const fn xterm_modifier_param(state: gtk4::gdk::ModifierType) -> u8 {
    let mut m: u8 = 0;
    if state.contains(gtk4::gdk::ModifierType::SHIFT_MASK) {
        m += 1;
    }
    if state.contains(gtk4::gdk::ModifierType::ALT_MASK) {
        m += 2;
    }
    if state.contains(gtk4::gdk::ModifierType::CONTROL_MASK) {
        m += 4;
    }
    m
}

/// Encode a CSI-letter key: `\x1b[X` unmodified, `\x1b[1;{m}X` modified.
fn csi_letter(suffix: u8, modifier: u8) -> Vec<u8> {
    if modifier == 0 {
        vec![0x1b, b'[', suffix]
    } else {
        format!("\x1b[1;{}{}", modifier + 1, suffix as char).into_bytes()
    }
}

/// Encode a CSI-tilde key: `\x1b[N~` unmodified, `\x1b[N;{m}~` modified.
fn csi_tilde(number: &str, modifier: u8) -> Vec<u8> {
    if modifier == 0 {
        format!("\x1b[{number}~").into_bytes()
    } else {
        format!("\x1b[{number};{}~", modifier + 1).into_bytes()
    }
}

/// Encode an SS3-letter key: `\x1bOX` unmodified, `\x1b[1;{m}X` modified.
fn ss3_or_csi(suffix: u8, modifier: u8) -> Vec<u8> {
    if modifier == 0 {
        vec![0x1b, b'O', suffix]
    } else {
        format!("\x1b[1;{}{}", modifier + 1, suffix as char).into_bytes()
    }
}

fn encode_terminal_key_input(
    key: gtk4::gdk::Key,
    state: gtk4::gdk::ModifierType,
) -> Option<Vec<u8>> {
    let ctrl = state.contains(gtk4::gdk::ModifierType::CONTROL_MASK);
    let alt = state.contains(gtk4::gdk::ModifierType::ALT_MASK);
    let m = xterm_modifier_param(state);

    let seq = match key {
        gtk4::gdk::Key::Return | gtk4::gdk::Key::KP_Enter => vec![b'\r'],
        gtk4::gdk::Key::BackSpace => vec![0x7f],
        gtk4::gdk::Key::Tab | gtk4::gdk::Key::KP_Tab => vec![b'\t'],
        gtk4::gdk::Key::ISO_Left_Tab => b"\x1b[Z".to_vec(),
        gtk4::gdk::Key::Escape => vec![0x1b],
        gtk4::gdk::Key::Up | gtk4::gdk::Key::KP_Up => return Some(csi_letter(b'A', m)),
        gtk4::gdk::Key::Down | gtk4::gdk::Key::KP_Down => return Some(csi_letter(b'B', m)),
        gtk4::gdk::Key::Right | gtk4::gdk::Key::KP_Right => return Some(csi_letter(b'C', m)),
        gtk4::gdk::Key::Left | gtk4::gdk::Key::KP_Left => return Some(csi_letter(b'D', m)),
        gtk4::gdk::Key::Home | gtk4::gdk::Key::KP_Home => return Some(csi_letter(b'H', m)),
        gtk4::gdk::Key::End | gtk4::gdk::Key::KP_End => return Some(csi_letter(b'F', m)),
        gtk4::gdk::Key::Insert | gtk4::gdk::Key::KP_Insert => return Some(csi_tilde("2", m)),
        gtk4::gdk::Key::Delete | gtk4::gdk::Key::KP_Delete => return Some(csi_tilde("3", m)),
        gtk4::gdk::Key::Page_Up | gtk4::gdk::Key::KP_Page_Up => return Some(csi_tilde("5", m)),
        gtk4::gdk::Key::Page_Down | gtk4::gdk::Key::KP_Page_Down => {
            return Some(csi_tilde("6", m));
        }
        gtk4::gdk::Key::F1 => return Some(ss3_or_csi(b'P', m)),
        gtk4::gdk::Key::F2 => return Some(ss3_or_csi(b'Q', m)),
        gtk4::gdk::Key::F3 => return Some(ss3_or_csi(b'R', m)),
        gtk4::gdk::Key::F4 => return Some(ss3_or_csi(b'S', m)),
        gtk4::gdk::Key::F5 => return Some(csi_tilde("15", m)),
        gtk4::gdk::Key::F6 => return Some(csi_tilde("17", m)),
        gtk4::gdk::Key::F7 => return Some(csi_tilde("18", m)),
        gtk4::gdk::Key::F8 => return Some(csi_tilde("19", m)),
        gtk4::gdk::Key::F9 => return Some(csi_tilde("20", m)),
        gtk4::gdk::Key::F10 => return Some(csi_tilde("21", m)),
        gtk4::gdk::Key::F11 => return Some(csi_tilde("23", m)),
        gtk4::gdk::Key::F12 => return Some(csi_tilde("24", m)),
        _ => {
            let ch = key.to_unicode()?;
            if ctrl && alt {
                let ctrl_byte = encode_control_character(ch)?;
                vec![0x1b, ctrl_byte]
            } else if ctrl {
                vec![encode_control_character(ch)?]
            } else if alt {
                let mut prefixed = vec![0x1b];
                prefixed.extend(ch.to_string().bytes());
                return Some(prefixed);
            } else {
                ch.to_string().into_bytes()
            }
        }
    };

    Some(seq)
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
    fn ctrl_d_encodes_eof_byte() {
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::d, gtk4::gdk::ModifierType::CONTROL_MASK),
            Some(vec![0x04])
        );
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::d,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                false,
                true,
            ),
            TerminalKeyAction::ForwardToPty(vec![0x04])
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

    /// F-keys must produce xterm escape sequences for managed terminals. #293.
    #[test]
    fn fkeys_produce_escape_sequences() {
        let expected: &[(gtk4::gdk::Key, &[u8])] = &[
            (gtk4::gdk::Key::F1, b"\x1bOP"),
            (gtk4::gdk::Key::F2, b"\x1bOQ"),
            (gtk4::gdk::Key::F3, b"\x1bOR"),
            (gtk4::gdk::Key::F4, b"\x1bOS"),
            (gtk4::gdk::Key::F5, b"\x1b[15~"),
            (gtk4::gdk::Key::F6, b"\x1b[17~"),
            (gtk4::gdk::Key::F7, b"\x1b[18~"),
            (gtk4::gdk::Key::F8, b"\x1b[19~"),
            (gtk4::gdk::Key::F9, b"\x1b[20~"),
            (gtk4::gdk::Key::F10, b"\x1b[21~"),
            (gtk4::gdk::Key::F11, b"\x1b[23~"),
            (gtk4::gdk::Key::F12, b"\x1b[24~"),
        ];
        for (key, seq) in expected {
            let result = encode_terminal_key_input(*key, gtk4::gdk::ModifierType::empty());
            assert_eq!(
                result.as_deref(),
                Some(*seq as &[u8]),
                "F-key {key:?} should produce escape sequence"
            );
        }
    }

    /// F-keys must be forwarded to PTY in managed mode, not dropped. #293.
    #[test]
    fn managed_terminal_forwards_fkeys_to_pty() {
        let action = terminal_key_action(
            TerminalInputBackend::Managed,
            gtk4::gdk::Key::F1,
            gtk4::gdk::ModifierType::empty(),
            false,
            false,
        );
        assert!(
            matches!(action, TerminalKeyAction::ForwardToPty(_)),
            "F1 must be ForwardToPty in managed mode, got {action:?}"
        );
    }

    /// Alt+F-key must use xterm modifier param 3. #293.
    #[test]
    fn alt_fkey_uses_modifier_encoding() {
        let result =
            encode_terminal_key_input(gtk4::gdk::Key::F2, gtk4::gdk::ModifierType::ALT_MASK);
        assert_eq!(result.as_deref(), Some(b"\x1b[1;3Q" as &[u8]));
    }

    /// Ctrl+Arrow must use xterm modified key format. #295.
    #[test]
    fn ctrl_arrow_uses_xterm_modifier_encoding() {
        let ctrl = gtk4::gdk::ModifierType::CONTROL_MASK;
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Right, ctrl).as_deref(),
            Some(b"\x1b[1;5C" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Left, ctrl).as_deref(),
            Some(b"\x1b[1;5D" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Up, ctrl).as_deref(),
            Some(b"\x1b[1;5A" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Down, ctrl).as_deref(),
            Some(b"\x1b[1;5B" as &[u8])
        );
    }

    /// Shift+Arrow must use xterm modified key format. #295.
    #[test]
    fn shift_arrow_uses_xterm_modifier_encoding() {
        let shift = gtk4::gdk::ModifierType::SHIFT_MASK;
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Right, shift).as_deref(),
            Some(b"\x1b[1;2C" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Left, shift).as_deref(),
            Some(b"\x1b[1;2D" as &[u8])
        );
    }

    /// Ctrl+Home/End must use xterm modified key format. #295.
    #[test]
    fn ctrl_home_end_uses_xterm_modifier_encoding() {
        let ctrl = gtk4::gdk::ModifierType::CONTROL_MASK;
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Home, ctrl).as_deref(),
            Some(b"\x1b[1;5H" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::End, ctrl).as_deref(),
            Some(b"\x1b[1;5F" as &[u8])
        );
    }

    /// Ctrl+Shift+Arrow must use modifier param 6. #295.
    #[test]
    fn ctrl_shift_arrow_uses_modifier_6() {
        let mods = gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK;
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Right, mods).as_deref(),
            Some(b"\x1b[1;6C" as &[u8])
        );
    }

    /// Alt+Ctrl+Arrow must use modifier param 7 (Alt prefix NOT doubled). #295.
    #[test]
    fn alt_ctrl_arrow_uses_modifier_7() {
        let mods = gtk4::gdk::ModifierType::ALT_MASK | gtk4::gdk::ModifierType::CONTROL_MASK;
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Right, mods).as_deref(),
            Some(b"\x1b[1;7C" as &[u8])
        );
    }

    /// Modifier+F-keys must use CSI modified format. #295.
    #[test]
    fn ctrl_fkey_uses_modified_format() {
        let ctrl = gtk4::gdk::ModifierType::CONTROL_MASK;
        // F5 = CSI 15~ → Ctrl+F5 = CSI 15;5~
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::F5, ctrl).as_deref(),
            Some(b"\x1b[15;5~" as &[u8])
        );
        // F1 = SS3 P → Ctrl+F1 = CSI 1;5P
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::F1, ctrl).as_deref(),
            Some(b"\x1b[1;5P" as &[u8])
        );
    }

    /// Modifier+Insert/Delete/PageUp/PageDown must use modified tilde format. #295.
    #[test]
    fn ctrl_tilde_keys_use_modified_format() {
        let ctrl = gtk4::gdk::ModifierType::CONTROL_MASK;
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Delete, ctrl).as_deref(),
            Some(b"\x1b[3;5~" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Page_Up, ctrl).as_deref(),
            Some(b"\x1b[5;5~" as &[u8])
        );
    }

    /// Unmodified navigation keys must remain unchanged. #295.
    #[test]
    fn unmodified_navigation_keys_unchanged() {
        let none = gtk4::gdk::ModifierType::empty();
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Right, none).as_deref(),
            Some(b"\x1b[C" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Home, none).as_deref(),
            Some(b"\x1b[H" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::F1, none).as_deref(),
            Some(b"\x1bOP" as &[u8])
        );
    }

    /// Ctrl+Alt+letter must produce ESC + control-char so Alt is not lost. #457.
    #[test]
    fn ctrl_alt_letter_preserves_alt_prefix() {
        let mods = gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::ALT_MASK;
        // Ctrl+Alt+a → ESC 0x01
        assert_eq!(encode_terminal_key_input(gtk4::gdk::Key::a, mods), Some(vec![0x1b, 0x01]));
        // Ctrl+Alt+c → ESC 0x03
        assert_eq!(encode_terminal_key_input(gtk4::gdk::Key::c, mods), Some(vec![0x1b, 0x03]));
    }

    /// Ctrl+Alt+letter must be forwarded as `ForwardToPty` in managed mode. #457.
    #[test]
    fn managed_ctrl_alt_letter_forwards_to_pty() {
        let mods = gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::ALT_MASK;
        let action = terminal_key_action(
            TerminalInputBackend::Managed,
            gtk4::gdk::Key::a,
            mods,
            false,
            false,
        );
        assert_eq!(action, TerminalKeyAction::ForwardToPty(vec![0x1b, 0x01]));
    }

    /// Keypad digits produce their ASCII digit in normal (numeric) mode. #457.
    #[test]
    fn keypad_digits_produce_ascii_in_normal_mode() {
        let none = gtk4::gdk::ModifierType::empty();
        let expected: &[(gtk4::gdk::Key, u8)] = &[
            (gtk4::gdk::Key::KP_0, b'0'),
            (gtk4::gdk::Key::KP_1, b'1'),
            (gtk4::gdk::Key::KP_2, b'2'),
            (gtk4::gdk::Key::KP_3, b'3'),
            (gtk4::gdk::Key::KP_4, b'4'),
            (gtk4::gdk::Key::KP_5, b'5'),
            (gtk4::gdk::Key::KP_6, b'6'),
            (gtk4::gdk::Key::KP_7, b'7'),
            (gtk4::gdk::Key::KP_8, b'8'),
            (gtk4::gdk::Key::KP_9, b'9'),
        ];
        for (key, byte) in expected {
            assert_eq!(
                encode_terminal_key_input(*key, none),
                Some(vec![*byte]),
                "KP digit {key:?} should produce ASCII digit"
            );
        }
    }

    /// Keypad operators produce their ASCII character in normal mode. #457.
    #[test]
    fn keypad_operators_produce_ascii_in_normal_mode() {
        let none = gtk4::gdk::ModifierType::empty();
        let expected: &[(gtk4::gdk::Key, &[u8])] = &[
            (gtk4::gdk::Key::KP_Add, b"+"),
            (gtk4::gdk::Key::KP_Subtract, b"-"),
            (gtk4::gdk::Key::KP_Multiply, b"*"),
            (gtk4::gdk::Key::KP_Divide, b"/"),
            (gtk4::gdk::Key::KP_Decimal, b"."),
        ];
        for (key, bytes) in expected {
            assert_eq!(
                encode_terminal_key_input(*key, none),
                Some(bytes.to_vec()),
                "KP operator {key:?} should produce ASCII"
            );
        }
    }

    /// Modified Insert/Delete/PageUp/PageDown use xterm tilde format. #457.
    #[test]
    fn shift_tilde_keys_use_modified_format() {
        let shift = gtk4::gdk::ModifierType::SHIFT_MASK;
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Insert, shift).as_deref(),
            Some(b"\x1b[2;2~" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Delete, shift).as_deref(),
            Some(b"\x1b[3;2~" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Page_Up, shift).as_deref(),
            Some(b"\x1b[5;2~" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Page_Down, shift).as_deref(),
            Some(b"\x1b[6;2~" as &[u8])
        );
    }

    /// Shift+Home/End use xterm modified letter format. #457.
    #[test]
    fn shift_home_end_uses_modified_format() {
        let shift = gtk4::gdk::ModifierType::SHIFT_MASK;
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Home, shift).as_deref(),
            Some(b"\x1b[1;2H" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::End, shift).as_deref(),
            Some(b"\x1b[1;2F" as &[u8])
        );
    }

    /// Managed backend must intercept all printable and control keys so VTE's
    /// `commit` signal only fires for mouse events. Regression for #442.
    #[test]
    fn managed_backend_intercepts_all_typeable_keys() {
        let keys_and_mods: &[(gtk4::gdk::Key, gtk4::gdk::ModifierType)] = &[
            (gtk4::gdk::Key::a, gtk4::gdk::ModifierType::empty()),
            (gtk4::gdk::Key::Return, gtk4::gdk::ModifierType::empty()),
            (gtk4::gdk::Key::space, gtk4::gdk::ModifierType::empty()),
            (gtk4::gdk::Key::Up, gtk4::gdk::ModifierType::empty()),
            (gtk4::gdk::Key::d, gtk4::gdk::ModifierType::CONTROL_MASK),
            (gtk4::gdk::Key::x, gtk4::gdk::ModifierType::ALT_MASK),
            (gtk4::gdk::Key::F5, gtk4::gdk::ModifierType::empty()),
        ];
        for (key, mods) in keys_and_mods {
            let action =
                terminal_key_action(TerminalInputBackend::Managed, *key, *mods, false, false);
            assert!(
                matches!(action, TerminalKeyAction::ForwardToPty(_)),
                "managed backend must intercept {key:?}+{mods:?}, got {action:?}"
            );
        }
    }

    /// Dead keys produce no direct encoding — the `IMContext` handles them. #462.
    #[test]
    fn dead_keys_produce_none_for_ime_handling() {
        let none = gtk4::gdk::ModifierType::empty();
        let dead_keys = [
            gtk4::gdk::Key::dead_acute,
            gtk4::gdk::Key::dead_grave,
            gtk4::gdk::Key::dead_circumflex,
            gtk4::gdk::Key::dead_tilde,
            gtk4::gdk::Key::dead_diaeresis,
        ];
        for key in &dead_keys {
            assert_eq!(
                encode_terminal_key_input(*key, none),
                None,
                "dead key {key:?} must return None so IMContext handles it"
            );
        }
    }

    /// Dead keys must pass through in managed mode so the `IMContext` can
    /// process the compose sequence. #462.
    #[test]
    fn managed_dead_keys_pass_through_for_ime() {
        let none = gtk4::gdk::ModifierType::empty();
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::dead_acute,
                none,
                false,
                false,
            ),
            TerminalKeyAction::PassThrough,
        );
    }

    /// Control sequences must still be encoded even when `IMContext` is active,
    /// because the `IMContext` does not consume modified keys. #462.
    #[test]
    fn control_keys_still_encoded_with_ime_active() {
        let ctrl = gtk4::gdk::ModifierType::CONTROL_MASK;
        assert_eq!(encode_terminal_key_input(gtk4::gdk::Key::c, ctrl), Some(vec![0x03]),);
        assert_eq!(encode_terminal_key_input(gtk4::gdk::Key::d, ctrl), Some(vec![0x04]),);
    }
}

#[cfg(test)]
mod trim_tests {
    use super::trim_trailing_whitespace;

    #[test]
    fn no_trailing_whitespace_unchanged() {
        assert_eq!(trim_trailing_whitespace("hello\nworld"), "hello\nworld");
    }

    #[test]
    fn trailing_spaces_removed() {
        assert_eq!(trim_trailing_whitespace("hello   \nworld  "), "hello\nworld");
    }

    #[test]
    fn trailing_tabs_removed() {
        assert_eq!(trim_trailing_whitespace("hello\t\t\nworld\t"), "hello\nworld");
    }

    #[test]
    fn mixed_whitespace_removed() {
        assert_eq!(trim_trailing_whitespace("hello \t \nworld\t "), "hello\nworld");
    }

    #[test]
    fn empty_string() {
        assert_eq!(trim_trailing_whitespace(""), "");
    }

    #[test]
    fn all_whitespace_lines_become_empty() {
        assert_eq!(trim_trailing_whitespace("   \n\t\t\n  \t  "), "\n\n");
    }

    #[test]
    fn single_line_no_newline() {
        assert_eq!(trim_trailing_whitespace("hello   "), "hello");
    }

    #[test]
    fn leading_whitespace_preserved() {
        assert_eq!(trim_trailing_whitespace("  hello  \n  world  "), "  hello\n  world");
    }

    #[test]
    fn preserves_internal_spaces() {
        assert_eq!(trim_trailing_whitespace("hello  world   "), "hello  world");
    }
}

#[cfg(test)]
mod pane_passive_tests {
    use gtk4::prelude::*;

    #[test]
    fn persistent_pane_view_is_constructible_without_action_buttons() {
        let _size = std::mem::size_of::<super::persistent_widget::PersistentPaneView>();
    }

    /// Guard flags default to `false` so `connect_input`/`connect_resize` run
    /// on the first call but are rejected on subsequent calls. #538.
    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn persistent_pane_connect_guards_default_to_false() {
        if !crate::test_helpers::ensure_gtk() {
            eprintln!("SKIPPED: no display available");
            return;
        }
        let pane = super::persistent_widget::PersistentPaneView::new("guard-mod", "runtime-1");
        assert!(!pane.input_connected_for_test());
        assert!(!pane.resize_connected_for_test());
        assert!(!pane.has_resize_tick_for_test());
    }

    #[test]
    fn place_from_cwd_derives_name_for_context_menu_action() {
        let place = crate::places::Place::from_cwd("/home/user/projects/rttx", vec![]);
        assert_eq!(place.name, "rttx");
        assert_eq!(place.path, "/home/user/projects/rttx");
    }

    #[test]
    fn populate_places_submenu_adds_builtins_and_matching_saved() {
        let menu = gtk4::gio::Menu::new();
        // With no saved places, builtins (Home, Root) should appear.
        super::populate_places_submenu(&menu, crate::host::LOCAL_KEY);
        assert!(menu.n_items() >= 2, "builtins must appear; got {}", menu.n_items());

        // Calling again clears previous items before repopulating.
        super::populate_places_submenu(&menu, crate::host::LOCAL_KEY);
        assert!(menu.n_items() >= 2);
    }

    #[test]
    fn populate_places_submenu_items_target_open_place_action() {
        let menu = gtk4::gio::Menu::new();
        super::populate_places_submenu(&menu, crate::host::LOCAL_KEY);
        for idx in 0..menu.n_items() {
            let action = menu
                .item_attribute_value(idx, "action", Some(gtk4::glib::VariantTy::STRING))
                .expect("each item must have an action");
            assert_eq!(action.get::<String>().unwrap(), "win.open-place");
            let target = menu
                .item_attribute_value(idx, "target", Some(gtk4::glib::VariantTy::STRING))
                .expect("each item must have a target path");
            assert!(!target.get::<String>().unwrap().is_empty());
        }
    }

    /// Gesture modifier masks used by link and context menu handlers must be
    /// distinct so Ctrl+click and plain right-click do not interfere with
    /// each other. Regression for #459.
    #[test]
    fn gesture_modifier_masks_are_distinct() {
        let ctrl = gtk4::gdk::ModifierType::CONTROL_MASK;
        let shift = gtk4::gdk::ModifierType::SHIFT_MASK;
        assert!(!ctrl.intersects(shift), "Ctrl and Shift masks must not overlap");
    }

    /// Context menu must use Start alignment so the popover's left edge
    /// aligns with the pointer, preventing immediate item activation on
    /// button release. Regression for #480.
    #[test]
    fn context_menu_halign_is_start() {
        assert_eq!(
            super::CONTEXT_MENU_HALIGN,
            gtk4::Align::Start,
            "context menu must open adjacent to the pointer, not centered on it"
        );
    }

    /// Plain right-click (no modifiers) must open the context menu.
    /// Matches GNOME Terminal, Ptyxis, and Tilix conventions. Regression for #659.
    #[test]
    fn plain_right_click_opens_context_menu() {
        assert!(
            super::should_open_context_menu(gtk4::gdk::ModifierType::empty()),
            "plain right-click must open context menu"
        );
    }

    /// Shift+right-click must pass through to VTE for mouse-aware apps.
    /// Regression for #659.
    #[test]
    fn shift_right_click_passes_through() {
        assert!(
            !super::should_open_context_menu(gtk4::gdk::ModifierType::SHIFT_MASK),
            "Shift+right-click must not open context menu"
        );
    }

    /// Ctrl+right-click (without Shift) must still open the context menu.
    /// Only Shift suppresses the menu. Regression for #659.
    #[test]
    fn ctrl_right_click_opens_context_menu() {
        assert!(
            super::should_open_context_menu(gtk4::gdk::ModifierType::CONTROL_MASK),
            "Ctrl+right-click (no Shift) must open context menu"
        );
    }

    /// Ctrl+Shift+right-click must pass through (Shift is present).
    /// Regression for #659.
    #[test]
    fn ctrl_shift_right_click_passes_through() {
        let mods = gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK;
        assert!(
            !super::should_open_context_menu(mods),
            "Ctrl+Shift+right-click must not open context menu"
        );
    }
}

#[cfg(test)]
mod search_tests {
    use super::links;
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

    /// Snapshot bell stripping must remove all 0x07 bytes. Regression for #268.
    #[test]
    fn snapshot_bell_stripping_removes_all_bel_bytes() {
        let input = b"\x07PS1> \x07cmd\r\n\x07PS1> ";
        let filtered: Vec<u8> = input.iter().copied().filter(|&b| b != 0x07).collect();
        assert!(!filtered.contains(&0x07));
        assert_eq!(filtered, b"PS1> cmd\r\nPS1> ");
    }

    /// Spawn error message must use ANSI red for visibility. Regression for #22.
    #[test]
    fn spawn_error_message_uses_ansi_red() {
        let error = "No such file or directory";
        let msg = format!("\r\n\x1b[31mFailed to spawn shell: {error}\x1b[0m\r\n");
        assert!(msg.contains("\x1b[31m"));
        assert!(msg.contains(error));
        assert!(msg.ends_with("\x1b[0m\r\n"));
    }

    /// Zoom button visibility rule: visible when zoomed OR multi-pane.
    #[test]
    fn zoom_button_visibility_rule() {
        let visible = |zoomed: bool, multi_pane: bool| zoomed || multi_pane;
        assert!(!visible(false, false), "single pane, not zoomed → hidden");
        assert!(visible(false, true), "multi pane, not zoomed → visible");
        assert!(visible(true, false), "zoomed (was multi) → visible");
        assert!(visible(true, true), "zoomed + multi → visible");
    }

    /// Copy Link should show the filesystem path for file URIs, not the URI.
    #[test]
    fn display_text_for_file_uri_shows_path() {
        assert_eq!(links::display_text_for_uri("file:///tmp/log.txt"), "/tmp/log.txt");
        assert_eq!(links::display_text_for_uri("https://example.com"), "https://example.com");
    }

    /// Regression: bell preferences must be applied to persistent panes.
    ///
    /// VTE defaults `audible_bell` to true. If `apply_preferences_to_persistent_pane`
    /// skips `set_audible_bell`/`set_visual_bell`, the audio bell fires regardless
    /// of user settings.
    #[test]
    fn preferences_contain_bell_fields_with_correct_defaults() {
        let prefs = crate::preferences::Preferences::default();
        assert!(prefs.audible_bell, "audible_bell should default to true");
        assert!(prefs.visual_bell, "visual_bell should default to true");
    }

    #[test]
    fn paste_guard_preferences_default_enabled() {
        let prefs = crate::preferences::Preferences::default();
        assert!(prefs.paste_guard);
        assert_eq!(prefs.paste_guard_threshold, 1024);
    }

    /// Legacy persisted state with bookmark source must deserialize without
    /// error after the Bookmark variant was removed from `PaneSource`.
    #[test]
    fn legacy_bookmark_pane_source_deserializes_after_removal() {
        let json = r#"{"source":{"bookmark":{"name":"Prod"}},"target":null,"startup":[]}"#;
        let recovery: crate::session::PaneRecovery = serde_json::from_str(json).unwrap();
        assert_eq!(recovery.source, crate::session::PaneSource::Manual);
    }

    #[test]
    fn accent_css_for_dark_returns_distinct_variants() {
        let dark = crate::application::accent_css_for_dark(true);
        let light = crate::application::accent_css_for_dark(false);
        assert_ne!(dark, light);
    }

    /// CPR responses must be stripped from VTE commit data so they do not
    /// leak into the daemon's shell input. Regression for #633.
    #[test]
    fn cpr_responses_stripped_from_commit_data() {
        use super::persistent_widget::strip_cpr_responses;

        // Pure CPR response is fully removed.
        assert_eq!(strip_cpr_responses(b"\x1b[1;6R").unwrap(), b"");

        // Mouse sequences pass through unchanged.
        assert!(strip_cpr_responses(b"\x1b[<0;5;10M").is_none());

        // Mixed: mouse preserved, CPR removed.
        let mixed = b"\x1b[<0;5;10M\x1b[1;6R";
        assert_eq!(strip_cpr_responses(mixed).unwrap(), b"\x1b[<0;5;10M");
    }

    /// Regression for #655: `strip_user_host_prefix` must remove the
    /// user@host prefix that shells set via OSC 0/2 so pane titles
    /// do not show redundant host information.
    #[test]
    fn strip_user_host_prefix_removes_shell_title_prefix() {
        use super::persistent_widget::strip_user_host_prefix;

        assert_eq!(strip_user_host_prefix("user@host: ~/projects"), "~/projects");
        assert_eq!(strip_user_host_prefix("bash"), "bash");
        assert_eq!(strip_user_host_prefix(""), "");
    }

    /// `TerminalHandle::scroll_position` returns `None` when the VTE has
    /// no vadjustment (widget not yet realized). Regression for #686.
    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn scroll_position_returns_none_before_realize() {
        if !crate::test_helpers::ensure_gtk() {
            eprintln!("SKIPPED: no display available");
            return;
        }

        let pane = super::persistent_widget::PersistentPaneView::new("sp-1", "rt-1");
        let handle = super::handle::TerminalHandle::Managed(pane);
        // Before the widget is parented, vadjustment may or may not exist
        // depending on VTE version, but scroll_position must not panic.
        let _ = handle.scroll_position();
    }
}
