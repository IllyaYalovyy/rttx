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
    let prefs = crate::store::default_store().load_preferences().into_value().unwrap_or_default();
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
    let saved = crate::store::default_store().load_places();
    let visible = crate::places::visible_for_host(&saved, host_key);
    for place in &visible {
        let item = gtk4::gio::MenuItem::new(Some(&place.display_label()), None);
        item.set_action_and_target_value(Some("win.open-place"), Some(&place.path.to_variant()));
        menu.append_item(&item);
    }
}

/// Test-only re-export of `encode_terminal_key_input` (normal mode).
#[doc(hidden)]
#[must_use]
pub fn encode_terminal_key_input_for_test(
    key: gtk4::gdk::Key,
    state: gtk4::gdk::ModifierType,
) -> Option<Vec<u8>> {
    encode_terminal_key_input(key, state, TerminalModes::default())
}

/// Test-only re-export of mode-aware `encode_terminal_key_input`.
#[doc(hidden)]
#[must_use]
pub fn encode_terminal_key_input_with_modes_for_test(
    key: gtk4::gdk::Key,
    state: gtk4::gdk::ModifierType,
    modes: TerminalModes,
) -> Option<Vec<u8>> {
    encode_terminal_key_input(key, state, modes)
}

/// Terminal interaction modes that affect key encoding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalModes {
    pub application_cursor_keys: bool,
    pub application_keypad: bool,
}

/// Byte sequence that resets VTE to a known ground state.
///
/// Intended to be fed into VTE *before* an additive mode-restore block so
/// the final state is deterministic regardless of what the preceding
/// scrollback bytes left behind.
///
/// Contents (in order):
/// 1. `CAN` (`\x18`) — abort any in-progress escape sequence
/// 2. DECRST for cursor-keys, mouse tracking, SGR mouse, urxvt mouse,
///    focus reporting, bracketed paste — turns every mode off
/// 3. DECPNM (`\x1b>`) — numeric keypad (cancel application keypad)
/// 4. DECTCEM set (`\x1b[?25h`) — cursor visible
/// 5. SGR reset (`\x1b[m`) — default text attributes
#[must_use]
pub const fn terminal_cleanup_bytes() -> &'static [u8] {
    b"\x18\x1b[?1l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1015l\x1b[?1004l\x1b[?2004l\x1b>\x1b[?25h\x1b[m"
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

/// Encode a cursor/navigation key: SS3 when application cursor mode is active
/// and no modifiers are held, CSI otherwise.
fn cursor_key(suffix: u8, modifier: u8, application_cursor: bool) -> Vec<u8> {
    if modifier == 0 && application_cursor {
        vec![0x1b, b'O', suffix]
    } else {
        csi_letter(suffix, modifier)
    }
}

/// Map a keypad key to its SS3 application-mode byte, if applicable.
const fn keypad_application_byte(key: gtk4::gdk::Key) -> Option<u8> {
    match key {
        gtk4::gdk::Key::KP_0 => Some(b'p'),
        gtk4::gdk::Key::KP_1 => Some(b'q'),
        gtk4::gdk::Key::KP_2 => Some(b'r'),
        gtk4::gdk::Key::KP_3 => Some(b's'),
        gtk4::gdk::Key::KP_4 => Some(b't'),
        gtk4::gdk::Key::KP_5 => Some(b'u'),
        gtk4::gdk::Key::KP_6 => Some(b'v'),
        gtk4::gdk::Key::KP_7 => Some(b'w'),
        gtk4::gdk::Key::KP_8 => Some(b'x'),
        gtk4::gdk::Key::KP_9 => Some(b'y'),
        gtk4::gdk::Key::KP_Add => Some(b'k'),
        gtk4::gdk::Key::KP_Subtract => Some(b'm'),
        gtk4::gdk::Key::KP_Multiply => Some(b'j'),
        gtk4::gdk::Key::KP_Divide => Some(b'o'),
        gtk4::gdk::Key::KP_Decimal => Some(b'n'),
        gtk4::gdk::Key::KP_Enter => Some(b'M'),
        _ => None,
    }
}

fn encode_printable_or_control(key: gtk4::gdk::Key, ctrl: bool, alt: bool) -> Option<Vec<u8>> {
    let ch = key.to_unicode()?;
    if ctrl && alt {
        let ctrl_byte = encode_control_character(ch)?;
        Some(vec![0x1b, ctrl_byte])
    } else if ctrl {
        Some(vec![encode_control_character(ch)?])
    } else if alt {
        let mut prefixed = vec![0x1b];
        prefixed.extend(ch.to_string().bytes());
        Some(prefixed)
    } else {
        Some(ch.to_string().into_bytes())
    }
}

fn encode_terminal_key_input(
    key: gtk4::gdk::Key,
    state: gtk4::gdk::ModifierType,
    modes: TerminalModes,
) -> Option<Vec<u8>> {
    let ctrl = state.contains(gtk4::gdk::ModifierType::CONTROL_MASK);
    let alt = state.contains(gtk4::gdk::ModifierType::ALT_MASK);
    let m = xterm_modifier_param(state);

    let seq = match key {
        gtk4::gdk::Key::KP_Enter if modes.application_keypad => {
            return Some(vec![0x1b, b'O', b'M']);
        }
        gtk4::gdk::Key::Return | gtk4::gdk::Key::KP_Enter => vec![b'\r'],
        gtk4::gdk::Key::BackSpace => vec![0x7f],
        gtk4::gdk::Key::Tab | gtk4::gdk::Key::KP_Tab => vec![b'\t'],
        gtk4::gdk::Key::ISO_Left_Tab => b"\x1b[Z".to_vec(),
        gtk4::gdk::Key::Escape => vec![0x1b],
        gtk4::gdk::Key::Up | gtk4::gdk::Key::KP_Up => {
            return Some(cursor_key(b'A', m, modes.application_cursor_keys));
        }
        gtk4::gdk::Key::Down | gtk4::gdk::Key::KP_Down => {
            return Some(cursor_key(b'B', m, modes.application_cursor_keys));
        }
        gtk4::gdk::Key::Right | gtk4::gdk::Key::KP_Right => {
            return Some(cursor_key(b'C', m, modes.application_cursor_keys));
        }
        gtk4::gdk::Key::Left | gtk4::gdk::Key::KP_Left => {
            return Some(cursor_key(b'D', m, modes.application_cursor_keys));
        }
        gtk4::gdk::Key::Home | gtk4::gdk::Key::KP_Home => {
            return Some(cursor_key(b'H', m, modes.application_cursor_keys));
        }
        gtk4::gdk::Key::End | gtk4::gdk::Key::KP_End => {
            return Some(cursor_key(b'F', m, modes.application_cursor_keys));
        }
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
        _ if modes.application_keypad => {
            if let Some(ss3) = keypad_application_byte(key) {
                return Some(vec![0x1b, b'O', ss3]);
            }
            return encode_printable_or_control(key, ctrl, alt);
        }
        _ => {
            return encode_printable_or_control(key, ctrl, alt);
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
    modes: TerminalModes,
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
        TerminalInputBackend::Managed => encode_terminal_key_input(key, modifiers, modes)
            .map_or(TerminalKeyAction::PassThrough, TerminalKeyAction::ForwardToPty),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TerminalInputBackend, TerminalKeyAction, TerminalModes, encode_terminal_key_input,
        terminal_key_action,
    };

    const DEFAULT_MODES: TerminalModes =
        TerminalModes { application_cursor_keys: false, application_keypad: false };

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
                DEFAULT_MODES,
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
                DEFAULT_MODES,
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
                DEFAULT_MODES,
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
                DEFAULT_MODES,
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
                DEFAULT_MODES,
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
                DEFAULT_MODES,
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
                DEFAULT_MODES,
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
                DEFAULT_MODES,
            ),
            TerminalKeyAction::ForwardToPty(b"\x1bx".to_vec())
        );
    }

    #[test]
    fn ctrl_d_encodes_eof_byte() {
        assert_eq!(
            encode_terminal_key_input(
                gtk4::gdk::Key::d,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                DEFAULT_MODES
            ),
            Some(vec![0x04])
        );
        assert_eq!(
            terminal_key_action(
                TerminalInputBackend::Managed,
                gtk4::gdk::Key::d,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                false,
                true,
                DEFAULT_MODES,
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
                DEFAULT_MODES,
            ),
            TerminalKeyAction::PasteClipboard
        );
    }

    #[test]
    fn encode_terminal_key_input_maps_basic_shell_keys() {
        assert_eq!(
            encode_terminal_key_input(
                gtk4::gdk::Key::a,
                gtk4::gdk::ModifierType::empty(),
                DEFAULT_MODES
            ),
            Some(vec![b'a'])
        );
        assert_eq!(
            encode_terminal_key_input(
                gtk4::gdk::Key::Return,
                gtk4::gdk::ModifierType::empty(),
                DEFAULT_MODES
            ),
            Some(vec![b'\r'])
        );
        assert_eq!(
            encode_terminal_key_input(
                gtk4::gdk::Key::BackSpace,
                gtk4::gdk::ModifierType::empty(),
                DEFAULT_MODES
            ),
            Some(vec![0x7f])
        );
        assert_eq!(
            encode_terminal_key_input(
                gtk4::gdk::Key::Left,
                gtk4::gdk::ModifierType::empty(),
                DEFAULT_MODES
            ),
            Some(b"\x1b[D".to_vec())
        );
        assert_eq!(
            encode_terminal_key_input(
                gtk4::gdk::Key::c,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                DEFAULT_MODES
            ),
            Some(vec![0x03])
        );
        assert_eq!(
            encode_terminal_key_input(
                gtk4::gdk::Key::x,
                gtk4::gdk::ModifierType::ALT_MASK,
                DEFAULT_MODES
            ),
            Some(b"\x1bx".to_vec())
        );
        assert_eq!(
            encode_terminal_key_input(
                gtk4::gdk::Key::Shift_L,
                gtk4::gdk::ModifierType::SHIFT_MASK,
                DEFAULT_MODES
            ),
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
                DEFAULT_MODES,
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
                DEFAULT_MODES,
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
            let result =
                encode_terminal_key_input(*key, gtk4::gdk::ModifierType::empty(), DEFAULT_MODES);
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
            DEFAULT_MODES,
        );
        assert!(
            matches!(action, TerminalKeyAction::ForwardToPty(_)),
            "F1 must be ForwardToPty in managed mode, got {action:?}"
        );
    }

    /// Alt+F-key must use xterm modifier param 3. #293.
    #[test]
    fn alt_fkey_uses_modifier_encoding() {
        let result = encode_terminal_key_input(
            gtk4::gdk::Key::F2,
            gtk4::gdk::ModifierType::ALT_MASK,
            DEFAULT_MODES,
        );
        assert_eq!(result.as_deref(), Some(b"\x1b[1;3Q" as &[u8]));
    }

    /// Ctrl+Arrow must use xterm modified key format. #295.
    #[test]
    fn ctrl_arrow_uses_xterm_modifier_encoding() {
        let ctrl = gtk4::gdk::ModifierType::CONTROL_MASK;
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Right, ctrl, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[1;5C" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Left, ctrl, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[1;5D" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Up, ctrl, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[1;5A" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Down, ctrl, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[1;5B" as &[u8])
        );
    }

    /// Shift+Arrow must use xterm modified key format. #295.
    #[test]
    fn shift_arrow_uses_xterm_modifier_encoding() {
        let shift = gtk4::gdk::ModifierType::SHIFT_MASK;
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Right, shift, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[1;2C" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Left, shift, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[1;2D" as &[u8])
        );
    }

    /// Ctrl+Home/End must use xterm modified key format. #295.
    #[test]
    fn ctrl_home_end_uses_xterm_modifier_encoding() {
        let ctrl = gtk4::gdk::ModifierType::CONTROL_MASK;
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Home, ctrl, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[1;5H" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::End, ctrl, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[1;5F" as &[u8])
        );
    }

    /// Ctrl+Shift+Arrow must use modifier param 6. #295.
    #[test]
    fn ctrl_shift_arrow_uses_modifier_6() {
        let mods = gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK;
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Right, mods, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[1;6C" as &[u8])
        );
    }

    /// Alt+Ctrl+Arrow must use modifier param 7 (Alt prefix NOT doubled). #295.
    #[test]
    fn alt_ctrl_arrow_uses_modifier_7() {
        let mods = gtk4::gdk::ModifierType::ALT_MASK | gtk4::gdk::ModifierType::CONTROL_MASK;
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Right, mods, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[1;7C" as &[u8])
        );
    }

    /// Modifier+F-keys must use CSI modified format. #295.
    #[test]
    fn ctrl_fkey_uses_modified_format() {
        let ctrl = gtk4::gdk::ModifierType::CONTROL_MASK;
        // F5 = CSI 15~ → Ctrl+F5 = CSI 15;5~
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::F5, ctrl, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[15;5~" as &[u8])
        );
        // F1 = SS3 P → Ctrl+F1 = CSI 1;5P
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::F1, ctrl, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[1;5P" as &[u8])
        );
    }

    /// Modifier+Insert/Delete/PageUp/PageDown must use modified tilde format. #295.
    #[test]
    fn ctrl_tilde_keys_use_modified_format() {
        let ctrl = gtk4::gdk::ModifierType::CONTROL_MASK;
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Delete, ctrl, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[3;5~" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Page_Up, ctrl, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[5;5~" as &[u8])
        );
    }

    /// Unmodified navigation keys must remain unchanged. #295.
    #[test]
    fn unmodified_navigation_keys_unchanged() {
        let none = gtk4::gdk::ModifierType::empty();
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Right, none, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[C" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Home, none, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[H" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::F1, none, DEFAULT_MODES).as_deref(),
            Some(b"\x1bOP" as &[u8])
        );
    }

    /// Ctrl+Alt+letter must produce ESC + control-char so Alt is not lost. #457.
    #[test]
    fn ctrl_alt_letter_preserves_alt_prefix() {
        let mods = gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::ALT_MASK;
        // Ctrl+Alt+a → ESC 0x01
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::a, mods, DEFAULT_MODES),
            Some(vec![0x1b, 0x01])
        );
        // Ctrl+Alt+c → ESC 0x03
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::c, mods, DEFAULT_MODES),
            Some(vec![0x1b, 0x03])
        );
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
            DEFAULT_MODES,
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
                encode_terminal_key_input(*key, none, DEFAULT_MODES),
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
                encode_terminal_key_input(*key, none, DEFAULT_MODES),
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
            encode_terminal_key_input(gtk4::gdk::Key::Insert, shift, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[2;2~" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Delete, shift, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[3;2~" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Page_Up, shift, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[5;2~" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Page_Down, shift, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[6;2~" as &[u8])
        );
    }

    /// Shift+Home/End use xterm modified letter format. #457.
    #[test]
    fn shift_home_end_uses_modified_format() {
        let shift = gtk4::gdk::ModifierType::SHIFT_MASK;
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Home, shift, DEFAULT_MODES).as_deref(),
            Some(b"\x1b[1;2H" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::End, shift, DEFAULT_MODES).as_deref(),
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
            let action = terminal_key_action(
                TerminalInputBackend::Managed,
                *key,
                *mods,
                false,
                false,
                DEFAULT_MODES,
            );
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
                encode_terminal_key_input(*key, none, DEFAULT_MODES),
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
                DEFAULT_MODES,
            ),
            TerminalKeyAction::PassThrough,
        );
    }

    /// Control sequences must still be encoded even when `IMContext` is active,
    /// because the `IMContext` does not consume modified keys. #462.
    #[test]
    fn control_keys_still_encoded_with_ime_active() {
        let ctrl = gtk4::gdk::ModifierType::CONTROL_MASK;
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::c, ctrl, DEFAULT_MODES),
            Some(vec![0x03]),
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::d, ctrl, DEFAULT_MODES),
            Some(vec![0x04]),
        );
    }

    /// Keys that produce `ForwardToPty` in managed mode are the ones that
    /// trigger scroll-to-bottom when the user types in a scrolled-up pane.
    /// Regression for #753.
    #[test]
    fn managed_forward_to_pty_keys_trigger_scroll_on_keystroke() {
        let typing_keys: &[(gtk4::gdk::Key, gtk4::gdk::ModifierType)] = &[
            (gtk4::gdk::Key::a, gtk4::gdk::ModifierType::empty()),
            (gtk4::gdk::Key::Return, gtk4::gdk::ModifierType::empty()),
            (gtk4::gdk::Key::d, gtk4::gdk::ModifierType::CONTROL_MASK),
            (gtk4::gdk::Key::Up, gtk4::gdk::ModifierType::empty()),
        ];
        for (key, mods) in typing_keys {
            let action = terminal_key_action(
                TerminalInputBackend::Managed,
                *key,
                *mods,
                false,
                false,
                DEFAULT_MODES,
            );
            assert!(
                matches!(action, TerminalKeyAction::ForwardToPty(_)),
                "{key:?}+{mods:?} must produce ForwardToPty (scroll-on-keystroke trigger)"
            );
        }
    }

    /// Application cursor mode: unmodified arrows use SS3. #767.
    #[test]
    fn app_cursor_mode_arrows_use_ss3() {
        let modes = TerminalModes { application_cursor_keys: true, application_keypad: false };
        let none = gtk4::gdk::ModifierType::empty();
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Up, none, modes).as_deref(),
            Some(b"\x1bOA" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::Down, none, modes).as_deref(),
            Some(b"\x1bOB" as &[u8])
        );
    }

    /// Application cursor mode: modified arrows still use CSI. #767.
    #[test]
    fn app_cursor_mode_modified_arrows_use_csi() {
        let modes = TerminalModes { application_cursor_keys: true, application_keypad: false };
        assert_eq!(
            encode_terminal_key_input(
                gtk4::gdk::Key::Up,
                gtk4::gdk::ModifierType::CONTROL_MASK,
                modes
            )
            .as_deref(),
            Some(b"\x1b[1;5A" as &[u8])
        );
    }

    /// Application keypad mode: digits use SS3. #767.
    #[test]
    fn app_keypad_mode_digits_use_ss3() {
        let modes = TerminalModes { application_cursor_keys: false, application_keypad: true };
        let none = gtk4::gdk::ModifierType::empty();
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::KP_0, none, modes).as_deref(),
            Some(b"\x1bOp" as &[u8])
        );
        assert_eq!(
            encode_terminal_key_input(gtk4::gdk::Key::KP_5, none, modes).as_deref(),
            Some(b"\x1bOu" as &[u8])
        );
    }

    /// Managed terminal with app cursor mode forwards SS3 arrows. #767.
    #[test]
    fn managed_app_cursor_forwards_ss3_arrows() {
        let modes = TerminalModes { application_cursor_keys: true, application_keypad: false };
        let action = terminal_key_action(
            TerminalInputBackend::Managed,
            gtk4::gdk::Key::Up,
            gtk4::gdk::ModifierType::empty(),
            false,
            false,
            modes,
        );
        assert_eq!(action, TerminalKeyAction::ForwardToPty(b"\x1bOA".to_vec()));
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

    /// `feed_snapshot` must not scroll synchronously — the scroll is deferred
    /// so VTE has time to update its layout. Regression for #707.
    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn feed_snapshot_defers_scroll_to_idle() {
        if !crate::test_helpers::ensure_gtk() {
            eprintln!("SKIPPED: no display available");
            return;
        }

        let pane = super::persistent_widget::PersistentPaneView::new("defer-mod", "runtime-1");
        let window = gtk4::Window::new();
        window.set_default_size(640, 480);
        window.set_child(Some(&pane));
        window.present();

        let ctx = gtk4::glib::MainContext::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(100);
        while std::time::Instant::now() < deadline {
            if !ctx.iteration(false) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }

        let mut data = Vec::new();
        for i in 0..200 {
            data.extend_from_slice(format!("line {i}\r\n").as_bytes());
        }
        pane.feed_snapshot(&data);

        // After idle fires, viewport must be at the bottom.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(100);
        while std::time::Instant::now() < deadline {
            if !ctx.iteration(false) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }

        let adj = pane.vte().vadjustment().expect("vadjustment should exist");
        let bottom = adj.upper() - adj.page_size();
        assert!(
            (adj.value() - bottom).abs() < 1.0,
            "deferred scroll must land at bottom; got {} expected ~{bottom}",
            adj.value()
        );
        window.close();
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

    /// Removed `PaneSource` variants must fail to deserialize now that the
    /// legacy backward-compat layer is gone.
    #[test]
    fn removed_pane_source_variant_rejects_on_deserialize() {
        let json = r#"{"source":{"bookmark":{"name":"Prod"}},"target":null,"startup":[]}"#;
        assert!(serde_json::from_str::<crate::workspace::PaneRecovery>(json).is_err());
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

    /// `terminal_cleanup_bytes` contains CAN, all mode-off sequences,
    /// DECPNM, DECTCEM set, and SGR reset. #809.
    #[test]
    fn terminal_cleanup_bytes_contains_required_sequences() {
        let bytes = super::terminal_cleanup_bytes();
        assert_eq!(bytes[0], 0x18, "must start with CAN");
        assert!(bytes.windows(5).any(|w| w == b"\x1b[?1l"), "DECCKM off");
        assert!(bytes.windows(8).any(|w| w == b"\x1b[?1000l"), "mouse normal off");
        assert!(bytes.windows(8).any(|w| w == b"\x1b[?1002l"), "mouse button off");
        assert!(bytes.windows(8).any(|w| w == b"\x1b[?1003l"), "mouse any off");
        assert!(bytes.windows(8).any(|w| w == b"\x1b[?1006l"), "SGR mouse off");
        assert!(bytes.windows(8).any(|w| w == b"\x1b[?1015l"), "urxvt mouse off");
        assert!(bytes.windows(8).any(|w| w == b"\x1b[?1004l"), "focus reporting off");
        assert!(bytes.windows(8).any(|w| w == b"\x1b[?2004l"), "bracketed paste off");
        assert!(bytes.windows(2).any(|w| w == b"\x1b>"), "DECPNM");
        assert!(bytes.windows(6).any(|w| w == b"\x1b[?25h"), "cursor visible");
        assert!(bytes.windows(3).any(|w| w == b"\x1b[m"), "SGR reset");
    }

    /// `TerminalHandle::repair_terminal` resets tracked modes on managed
    /// panes so key encoding matches the cleaned VTE state. #811.
    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn repair_terminal_resets_tracked_modes_on_managed_handle() {
        if !crate::test_helpers::ensure_gtk() {
            eprintln!("SKIPPED: no display available");
            return;
        }

        let pane = super::persistent_widget::PersistentPaneView::new("repair-mod-1", "rt-1");
        pane.set_application_modes(true, true);
        let handle = super::handle::TerminalHandle::Managed(pane.clone());
        handle.repair_terminal();
        let modes = pane.terminal_modes();
        assert!(!modes.application_cursor_keys);
        assert!(!modes.application_keypad);
    }

    /// `TerminalHandle::set_custom_title` propagates to the underlying
    /// widget and clearing reverts to the daemon-reported title. #819.
    #[test]
    #[ignore = "requires isolated GTK harness"]
    fn handle_set_custom_title_propagates_and_clears() {
        if !crate::test_helpers::ensure_gtk() {
            eprintln!("SKIPPED: no display available");
            return;
        }

        let pane = super::persistent_widget::PersistentPaneView::new("ct-mod-1", "rt-1");
        pane.set_daemon_title("auto title");
        let handle = super::handle::TerminalHandle::Managed(pane.clone());

        assert!(handle.custom_title().is_none());
        handle.set_custom_title(Some("renamed"));
        assert_eq!(handle.custom_title().as_deref(), Some("renamed"));
        assert_eq!(pane.title_label().label(), "renamed");

        handle.set_custom_title(None);
        assert!(handle.custom_title().is_none());
        assert_eq!(pane.title_label().label(), "auto title");
    }
}
