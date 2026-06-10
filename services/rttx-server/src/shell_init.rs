//! Shell-correct durable per-pane history (RFC-031 §7).
//!
//! Replaces the bash-only `PROMPT_COMMAND=history -a` environment hack with
//! per-shell init keyed on the durable `PaneId`, robust against the user's rc
//! files:
//!
//! - **bash** — spawn with `--rcfile <generated>` that sources `~/.bashrc`,
//!   then sets `HISTFILE` and *appends* `history -a` to `PROMPT_COMMAND`
//!   (never overwrites, so a user's `PROMPT_COMMAND` cannot disable capture).
//! - **zsh** — point `ZDOTDIR` at a generated dir whose `.zshrc` sources the
//!   user's config, then sets `HISTFILE` and `setopt INC_APPEND_HISTORY`.
//! - **fish** — select a per-pane history session via `--init-command`; fish
//!   autosaves history after every command.
//! - **other** — set `HISTFILE` best-effort (documented limitation: POSIX
//!   shells only persist on clean exit).
//!
//! On respawn the shell loads `HISTFILE` at startup, so up-arrow / Ctrl-R
//! recall survives crashes under the same `PaneId`.

use crate::state::layout;
use std::path::Path;
use uuid::Uuid;

/// Recognized interactive shells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Bash,
    Zsh,
    Fish,
    /// Any other shell (`sh`, `dash`, ...): `HISTFILE` best-effort only.
    Other,
}

impl ShellKind {
    /// Classify a shell from its executable path.
    #[must_use]
    pub fn detect(shell_path: &str) -> Self {
        match basename(shell_path) {
            "bash" => Self::Bash,
            "zsh" => Self::Zsh,
            "fish" => Self::Fish,
            _ => Self::Other,
        }
    }
}

/// The shell command override and environment additions for a pane.
#[derive(Debug, Clone, Default)]
pub struct ShellSpawn {
    /// Command override. Empty means "use the engine's default login shell".
    pub command: Vec<String>,
    /// Extra environment variables to pass to the shell.
    pub env: Vec<(String, String)>,
}

/// The user's shell, same source as the PTY engine default.
#[must_use]
pub fn user_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

/// Build the command + environment that gives a pane shell-correct, durable
/// per-pane history.
///
/// `no_persist` panes flush to `/dev/null` so they never pollute the user's
/// real history and skip the flush overhead.
#[must_use]
pub fn build(
    shell_path: &str,
    state_dir: &Path,
    runtime_id: Uuid,
    pane_id: Uuid,
    no_persist: bool,
) -> ShellSpawn {
    if no_persist {
        return ShellSpawn { command: vec![], env: vec![histfile_env("/dev/null")] };
    }

    match ShellKind::detect(shell_path) {
        ShellKind::Bash => build_bash(shell_path, state_dir, runtime_id, pane_id),
        ShellKind::Zsh => build_zsh(state_dir, runtime_id, pane_id),
        ShellKind::Fish => build_fish(shell_path, pane_id),
        ShellKind::Other => {
            ShellSpawn { command: vec![], env: vec![histfile_env(&histfile(state_dir, runtime_id, pane_id))] }
        }
    }
}

fn build_bash(shell_path: &str, state_dir: &Path, runtime_id: Uuid, pane_id: Uuid) -> ShellSpawn {
    let hist = histfile(state_dir, runtime_id, pane_id);
    let dir = shell_init_dir(state_dir, runtime_id, pane_id);
    let rcfile = dir.join("bashrc");
    let contents = format!(
        "# rttx-generated bash init (RFC-031) — do not edit\n\
         if [ -f \"$HOME/.bashrc\" ]; then . \"$HOME/.bashrc\"; fi\n\
         export HISTFILE={hist}\n\
         case \"${{PROMPT_COMMAND-}}\" in\n\
         *'history -a'*) ;;\n\
         '') PROMPT_COMMAND='history -a' ;;\n\
         *) PROMPT_COMMAND=\"${{PROMPT_COMMAND}}\"$'\\n''history -a' ;;\n\
         esac\n",
        hist = sq(&hist),
    );
    let _ = std::fs::write(&rcfile, contents);
    ShellSpawn {
        command: vec![
            shell_path.to_string(),
            "--rcfile".to_string(),
            rcfile.to_string_lossy().into_owned(),
        ],
        env: vec![],
    }
}

fn build_zsh(state_dir: &Path, runtime_id: Uuid, pane_id: Uuid) -> ShellSpawn {
    let hist = histfile(state_dir, runtime_id, pane_id);
    let dir = shell_init_dir(state_dir, runtime_id, pane_id);
    let zshrc = dir.join(".zshrc");
    let contents = format!(
        "# rttx-generated zsh init (RFC-031) — do not edit\n\
         if [ -f \"${{RTTX_USER_ZDOTDIR}}/.zshrc\" ]; then source \"${{RTTX_USER_ZDOTDIR}}/.zshrc\"; fi\n\
         export HISTFILE={hist}\n\
         setopt INC_APPEND_HISTORY\n",
        hist = sq(&hist),
    );
    let _ = std::fs::write(&zshrc, contents);
    let user_zdotdir =
        std::env::var("ZDOTDIR").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    ShellSpawn {
        command: vec![],
        env: vec![
            ("ZDOTDIR".to_string(), dir.to_string_lossy().into_owned()),
            ("RTTX_USER_ZDOTDIR".to_string(), user_zdotdir),
        ],
    }
}

fn build_fish(shell_path: &str, pane_id: Uuid) -> ShellSpawn {
    // Fish has no HISTFILE; it stores history in its own data dir keyed by a
    // session name and autosaves after every command. A pane-scoped session
    // name makes history per-pane and crash-durable.
    let session = format!("rttx_{}", pane_id.simple());
    ShellSpawn {
        command: vec![
            shell_path.to_string(),
            "-l".to_string(),
            "--init-command".to_string(),
            format!("set -g fish_history {session}"),
        ],
        env: vec![],
    }
}

/// Compute the pane history file path and ensure its parent directory exists.
fn histfile(state_dir: &Path, runtime_id: Uuid, pane_id: Uuid) -> String {
    let hist = layout::history_file(state_dir, runtime_id, pane_id);
    if let Some(parent) = hist.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    hist.to_string_lossy().into_owned()
}

/// Compute the generated shell-init directory and ensure it exists.
fn shell_init_dir(state_dir: &Path, runtime_id: Uuid, pane_id: Uuid) -> std::path::PathBuf {
    let dir = layout::shell_init_dir(state_dir, runtime_id, pane_id);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn histfile_env(path: &str) -> (String, String) {
    ("HISTFILE".to_string(), path.to_string())
}

/// Single-quote a value for safe inclusion in a generated shell script.
fn sq(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Extract the filename from a path (`/usr/bin/bash` → `bash`).
fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn ids() -> (Uuid, Uuid) {
        (Uuid::new_v4(), Uuid::new_v4())
    }

    #[test]
    fn detect_classifies_known_shells() {
        assert_eq!(ShellKind::detect("/usr/bin/bash"), ShellKind::Bash);
        assert_eq!(ShellKind::detect("/bin/zsh"), ShellKind::Zsh);
        assert_eq!(ShellKind::detect("/usr/local/bin/fish"), ShellKind::Fish);
        assert_eq!(ShellKind::detect("/bin/sh"), ShellKind::Other);
        assert_eq!(ShellKind::detect("/usr/bin/dash"), ShellKind::Other);
        assert_eq!(ShellKind::detect("bash"), ShellKind::Bash);
    }

    #[test]
    fn no_persist_flushes_to_dev_null_regardless_of_shell() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (rt, pane) = ids();
        let spawn = build("/usr/bin/bash", tmp.path(), rt, pane, true);
        assert!(spawn.command.is_empty(), "ephemeral keeps the default login shell");
        assert_eq!(spawn.env, vec![("HISTFILE".to_string(), "/dev/null".to_string())]);
        // No rcfile is generated for ephemeral panes.
        assert!(!layout::shell_init_dir(tmp.path(), rt, pane).join("bashrc").exists());
    }

    #[test]
    fn bash_uses_rcfile_that_sources_user_rc_and_appends_history_flush() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (rt, pane) = ids();
        let spawn = build("/usr/bin/bash", tmp.path(), rt, pane, false);

        assert_eq!(spawn.command[0], "/usr/bin/bash");
        assert_eq!(spawn.command[1], "--rcfile");
        let rcfile = &spawn.command[2];
        let body = std::fs::read_to_string(rcfile).unwrap();

        assert!(body.contains(". \"$HOME/.bashrc\""), "must source the user's bashrc: {body}");
        let hist = layout::history_file(tmp.path(), rt, pane);
        assert!(
            body.contains(&format!("export HISTFILE='{}'", hist.display())),
            "must export the per-pane HISTFILE: {body}"
        );
        assert!(body.contains("history -a"), "must flush history: {body}");
        // Appends, never overwrites: the user's existing PROMPT_COMMAND is
        // preserved by the `*)` arm of the case statement.
        assert!(
            body.contains("PROMPT_COMMAND=\"${PROMPT_COMMAND}\""),
            "must append to existing PROMPT_COMMAND: {body}"
        );
        // No PROMPT_COMMAND env is injected anymore.
        assert!(spawn.env.is_empty(), "bash carries history via rcfile, not env: {:?}", spawn.env);
    }

    #[test]
    fn zsh_points_zdotdir_at_generated_rc_that_sources_user_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (rt, pane) = ids();
        let spawn = build("/bin/zsh", tmp.path(), rt, pane, false);

        assert!(spawn.command.is_empty(), "zsh keeps the default login shell");
        let zdotdir = spawn
            .env
            .iter()
            .find(|(k, _)| k == "ZDOTDIR")
            .map(|(_, v)| v.clone())
            .expect("ZDOTDIR must be set");
        let generated = layout::shell_init_dir(tmp.path(), rt, pane);
        assert_eq!(std::path::Path::new(&zdotdir), generated);
        assert!(spawn.env.iter().any(|(k, _)| k == "RTTX_USER_ZDOTDIR"));

        let body = std::fs::read_to_string(generated.join(".zshrc")).unwrap();
        assert!(body.contains("RTTX_USER_ZDOTDIR"), "must source the user's zshrc: {body}");
        let hist = layout::history_file(tmp.path(), rt, pane);
        assert!(body.contains(&format!("export HISTFILE='{}'", hist.display())));
        assert!(body.contains("setopt INC_APPEND_HISTORY"), "must append incrementally: {body}");
    }

    #[test]
    fn fish_selects_a_per_pane_history_session() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (rt, pane) = ids();
        let spawn = build("/usr/bin/fish", tmp.path(), rt, pane, false);

        assert_eq!(spawn.command[0], "/usr/bin/fish");
        assert!(spawn.command.iter().any(|a| a == "--init-command"));
        let session = format!("set -g fish_history rttx_{}", pane.simple());
        assert!(
            spawn.command.iter().any(|a| a == &session),
            "fish history session must be keyed on PaneId: {:?}",
            spawn.command
        );
    }

    #[test]
    fn other_shell_sets_histfile_best_effort() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (rt, pane) = ids();
        let spawn = build("/bin/sh", tmp.path(), rt, pane, false);

        assert!(spawn.command.is_empty());
        let hist = layout::history_file(tmp.path(), rt, pane);
        assert_eq!(spawn.env, vec![("HISTFILE".to_string(), hist.to_string_lossy().into_owned())]);
    }

    #[test]
    fn histfile_path_is_keyed_on_pane_and_created() {
        let tmp = tempfile::TempDir::new().unwrap();
        let rt = Uuid::new_v4();
        let p1 = Uuid::new_v4();
        let p2 = Uuid::new_v4();
        let _ = build("/bin/sh", tmp.path(), rt, p1, false);
        let _ = build("/bin/sh", tmp.path(), rt, p2, false);
        let h1 = layout::history_file(tmp.path(), rt, p1);
        let h2 = layout::history_file(tmp.path(), rt, p2);
        assert_ne!(h1, h2, "history must be per-pane");
        assert!(h1.parent().unwrap().is_dir(), "history dir must be created");
    }

    #[test]
    fn generated_paths_are_quoted_against_spaces() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("dir with spaces");
        std::fs::create_dir_all(&dir).unwrap();
        let (rt, pane) = ids();
        let spawn = build("/usr/bin/bash", &dir, rt, pane, false);
        let body = std::fs::read_to_string(&spawn.command[2]).unwrap();
        assert!(body.contains("dir with spaces"), "path with spaces must appear quoted: {body}");
        assert!(body.contains("export HISTFILE='"), "histfile must be single-quoted: {body}");
    }
}
