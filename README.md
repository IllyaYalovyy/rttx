# rttx

A tiling terminal emulator for GNOME, built with Rust, GTK4, and Libadwaita.

Spiritual successor to [Tilix](https://github.com/gnunn1/tilix), rewritten from scratch.

## Features

- **Split-screen terminals** — split horizontally (Ctrl+Shift+E) or vertically (Ctrl+Shift+O)
- **Sidebar session management** — multiple sessions with named tabs, not horizontal tab bar
- **Session persistence** — layout, CWDs, and custom titles saved on exit, restored on launch
- **Input synchronization** — type in one terminal, replicate to all others in the session
- **Drag and drop** — rearrange terminals by dragging headers
- **Custom titles** — double-click terminal header to set a custom name
- **Preferences** — font, color scheme, scrollback, opacity, and more
- **Tilix color scheme compatibility** — load existing Tilix JSON color scheme files
- **Process notifications** — desktop notification when a background process completes
- **Keyboard-driven** — comprehensive shortcuts for all operations

## Keyboard Shortcuts

| Action | Shortcut |
|---|---|
| New session | Ctrl+Shift+T |
| Close terminal | Ctrl+Shift+W |
| Split horizontal | Ctrl+Shift+E |
| Split vertical | Ctrl+Shift+O |
| Toggle sidebar | Ctrl+Shift+N |
| Search | Ctrl+Shift+F |
| Copy | Ctrl+Shift+C |
| Paste | Ctrl+Shift+V |
| Input sync toggle | Ctrl+Shift+I |
| Next/prev session | Ctrl+Tab / Ctrl+Shift+Tab |
| Zoom in/out/reset | Ctrl+Plus / Ctrl+Minus / Ctrl+0 |
| Preferences | Ctrl+, |
| Fullscreen | F11 |

## Building

### Dependencies

- Rust 1.75+
- GTK4 4.14+
- Libadwaita 1.5+
- VTE 0.76+ (GTK4 variant)

**Ubuntu/Debian:**
```bash
sudo apt install libgtk-4-dev libadwaita-1-dev libvte-2.91-gtk4-dev
```

**Fedora:**
```bash
sudo dnf install gtk4-devel libadwaita-devel vte291-gtk4-devel
```

**Arch Linux:**
```bash
sudo pacman -S gtk4 libadwaita vte4
```

### Build and run

```bash
cargo build --release
./target/release/rttx
```

### Install with Meson

```bash
meson setup build
meson install -C build
```

## Testing

Run the standard test suite:
```bash
cargo test
```

To run GTK widget tests in a headless environment (CI or no active X11/Wayland session), use the Broadway backend:
```bash
GDK_BACKEND=broadway GTK_A11Y=none cargo test
```

102 tests covering layout tree operations, session persistence, color scheme compatibility,
preferences, and property-based testing with proptest.

## License

GPL-3.0-or-later
