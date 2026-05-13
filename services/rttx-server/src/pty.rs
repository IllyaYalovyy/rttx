//! PTY ownership and I/O.
//!
//! Wraps `pty-process` to provide async PTY creation, read/write, resize,
//! and child process lifecycle management.

use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

/// A running PTY with its controller and child process.
pub struct Pty {
    read_half: pty_process::OwnedReadPty,
    write_half: pty_process::OwnedWritePty,
    child: tokio::process::Child,
    id: Uuid,
}

/// Configuration for spawning a PTY process.
#[derive(Debug, Clone)]
pub struct PtyConfig {
    /// Command to execute (default: user's shell).
    pub command: Vec<String>,
    /// Working directory.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables.
    pub env: Vec<(String, String)>,
    /// Initial terminal size (cols).
    pub cols: u16,
    /// Initial terminal size (rows).
    pub rows: u16,
}

impl Default for PtyConfig {
    fn default() -> Self {
        Self { command: vec![default_shell()], cwd: None, env: Vec::new(), cols: 80, rows: 24 }
    }
}

/// Errors from PTY operations.
#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    /// PTY allocation or process spawn failed.
    #[error("pty error: {0}")]
    Pty(#[from] pty_process::Error),

    /// I/O error on the PTY fd.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl Pty {
    /// Spawn a new PTY with a child process.
    pub fn spawn(id: Uuid, config: &PtyConfig) -> Result<Self, PtyError> {
        let (pty, pts) = pty_process::open()?;
        pty.resize(pty_process::Size::new(config.rows, config.cols))?;

        let mut cmd = pty_process::Command::new(&config.command[0]);
        if config.command.len() > 1 {
            cmd = cmd.args(&config.command[1..]);
        } else {
            // Spawn as a login shell (argv[0] = "-shell") so that
            // /etc/profile.d/ scripts are sourced. This is critical for
            // OSC 7 (CWD reporting) which is set up by vte-2.91.sh.
            cmd = cmd.arg0(format!("-{}", basename(&config.command[0])));
        }
        let effective_cwd = config
            .cwd
            .as_ref()
            .map(resolve_cwd)
            .or_else(home_dir)
            .filter(|p| p.is_dir());
        if let Some(ref cwd) = effective_cwd {
            cmd = cmd.current_dir(cwd);
        }
        cmd = cmd.env("TERM", "xterm-256color").env("COLORTERM", "truecolor");
        for (key, val) in &config.env {
            cmd = cmd.env(key, val);
        }

        let child = cmd.spawn(pts)?;
        let (read_half, write_half) = pty.into_split();

        Ok(Self { read_half, write_half, child, id })
    }

    /// Read output from the PTY. Returns bytes read, 0 on EOF.
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, PtyError> {
        Ok(self.read_half.read(buf).await?)
    }

    /// Write input to the PTY.
    pub async fn write_all(&mut self, data: &[u8]) -> Result<(), PtyError> {
        self.write_half.write_all(data).await?;
        self.write_half.flush().await?;
        Ok(())
    }

    /// Resize the PTY.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        self.write_half.resize(pty_process::Size::new(rows, cols))?;
        Ok(())
    }

    /// Wait for the child process to exit and return its status code.
    pub async fn wait(&mut self) -> Result<i32, PtyError> {
        let status = self.child.wait().await?;
        Ok(status.code().unwrap_or(-1))
    }

    /// Return the pane id associated with this PTY.
    #[must_use]
    pub const fn id(&self) -> Uuid {
        self.id
    }

    /// Return the child PID, if still running.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Kill the child process.
    pub fn kill(&mut self) -> Result<(), PtyError> {
        self.child.start_kill()?;
        Ok(())
    }

    /// Consume the PTY and return its components for separate ownership.
    ///
    /// The reader goes to the PTY output loop, the writer is stored for
    /// Input/Resize routing, and the child is owned by the output loop
    /// for exit-status collection.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (pty_process::OwnedReadPty, pty_process::OwnedWritePty, tokio::process::Child) {
        (self.read_half, self.write_half, self.child)
    }
}

/// Determine the user's default shell.
fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

/// Extract the filename from a path (e.g., "/bin/bash" → "bash").
fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Resolve the user's home directory from the environment.
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

/// Expand a CWD path so it can be validated and used with `current_dir`.
///
/// - `~/…` → `$HOME/…`
/// - bare `~` → `$HOME`
/// - relative paths (no leading `/`) → `$HOME/path`
/// - absolute paths → unchanged
fn resolve_cwd(path: &PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if s == "~" {
        return home_dir().unwrap_or_else(|| path.clone());
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return home_dir().map_or_else(|| path.clone(), |h| h.join(rest));
    }
    if path.is_relative() {
        return home_dir().map_or_else(|| path.clone(), |h| h.join(path));
    }
    path.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawned_pty_child_inherits_colorterm() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async {
            let config = PtyConfig {
                command: vec!["/bin/sh".into(), "-c".into(), "sleep 60".into()],
                ..PtyConfig::default()
            };
            let mut pty = Pty::spawn(Uuid::new_v4(), &config).expect("spawn must succeed");
            let pid = pty.pid().expect("child must be running");
            let environ = std::fs::read_to_string(format!("/proc/{pid}/environ"))
                .expect("read /proc environ");
            assert!(
                environ.contains("COLORTERM=truecolor"),
                "PTY child must have COLORTERM=truecolor in its environment"
            );
            assert!(
                environ.contains("TERM=xterm-256color"),
                "PTY child must have TERM=xterm-256color in its environment"
            );
            pty.kill().expect("kill must succeed");
        });
    }

    #[test]
    fn none_cwd_resolves_to_home_directory() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async {
            let home = std::env::var("HOME").expect("HOME must be set");
            let config = PtyConfig {
                command: vec!["/bin/sh".into(), "-c".into(), "pwd".into()],
                cwd: None,
                ..PtyConfig::default()
            };
            let mut pty = Pty::spawn(Uuid::new_v4(), &config).expect("spawn must succeed");
            let output = read_pty_output(&mut pty).await;
            let cwd = output.trim();
            assert_eq!(cwd, home, "PTY with cwd=None must start in $HOME, got {cwd}");
        });
    }

    #[test]
    fn explicit_cwd_is_used() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async {
            let config = PtyConfig {
                command: vec!["/bin/sh".into(), "-c".into(), "pwd".into()],
                cwd: Some(PathBuf::from("/tmp")),
                ..PtyConfig::default()
            };
            let mut pty = Pty::spawn(Uuid::new_v4(), &config).expect("spawn must succeed");
            let output = read_pty_output(&mut pty).await;
            let cwd = output.trim();
            assert_eq!(cwd, "/tmp", "PTY with cwd=Some(\"/tmp\") must start in /tmp, got {cwd}");
        });
    }

    async fn read_pty_output(pty: &mut Pty) -> String {
        let mut buf = [0u8; 4096];
        let mut output = String::new();
        loop {
            match pty.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => output.push_str(&String::from_utf8_lossy(&buf[..n])),
            }
        }
        let _ = pty.wait().await;
        output
    }

    #[test]
    fn home_dir_returns_value_from_env() {
        let result = home_dir();
        let expected = std::env::var("HOME").ok().map(PathBuf::from);
        assert_eq!(result, expected);
    }

    #[test]
    fn default_shell_spawns_as_login_shell() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async {
            let config = PtyConfig::default();
            let mut pty = Pty::spawn(Uuid::new_v4(), &config).expect("spawn must succeed");
            let pid = pty.pid().expect("child must be running");
            let cmdline = std::fs::read(format!("/proc/{pid}/cmdline"))
                .expect("read /proc cmdline");
            let argv0 = cmdline.split(|&b| b == 0).next().unwrap_or(&[]);
            let argv0_str = std::str::from_utf8(argv0).expect("valid utf8");
            assert!(
                argv0_str.starts_with('-'),
                "default shell must be spawned as login shell (argv[0] starts with '-'), got: {argv0_str}"
            );
            pty.kill().expect("kill must succeed");
        });
    }

    #[test]
    fn basename_extracts_filename() {
        assert_eq!(basename("/bin/bash"), "bash");
        assert_eq!(basename("/usr/local/bin/zsh"), "zsh");
        assert_eq!(basename("sh"), "sh");
    }

    // ── resolve_cwd unit tests ──────────────────────────────────

    #[test]
    fn resolve_cwd_expands_bare_tilde() {
        let home = std::env::var("HOME").expect("HOME must be set");
        assert_eq!(resolve_cwd(&PathBuf::from("~")), PathBuf::from(&home));
    }

    #[test]
    fn resolve_cwd_expands_tilde_prefix() {
        let home = std::env::var("HOME").expect("HOME must be set");
        let resolved = resolve_cwd(&PathBuf::from("~/projects"));
        assert_eq!(resolved, PathBuf::from(format!("{home}/projects")));
    }

    #[test]
    fn resolve_cwd_resolves_relative_path_against_home() {
        let home = std::env::var("HOME").expect("HOME must be set");
        let resolved = resolve_cwd(&PathBuf::from("projects/myapp"));
        assert_eq!(resolved, PathBuf::from(format!("{home}/projects/myapp")));
    }

    #[test]
    fn resolve_cwd_preserves_absolute_path() {
        let resolved = resolve_cwd(&PathBuf::from("/tmp/test"));
        assert_eq!(resolved, PathBuf::from("/tmp/test"));
    }

    // ── PTY spawn with tilde CWD ────────────────────────────────

    #[test]
    fn tilde_cwd_expands_to_home() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async {
            let home = std::env::var("HOME").expect("HOME must be set");
            let config = PtyConfig {
                command: vec!["/bin/sh".into(), "-c".into(), "pwd".into()],
                cwd: Some(PathBuf::from("~")),
                ..PtyConfig::default()
            };
            let mut pty = Pty::spawn(Uuid::new_v4(), &config).expect("spawn must succeed");
            let output = read_pty_output(&mut pty).await;
            let cwd = output.trim();
            assert_eq!(cwd, home, "PTY with cwd=~ must start in $HOME, got {cwd}");
        });
    }
}
