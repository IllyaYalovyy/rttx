# rttx

A tiling terminal emulator for GNOME, built with Rust, GTK4, and Libadwaita.

Spiritual successor to [Tilix](https://github.com/gnunn1/tilix), rewritten from scratch with a focus on extreme stability, native integration, and practical workflows.

## Philosophy

`rttx` is built for developers and sysadmins who want a terminal that is:

- **Rock-Solid**: No memory leaks, no crashes, no "magic." Every core feature is backed by a comprehensive suite of unit, integration, and property-based tests.
- **Deeply Integrated**: Designed specifically for GNOME. It uses Libadwaita native window chrome and widgets, follows system light/dark settings, and integrates with system notifications.
- **Context-Aware**: A "session" should be more than a layout. `rttx` aims to preserve your actual working context, including paths and history.
- **Strictly Maintained**: The project enforces aggressive linting (Clippy pedantic/nursery) and automated formatting on every build to ensure long-term maintainability.

## Features

- **Split-screen terminals** — High-performance tiling splits up to 5 levels deep (Horizontal: Ctrl+Shift+E, Vertical: Ctrl+Shift+O).
- **Sidebar session management** — Persistent sessions organized in native Adwaita session rows.
- **Dual-side workflow layout** — Sessions stay in the left sidebar while bookmarks live in a dedicated right tools sidebar.
- **Custom session names** — Double-click a session in the sidebar to rename it.
- **Native GNOME window layout** — Libadwaita `ToolbarView` and `OverlaySplitView` keep the main window consistent with modern GNOME apps.
- **GNOME-native app metadata** — The app menu includes a native Libadwaita About window alongside preferences and shortcuts.
- **Context Persistence** — Layout, pane sizes, active pane, Current Working Directories, custom titles, and per-pane recovery metadata are saved automatically and restored on launch.
- **Input synchronization** — Broadcast keystrokes to all terminals in a session simultaneously (Ctrl+Shift+I).
- **Terminal swapping** — Drag and drop terminal headers to reorder your layout.
- **Custom titles** — Double-click any terminal header to rename it for your current task.
- **Bookmarks** — Save folder, SSH, tmux, or combined bookmarks; add, edit, and delete them directly from the sidebar; search, run in the current pane, or open as a new session in one click.
- **Commands launcher** — Save searchable single-line or multiline commands in the right tools sidebar and either run them immediately or insert them into the active terminal.
- **Structured session recovery** — Bookmark-driven panes restore as explicit targets such as local folders, local tmux, remote SSH shells, or remote SSH+tmux sessions.
- **Attach-only tmux recovery** — tmux-backed panes reattach to the named session; `rttx` never silently creates a new clean tmux session during recovery.
- **In-pane retry for failed recovery** — if a recoverable SSH/tmux pane fails to reconnect, the pane stays open and offers a non-modal retry action inside the terminal.
- **Optional smart clipboard** — An opt-in mode lets plain `Ctrl+C` copy selected terminal text and `Ctrl+V` paste from the clipboard.
- **Clickable links and paths** — `http(s)` URLs, OSC 8 hyperlinks, and detected file paths in terminal output highlight and open with a click.
- **Built-in terminal themes** — Native "Nightfall" and "Daybreak" schemes with full Tilix color scheme compatibility.
- **Smart Notifications** — Process exit in a hidden session shows an in-app toast while `rttx` is focused, and falls back to a desktop notification when the window is not active.

## Distribution

### Fedora (COPR)
The easiest way to install on Fedora is via the official COPR repository:
```bash
sudo dnf copr enable illya/rttx
sudo dnf install rttx
```

### Ubuntu/Debian (Coming soon)
Native `.deb` packages and a PPA are under development.

### Flatpak
Native Flatpak support via Flathub is under development.

## Keyboard Shortcuts

| Action | Shortcut |
|---|---|
| New session | Ctrl+Shift+T |
| Toggle tools sidebar | Ctrl+Shift+B |
| Close terminal | Ctrl+Shift+W |
| Split horizontal | Ctrl+Shift+E |
| Split vertical | Ctrl+Shift+O |
| Toggle sidebar | Ctrl+Shift+N |
| Search | Ctrl+Shift+F |
| Copy | Ctrl+Shift+C |
| Paste | Ctrl+Shift+V |
| Input sync toggle | Ctrl+Shift+I |
| Next/prev session | Ctrl+Tab / Ctrl+Shift+Tab |
| Session 1-9 | Alt+1 through Alt+9 |
| Zoom in/out/reset | Ctrl+Plus / Ctrl+Minus / Ctrl+0 |
| Preferences | Ctrl+, |
| Fullscreen | F11 |

## Development & Building

### Dependencies

- Rust 1.75+
- GTK4 4.14+
- Libadwaita 1.5+
- VTE 0.76+ (GTK4 variant)

### Build and run

The build process automatically runs `rustfmt` and `clippy` to ensure code quality.
```bash
cargo build --release
./target/release/rttx
```

To run a local development build alongside the installed app without sharing state:
```bash
RTTX_DEV_MODE=1 cargo run
```

Development mode uses a separate app ID, a separate config root, a distinct icon, and visible
`Devel` labeling in the window chrome.

### Install with Meson

For full system integration (icons, desktop files, etc.):
```bash
meson setup build --prefix="$HOME/.local"
meson install -C build
gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor"
update-desktop-database "$HOME/.local/share/applications"
```

For a system-wide install, use a system prefix and run the install step with elevated privileges:
```bash
meson setup build --prefix=/usr/local
sudo meson install -C build
```

If GNOME Shell still shows a generic icon after a user-local install, log out and back in so the shell refreshes its app grid cache.

## Testing

Stability is our main feature. We run a massive test suite covering the data model, UI lifecycle, and Rust/GTK boundary.

```bash
cargo test
```

For headless widget testing:
```bash
broadwayd :5
GDK_BACKEND=broadway BROADWAY_DISPLAY=:5 GTK_A11Y=none cargo test
```

## Author

Illya Yalovyy

- LinkedIn: [https://www.linkedin.com/in/illyayalovyy/](https://www.linkedin.com/in/illyayalovyy/)
- GitHub: [https://github.com/IllyaYalovyy](https://github.com/IllyaYalovyy)

## License

GPL-3.0-or-later
