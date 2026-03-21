# rttx

A tiling terminal emulator for GNOME, built with Rust, GTK4, and Libadwaita.

Spiritual successor to [Tilix](https://github.com/gnunn1/tilix), rewritten from scratch.

## Motivation

There is no shortage of terminal emulators. New ones appear constantly, usually competing on rendering tricks, portability, customization systems, or novelty. `rttx` exists for a different reason.

This project is not trying to be the most feature-packed terminal, the most portable terminal, or the most technically flashy terminal. It is not interested in GPU marketing, gimmicks, or turning the terminal into a media player. It is focused on something much more practical: being an excellent terminal emulator for GNOME on Linux.

The goals are simple:

- tight desktop integration, so the application feels native instead of visually and behaviorally out of place
- practical features that matter in real work: tiling splits, sessions, persistence, search, synchronization, and workflow-oriented ergonomics
- simple configuration, without forcing users to learn a new language or adopt a configuration ecosystem just to change basic behavior
- rock-solid stability for long-running work, with a strong bias toward correctness over cleverness

That last point is central. A terminal is not a toy window. It often stays open for days, weeks, or months and is frequently used during critical work. It should not randomly crash, corrupt state, or slowly leak memory until it becomes unusable. `rttx` is written in Rust for a reason, and the project puts unusual emphasis on test coverage, especially around the Rust/GTK boundary where many subtle bugs tend to hide.

The scope is intentionally narrow: GNOME, Linux, practical features, and reliability first.

## Features

- **Split-screen terminals** — split horizontally (Ctrl+Shift+E) or vertically (Ctrl+Shift+O)
- **Sidebar session management** — multiple sessions with named tabs, not horizontal tab bar
- **Session persistence** — layout, CWDs, and custom titles saved on exit, restored on launch
- **Input synchronization** — type in one terminal, replicate to all others in the session
- **Terminal swapping by drag and drop** — drag one terminal header onto another to swap them
- **Custom titles** — double-click terminal header to set a custom name
- **Preferences** — font, terminal theme mode, light/dark terminal palettes, scrollback, header visibility, bell, and scroll behavior
- **Built-in terminal themes** — bundled light and dark palettes with optional system light/dark following
- **Tilix color scheme compatibility** — load Tilix JSON color scheme files
- **Process exit notifications** — desktop notification when an unfocused terminal process exits
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

To run GTK widget tests in a headless environment, start Broadway and point GTK at it:
```bash
broadwayd :5
GDK_BACKEND=broadway BROADWAY_DISPLAY=:5 GTK_A11Y=none cargo test
```

The repository includes unit, integration, GTK widget, and property-based tests covering layout
operations, session persistence, preferences, and Rust/GTK boundary behavior.

## Author

Illya Yalovyy

- LinkedIn: https://www.linkedin.com/in/illyayalovyy/
- Medium: https://medium.com/@yalovoy
- GitHub: https://github.com/IllyaYalovyy

## License

GPL-3.0-or-later
