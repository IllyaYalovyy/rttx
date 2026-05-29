//! Integration tests for keyboard shortcut behavior with non-Latin layouts.
//!
//! Verifies that the shortcut normalization logic correctly maps Cyrillic
//! keyvals to their Latin equivalents for both direct and managed terminals.
//! Regression coverage for #983.

use rttx::terminal::{
    TerminalInputBackend, TerminalKeyAction, TerminalModes,
    terminal_key_action_for_test as terminal_key_action,
};

const DEFAULT_MODES: TerminalModes =
    TerminalModes { application_cursor_keys: false, application_keypad: false };

/// Smart copy triggers via Latin hint when Cyrillic keyval is active. #983.
#[test]
fn non_latin_ctrl_c_triggers_smart_copy_via_latin_hint() {
    assert_eq!(
        terminal_key_action(
            TerminalInputBackend::Direct,
            gtk4::gdk::Key::Cyrillic_es,
            gtk4::gdk::ModifierType::CONTROL_MASK,
            true,
            true,
            DEFAULT_MODES,
            Some(gtk4::gdk::Key::c),
        ),
        TerminalKeyAction::CopySelection,
    );
}

/// Smart paste triggers via Latin hint when Cyrillic keyval is active. #983.
#[test]
fn non_latin_ctrl_v_triggers_smart_paste_via_latin_hint() {
    assert_eq!(
        terminal_key_action(
            TerminalInputBackend::Direct,
            gtk4::gdk::Key::Cyrillic_em,
            gtk4::gdk::ModifierType::CONTROL_MASK,
            false,
            true,
            DEFAULT_MODES,
            Some(gtk4::gdk::Key::v),
        ),
        TerminalKeyAction::PasteClipboard,
    );
}

/// Window accelerator pass-through works via Latin hint in managed mode. #983.
#[test]
fn non_latin_ctrl_shift_t_passes_through_for_window_accelerator() {
    assert_eq!(
        terminal_key_action(
            TerminalInputBackend::Managed,
            gtk4::gdk::Key::Cyrillic_IE,
            gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK,
            false,
            false,
            DEFAULT_MODES,
            Some(gtk4::gdk::Key::T),
        ),
        TerminalKeyAction::PassThrough,
    );
}

/// Managed copy triggers via Latin hint with Cyrillic keyval. #983.
#[test]
fn non_latin_managed_ctrl_shift_c_copies_with_selection() {
    assert_eq!(
        terminal_key_action(
            TerminalInputBackend::Managed,
            gtk4::gdk::Key::Cyrillic_ES,
            gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK,
            true,
            false,
            DEFAULT_MODES,
            Some(gtk4::gdk::Key::C),
        ),
        TerminalKeyAction::CopySelection,
    );
}

/// Managed paste triggers via Latin hint with Cyrillic keyval. #983.
#[test]
fn non_latin_managed_ctrl_shift_v_pastes() {
    assert_eq!(
        terminal_key_action(
            TerminalInputBackend::Managed,
            gtk4::gdk::Key::Cyrillic_EM,
            gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK,
            false,
            false,
            DEFAULT_MODES,
            Some(gtk4::gdk::Key::V),
        ),
        TerminalKeyAction::PasteClipboard,
    );
}

/// All window accelerator shortcuts pass through with Latin hint. #983.
#[test]
fn all_ctrl_shift_window_shortcuts_pass_through_with_latin_hint() {
    let ctrl_shift = gtk4::gdk::ModifierType::CONTROL_MASK | gtk4::gdk::ModifierType::SHIFT_MASK;

    let mappings: &[(gtk4::gdk::Key, gtk4::gdk::Key)] = &[
        (gtk4::gdk::Key::Cyrillic_IE, gtk4::gdk::Key::T),
        (gtk4::gdk::Key::Cyrillic_TSE, gtk4::gdk::Key::W),
        (gtk4::gdk::Key::Cyrillic_U, gtk4::gdk::Key::E),
        (gtk4::gdk::Key::Cyrillic_SHCHA, gtk4::gdk::Key::O),
        (gtk4::gdk::Key::Cyrillic_A, gtk4::gdk::Key::F),
        (gtk4::gdk::Key::Cyrillic_I, gtk4::gdk::Key::B),
        (gtk4::gdk::Key::Cyrillic_TE, gtk4::gdk::Key::N),
    ];

    for (cyrillic, latin) in mappings {
        let action = terminal_key_action(
            TerminalInputBackend::Managed,
            *cyrillic,
            ctrl_shift,
            false,
            false,
            DEFAULT_MODES,
            Some(*latin),
        );
        assert_eq!(
            action,
            TerminalKeyAction::PassThrough,
            "Ctrl+Shift+{cyrillic:?} with latin_key={latin:?} must PassThrough"
        );
    }
}
