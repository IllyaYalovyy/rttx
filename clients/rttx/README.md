# rttx

A tiling terminal emulator for GNOME, built with Rust, GTK4, and Libadwaita, organized around named workspaces and split panes.

This package lives in the `clients/rttx/` subtree of the consolidated rttx repository.

Spiritual successor to [Tilix](https://github.com/gnunn1/tilix), rewritten from scratch for the modern GNOME desktop.

![rttx screenshot](data/screenshots/rttx-main.png)

## Development

Build the client package:

```bash
cargo build -p rttx
```

Run the client in normal mode:

```bash
cargo run -p rttx
```

Run the client in development mode:

```bash
RTTX_DEV_MODE=1 cargo run -p rttx
```

Build the whole workspace when you are changing client, daemon, and protocol together:

```bash
cargo build --workspace
```

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

Run these commands from the repository root. Meson installs the client only.

Production install from source:

```bash
meson setup build --prefix=/usr/local
meson compile -C build
sudo meson install -C build
sudo gtk-update-icon-cache -f -t /usr/local/share/icons/hicolor
sudo update-desktop-database /usr/local/share/applications
```

If `build/` already exists, reconfigure it:

```bash
meson setup --reconfigure build --prefix=/usr/local
meson compile -C build
```

## Terminology

- **Window** — one rttx application window.
- **Tab** — not a separate rttx object. In most places where other terminal apps would say "tab", rttx uses a workspace.
- **Workspace** — the top-level GUI object listed in the left sidebar. A workspace contains panes, a layout, and user-facing presentation state.
- **Runtime** — the live backend object owned by `rttx-server` for a daemon-backed workspace. It owns PTYs, scrollback, CWD, runtime titles, and process lifetime independently from the GUI.
- **Pane** — one terminal pane inside a workspace.
- **Layout** — the arrangement of panes and split ratios inside a workspace.
- **Endpoint** — the local daemon or one remote host daemon that serves runtimes.
- **Policy** — the runtime retention model for a workspace: `ephemeral` or `persistent`. Both policies are daemon-backed.
- **Bookmark** — a saved launch target such as a folder, SSH host, tmux session, or a combination of them.
- **Command** — a saved command snippet you can run or insert into a pane.

Current Rust modules and persisted types still use `Session*` names in places. In product docs and UI discussions, `Workspace` and `Runtime` are the preferred terms.

## Architecture Direction

- Managed local and remote execution is daemon-backed through `rttx-server`.
- A workspace chooses a runtime policy: `ephemeral` or `persistent`.
- rttx does not silently fall back to a different execution model when a daemon or SSH connection is unavailable.
- Transient endpoint failures should reconnect automatically; failures that need user action stay explicit in the workspace UI.
- GUI state and daemon state reconcile non-destructively. Missing GUI metadata must never delete a daemon runtime or pane automatically.

## Features

### Workspaces and layouts

- Create named workspaces in the left sidebar for separate work contexts
- Split a workspace into panes horizontally or vertically, up to 5 levels deep
- Drag pane headers to rearrange a workspace layout
- Broadcast keystrokes to all panes in a workspace (input sync)

### Bookmarks and commands

- Save folder, SSH, tmux, or combined bookmarks for quick access
- Run bookmarks in the current pane or open them as new workspaces
- Save and search reusable commands in the right sidebar

### Recovery and reconnect

- Workspace layouts, split sizes, and working directories persist automatically
- Bookmark-driven panes restore as explicit targets (local folder, SSH, tmux, or combined)
- Failed SSH/tmux pane recovery offers in-pane retry — no modal dialogs
- tmux recovery reattaches to existing sessions, never creates new ones silently
- Daemon-backed workspaces are expected to reconnect explicitly instead of silently degrading to a different execution path

### Terminal

- Clickable URLs, OSC 8 hyperlinks, and detected file paths
- Optional smart clipboard: plain Ctrl+C copies selected text, Ctrl+V pastes
- Built-in Nightfall and Daybreak themes, with Tilix color scheme compatibility
- Background process notifications via toast (foreground) or desktop notification (background)

## Keyboard Shortcuts

| Action | Shortcut |
|---|---|
| New workspace | Ctrl+Shift+T |
| Close pane | Ctrl+Shift+W |
| Split horizontal | Ctrl+Shift+E |
| Split vertical | Ctrl+Shift+O |
| Toggle workspace sidebar | Ctrl+Shift+N |
| Toggle tools sidebar | Ctrl+Shift+B |
| Copy / Paste | Ctrl+Shift+C / Ctrl+Shift+V |
| Input sync toggle | Ctrl+Shift+I |
| Next / previous workspace | Ctrl+Tab / Ctrl+Shift+Tab |
| Jump to workspace 1–9 | Alt+1 through Alt+9 |
| Zoom in / out / reset | Ctrl+Plus / Ctrl+Minus / Ctrl+0 |
| Preferences | Ctrl+, |
| Fullscreen | F11 |

## Contributing

See the repository-level [CONTRIBUTING.md](../../CONTRIBUTING.md) for development setup, code standards, testing, and the pull request process.

## Author

Illya Yalovyy — [GitHub](https://github.com/IllyaYalovyy) · [LinkedIn](https://www.linkedin.com/in/illyayalovyy/)

## License

GPL-3.0-or-later
