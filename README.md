# rttx

A tiling terminal emulator for GNOME, built with Rust, GTK4, and Libadwaita.

Spiritual successor to [Tilix](https://github.com/gnunn1/tilix), rewritten from scratch for the modern GNOME desktop.

## Install

**Fedora (COPR):**

```bash
sudo dnf copr enable illya/rttx
sudo dnf install rttx
```

**Flatpak** — works on any Linux distro with Flatpak support:

```bash
flatpak install --user ./rttx.flatpak
```

**From source** — see [INSTALL.md](INSTALL.md) for full instructions.

## Features

### Tiling and sessions

- Split terminals horizontally or vertically, up to 5 levels deep
- Organize work into named sessions in the left sidebar
- Drag terminal headers to rearrange your layout
- Broadcast keystrokes to all terminals in a session (input sync)

### Bookmarks and commands

- Save folder, SSH, tmux, or combined bookmarks for quick access
- Run bookmarks in the current pane or open them as new sessions
- Save and search reusable commands in the right sidebar

### Session recovery

- Layout, split sizes, working directories, and custom titles persist automatically
- Bookmark-driven panes restore as explicit targets (local folder, SSH, tmux, or combined)
- Failed SSH/tmux connections offer in-pane retry — no modal dialogs
- tmux recovery reattaches to existing sessions, never creates new ones silently

### Terminal

- Clickable URLs, OSC 8 hyperlinks, and detected file paths
- Optional smart clipboard: plain Ctrl+C copies selected text, Ctrl+V pastes
- Built-in Nightfall and Daybreak themes, with Tilix color scheme compatibility
- Background process notifications via toast (foreground) or desktop notification (background)

## Keyboard Shortcuts

| Action | Shortcut |
|---|---|
| New session | Ctrl+Shift+T |
| Close terminal | Ctrl+Shift+W |
| Split horizontal | Ctrl+Shift+E |
| Split vertical | Ctrl+Shift+O |
| Toggle session sidebar | Ctrl+Shift+N |
| Toggle tools sidebar | Ctrl+Shift+B |
| Copy / Paste | Ctrl+Shift+C / Ctrl+Shift+V |
| Input sync toggle | Ctrl+Shift+I |
| Next / previous session | Ctrl+Tab / Ctrl+Shift+Tab |
| Jump to session 1–9 | Alt+1 through Alt+9 |
| Zoom in / out / reset | Ctrl+Plus / Ctrl+Minus / Ctrl+0 |
| Preferences | Ctrl+, |
| Fullscreen | F11 |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, code standards, testing, and the pull request process.

## Author

Illya Yalovyy — [GitHub](https://github.com/IllyaYalovyy) · [LinkedIn](https://www.linkedin.com/in/illyayalovyy/)

## License

GPL-3.0-or-later
