//! VTE parity harness for managed terminal input encoding.
//!
//! Verifies that `encode_terminal_key_input` produces the same escape
//! sequences a standard xterm/VTE terminal would emit for every key
//! category: printable characters, control keys, Alt/Ctrl/Shift
//! combinations, navigation keys, F-keys, keypad, and special keys.
//!
//! This is the required regression layer for future terminal input fixes
//! (see GitHub issue #464).

use rttx::terminal::encode_terminal_key_input_for_test as encode;

type Mods = gtk4::gdk::ModifierType;
type Key = gtk4::gdk::Key;

const NONE: Mods = Mods::empty();
const CTRL: Mods = Mods::CONTROL_MASK;
const ALT: Mods = Mods::ALT_MASK;
const SHIFT: Mods = Mods::SHIFT_MASK;

/// Table-driven parity assertion: each entry is (key, modifiers, expected bytes).
fn assert_parity(cases: &[(Key, Mods, &[u8])]) {
    for (key, mods, expected) in cases {
        let actual = encode(*key, *mods);
        assert_eq!(
            actual.as_deref(),
            Some(*expected),
            "parity mismatch for {key:?} + {mods:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

// ── Printable characters ────────────────────────────────────────

#[test]
fn printable_ascii_lowercase() {
    assert_parity(&[
        (Key::a, NONE, b"a"),
        (Key::b, NONE, b"b"),
        (Key::c, NONE, b"c"),
        (Key::d, NONE, b"d"),
        (Key::e, NONE, b"e"),
        (Key::f, NONE, b"f"),
        (Key::g, NONE, b"g"),
        (Key::h, NONE, b"h"),
        (Key::i, NONE, b"i"),
        (Key::j, NONE, b"j"),
        (Key::k, NONE, b"k"),
        (Key::l, NONE, b"l"),
        (Key::m, NONE, b"m"),
        (Key::n, NONE, b"n"),
        (Key::o, NONE, b"o"),
        (Key::p, NONE, b"p"),
        (Key::q, NONE, b"q"),
        (Key::r, NONE, b"r"),
        (Key::s, NONE, b"s"),
        (Key::t, NONE, b"t"),
        (Key::u, NONE, b"u"),
        (Key::v, NONE, b"v"),
        (Key::w, NONE, b"w"),
        (Key::x, NONE, b"x"),
        (Key::y, NONE, b"y"),
        (Key::z, NONE, b"z"),
    ]);
}

#[test]
fn printable_digits() {
    assert_parity(&[
        (Key::_0, NONE, b"0"),
        (Key::_1, NONE, b"1"),
        (Key::_2, NONE, b"2"),
        (Key::_3, NONE, b"3"),
        (Key::_4, NONE, b"4"),
        (Key::_5, NONE, b"5"),
        (Key::_6, NONE, b"6"),
        (Key::_7, NONE, b"7"),
        (Key::_8, NONE, b"8"),
        (Key::_9, NONE, b"9"),
    ]);
}

#[test]
fn space_produces_space_byte() {
    assert_eq!(encode(Key::space, NONE), Some(vec![b' ']));
}

// ── Control key combinations ────────────────────────────────────

#[test]
fn ctrl_a_through_z() {
    let keys_and_expected: &[(Key, u8)] = &[
        (Key::a, 0x01),
        (Key::b, 0x02),
        (Key::c, 0x03),
        (Key::d, 0x04),
        (Key::e, 0x05),
        (Key::f, 0x06),
        (Key::g, 0x07),
        (Key::h, 0x08),
        (Key::i, 0x09),
        (Key::j, 0x0a),
        (Key::k, 0x0b),
        (Key::l, 0x0c),
        (Key::m, 0x0d),
        (Key::n, 0x0e),
        (Key::o, 0x0f),
        (Key::p, 0x10),
        (Key::q, 0x11),
        (Key::r, 0x12),
        (Key::s, 0x13),
        (Key::t, 0x14),
        (Key::u, 0x15),
        (Key::v, 0x16),
        (Key::w, 0x17),
        (Key::x, 0x18),
        (Key::y, 0x19),
        (Key::z, 0x1a),
    ];
    for (key, expected) in keys_and_expected {
        let actual = encode(*key, CTRL);
        assert_eq!(actual, Some(vec![*expected]), "Ctrl+{key:?} should produce 0x{expected:02x}");
    }
}

#[test]
fn ctrl_special_characters() {
    assert_parity(&[
        (Key::space, CTRL, &[0x00]),        // Ctrl+Space = NUL
        (Key::bracketleft, CTRL, &[0x1b]),  // Ctrl+[ = ESC
        (Key::bracketright, CTRL, &[0x1d]), // Ctrl+] = GS
        (Key::backslash, CTRL, &[0x1c]),    // Ctrl+\ = FS
        (Key::slash, CTRL, &[0x1f]),        // Ctrl+/ = US
        (Key::question, CTRL, &[0x7f]),     // Ctrl+? = DEL
    ]);
}

// ── Alt combinations ────────────────────────────────────────────

#[test]
fn alt_printable_sends_esc_prefix() {
    assert_parity(&[
        (Key::a, ALT, b"\x1ba"),
        (Key::b, ALT, b"\x1bb"),
        (Key::x, ALT, b"\x1bx"),
        (Key::z, ALT, b"\x1bz"),
    ]);
}

// ── Navigation keys (unmodified) ────────────────────────────────

#[test]
fn arrow_keys_unmodified() {
    assert_parity(&[
        (Key::Up, NONE, b"\x1b[A"),
        (Key::Down, NONE, b"\x1b[B"),
        (Key::Right, NONE, b"\x1b[C"),
        (Key::Left, NONE, b"\x1b[D"),
    ]);
}

#[test]
fn home_end_unmodified() {
    assert_parity(&[(Key::Home, NONE, b"\x1b[H"), (Key::End, NONE, b"\x1b[F")]);
}

#[test]
fn insert_delete_page_keys_unmodified() {
    assert_parity(&[
        (Key::Insert, NONE, b"\x1b[2~"),
        (Key::Delete, NONE, b"\x1b[3~"),
        (Key::Page_Up, NONE, b"\x1b[5~"),
        (Key::Page_Down, NONE, b"\x1b[6~"),
    ]);
}

// ── Navigation keys (modified) ──────────────────────────────────

#[test]
fn ctrl_arrow_keys() {
    assert_parity(&[
        (Key::Up, CTRL, b"\x1b[1;5A"),
        (Key::Down, CTRL, b"\x1b[1;5B"),
        (Key::Right, CTRL, b"\x1b[1;5C"),
        (Key::Left, CTRL, b"\x1b[1;5D"),
    ]);
}

#[test]
fn shift_arrow_keys() {
    assert_parity(&[
        (Key::Up, SHIFT, b"\x1b[1;2A"),
        (Key::Down, SHIFT, b"\x1b[1;2B"),
        (Key::Right, SHIFT, b"\x1b[1;2C"),
        (Key::Left, SHIFT, b"\x1b[1;2D"),
    ]);
}

#[test]
fn alt_arrow_keys() {
    assert_parity(&[
        (Key::Up, ALT, b"\x1b[1;3A"),
        (Key::Down, ALT, b"\x1b[1;3B"),
        (Key::Right, ALT, b"\x1b[1;3C"),
        (Key::Left, ALT, b"\x1b[1;3D"),
    ]);
}

#[test]
fn ctrl_shift_arrow_keys() {
    let mods = CTRL.union(SHIFT);
    assert_parity(&[
        (Key::Up, mods, b"\x1b[1;6A"),
        (Key::Down, mods, b"\x1b[1;6B"),
        (Key::Right, mods, b"\x1b[1;6C"),
        (Key::Left, mods, b"\x1b[1;6D"),
    ]);
}

#[test]
fn alt_ctrl_arrow_keys() {
    let mods = ALT.union(CTRL);
    assert_parity(&[
        (Key::Up, mods, b"\x1b[1;7A"),
        (Key::Down, mods, b"\x1b[1;7B"),
        (Key::Right, mods, b"\x1b[1;7C"),
        (Key::Left, mods, b"\x1b[1;7D"),
    ]);
}

#[test]
fn ctrl_home_end() {
    assert_parity(&[(Key::Home, CTRL, b"\x1b[1;5H"), (Key::End, CTRL, b"\x1b[1;5F")]);
}

#[test]
fn ctrl_delete_insert_page() {
    assert_parity(&[
        (Key::Delete, CTRL, b"\x1b[3;5~"),
        (Key::Insert, CTRL, b"\x1b[2;5~"),
        (Key::Page_Up, CTRL, b"\x1b[5;5~"),
        (Key::Page_Down, CTRL, b"\x1b[6;5~"),
    ]);
}

// ── F-keys (unmodified) ─────────────────────────────────────────

#[test]
fn fkeys_unmodified() {
    assert_parity(&[
        (Key::F1, NONE, b"\x1bOP"),
        (Key::F2, NONE, b"\x1bOQ"),
        (Key::F3, NONE, b"\x1bOR"),
        (Key::F4, NONE, b"\x1bOS"),
        (Key::F5, NONE, b"\x1b[15~"),
        (Key::F6, NONE, b"\x1b[17~"),
        (Key::F7, NONE, b"\x1b[18~"),
        (Key::F8, NONE, b"\x1b[19~"),
        (Key::F9, NONE, b"\x1b[20~"),
        (Key::F10, NONE, b"\x1b[21~"),
        (Key::F11, NONE, b"\x1b[23~"),
        (Key::F12, NONE, b"\x1b[24~"),
    ]);
}

// ── F-keys (modified) ───────────────────────────────────────────

#[test]
fn ctrl_fkeys() {
    assert_parity(&[
        (Key::F1, CTRL, b"\x1b[1;5P"),
        (Key::F2, CTRL, b"\x1b[1;5Q"),
        (Key::F3, CTRL, b"\x1b[1;5R"),
        (Key::F4, CTRL, b"\x1b[1;5S"),
        (Key::F5, CTRL, b"\x1b[15;5~"),
        (Key::F6, CTRL, b"\x1b[17;5~"),
        (Key::F12, CTRL, b"\x1b[24;5~"),
    ]);
}

#[test]
fn shift_fkeys() {
    assert_parity(&[(Key::F1, SHIFT, b"\x1b[1;2P"), (Key::F5, SHIFT, b"\x1b[15;2~")]);
}

#[test]
fn alt_fkeys() {
    assert_parity(&[
        (Key::F1, ALT, b"\x1b[1;3P"),
        (Key::F2, ALT, b"\x1b[1;3Q"),
        (Key::F5, ALT, b"\x1b[15;3~"),
    ]);
}

// ── Keypad keys ─────────────────────────────────────────────────

#[test]
fn keypad_navigation_keys() {
    assert_parity(&[
        (Key::KP_Up, NONE, b"\x1b[A"),
        (Key::KP_Down, NONE, b"\x1b[B"),
        (Key::KP_Right, NONE, b"\x1b[C"),
        (Key::KP_Left, NONE, b"\x1b[D"),
        (Key::KP_Home, NONE, b"\x1b[H"),
        (Key::KP_End, NONE, b"\x1b[F"),
        (Key::KP_Insert, NONE, b"\x1b[2~"),
        (Key::KP_Delete, NONE, b"\x1b[3~"),
        (Key::KP_Page_Up, NONE, b"\x1b[5~"),
        (Key::KP_Page_Down, NONE, b"\x1b[6~"),
    ]);
}

#[test]
fn keypad_enter_and_tab() {
    assert_parity(&[(Key::KP_Enter, NONE, b"\r"), (Key::KP_Tab, NONE, b"\t")]);
}

// ── Special keys ────────────────────────────────────────────────

#[test]
fn return_backspace_tab_escape() {
    assert_parity(&[
        (Key::Return, NONE, b"\r"),
        (Key::BackSpace, NONE, &[0x7f]),
        (Key::Tab, NONE, b"\t"),
        (Key::Escape, NONE, &[0x1b]),
    ]);
}

#[test]
fn shift_tab_produces_backtab() {
    assert_eq!(encode(Key::ISO_Left_Tab, NONE), Some(b"\x1b[Z".to_vec()));
}

// ── Modifier-only keys produce None ─────────────────────────────

#[test]
fn modifier_only_keys_produce_none() {
    let modifier_keys = [
        Key::Shift_L,
        Key::Shift_R,
        Key::Control_L,
        Key::Control_R,
        Key::Alt_L,
        Key::Alt_R,
        Key::Super_L,
        Key::Super_R,
        Key::Caps_Lock,
        Key::Num_Lock,
    ];
    for key in modifier_keys {
        assert_eq!(encode(key, NONE), None, "modifier-only {key:?} must produce None");
    }
}

// ── Xterm modifier parameter encoding ───────────────────────────

#[test]
fn modifier_parameter_values_follow_xterm_convention() {
    // xterm modifier = 1 + (shift?1:0) + (alt?2:0) + (ctrl?4:0)
    // Param 2 = Shift, 3 = Alt, 4 = Shift+Alt, 5 = Ctrl,
    // 6 = Ctrl+Shift, 7 = Ctrl+Alt, 8 = Ctrl+Shift+Alt
    let cases: &[(Mods, &[u8])] = &[
        (SHIFT, b"\x1b[1;2A"),
        (ALT, b"\x1b[1;3A"),
        (SHIFT.union(ALT), b"\x1b[1;4A"),
        (CTRL, b"\x1b[1;5A"),
        (CTRL.union(SHIFT), b"\x1b[1;6A"),
        (CTRL.union(ALT), b"\x1b[1;7A"),
        (CTRL.union(SHIFT).union(ALT), b"\x1b[1;8A"),
    ];
    for (mods, expected) in cases {
        let actual = encode(Key::Up, *mods);
        assert_eq!(actual.as_deref(), Some(*expected), "Up + {mods:?} modifier param");
    }
}
