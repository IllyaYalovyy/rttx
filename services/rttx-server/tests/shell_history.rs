//! Per-shell durable-history crash-survival integration tests (RFC-031 §7).
//!
//! Each test spawns the real shell in a PTY using the exact command + env that
//! the daemon would use (`shell_init::build`), runs a command, simulates a hard
//! crash by dropping the PTY, and asserts the command survived in the per-pane
//! history — then that a freshly respawned shell recalls it under the same
//! `PaneId`.
//!
//! Tests skip with a clear message when the shell is not installed, so CI on a
//! minimal image still passes while developer machines with zsh/fish get full
//! coverage.

use rttx_server::pty::{Pty, PtyConfig};
use rttx_server::shell_init;
use std::path::Path;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Resolve a shell executable on `$PATH`, or `None` if it is not installed.
fn find_shell(name: &str) -> Option<String> {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name}"))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
}

/// Write a line to the PTY.
async fn send(pty: &mut Pty, line: &str) {
    pty.write_all(line.as_bytes()).await.unwrap();
}

/// Drain PTY output for `dur`, returning everything read as a lossy string.
async fn drain(pty: &mut Pty, dur: Duration) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let deadline = Instant::now() + dur;
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(150), pty.read(&mut chunk)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
            _ => {}
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Poll `path` until it contains `needle` or the deadline elapses.
fn wait_for_file_contains(path: &Path, needle: &str, dur: Duration) -> bool {
    let deadline = Instant::now() + dur;
    while Instant::now() < deadline {
        if std::fs::read_to_string(path).unwrap_or_default().contains(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn spawn(command: Vec<String>, env: Vec<(String, String)>, cwd: &Path) -> Pty {
    Pty::spawn(
        Uuid::new_v4(),
        &PtyConfig { command, cwd: Some(cwd.to_path_buf()), env, cols: 80, rows: 24 },
    )
    .unwrap()
}

/// bash: history survives a crash even when the user's rc sets its own
/// `PROMPT_COMMAND`, and a respawned shell recalls it.
#[tokio::test]
async fn bash_history_survives_crash_with_hostile_user_promptcommand() {
    let Some(bash) = find_shell("bash") else {
        eprintln!("SKIPPED: bash not installed");
        return;
    };

    let state = tempfile::TempDir::new().unwrap();
    let home = tempfile::TempDir::new().unwrap();
    // Hostile user rc: sets its own PROMPT_COMMAND that does NOT flush history.
    std::fs::write(home.path().join(".bashrc"), "PROMPT_COMMAND='echo USER_RC_ACTIVE'\n").unwrap();

    let runtime_id = Uuid::new_v4();
    let pane_id = Uuid::new_v4();
    let marker = "RTTX_MARKER_BASH_7f3a";

    let env_with_home = |spawn: shell_init::ShellSpawn| {
        let mut env = spawn.env;
        env.push(("HOME".to_string(), home.path().to_string_lossy().into_owned()));
        (spawn.command, env)
    };

    {
        let s = shell_init::build(&bash, state.path(), runtime_id, pane_id, false);
        let (command, env) = env_with_home(s);
        let mut pty = spawn(command, env, home.path());
        drain(&mut pty, Duration::from_millis(400)).await;
        send(&mut pty, &format!("echo {marker}\n")).await;
        drain(&mut pty, Duration::from_millis(300)).await;
        // Trigger another prompt so PROMPT_COMMAND runs history -a.
        send(&mut pty, "true\n").await;
        drain(&mut pty, Duration::from_millis(300)).await;

        let hist = rttx_server::state::layout::history_file(state.path(), runtime_id, pane_id);
        assert!(
            wait_for_file_contains(&hist, marker, Duration::from_secs(5)),
            "bash history must be flushed to {} despite the user's PROMPT_COMMAND",
            hist.display()
        );
        // Drop pty → kill_on_drop simulates a hard crash with no clean exit.
    }

    // Respawn under the same PaneId and confirm recall.
    let s = shell_init::build(&bash, state.path(), runtime_id, pane_id, false);
    let (command, env) = env_with_home(s);
    let mut pty = spawn(command, env, home.path());
    drain(&mut pty, Duration::from_millis(400)).await;
    send(&mut pty, "history\n").await;
    let out = drain(&mut pty, Duration::from_secs(2)).await;
    assert!(out.contains(marker), "respawned bash must recall prior history, got: {out}");
}

/// zsh: `INC_APPEND_HISTORY` flushes each command to the per-pane HISTFILE, so
/// it survives a crash and is recalled after respawn.
#[tokio::test]
async fn zsh_history_survives_crash() {
    let Some(zsh) = find_shell("zsh") else {
        eprintln!("SKIPPED: zsh not installed");
        return;
    };

    let state = tempfile::TempDir::new().unwrap();
    let home = tempfile::TempDir::new().unwrap();
    std::fs::write(home.path().join(".zshrc"), "# user zshrc\n").unwrap();

    let runtime_id = Uuid::new_v4();
    let pane_id = Uuid::new_v4();
    let marker = "RTTX_MARKER_ZSH_91c2";

    let with_env = |s: shell_init::ShellSpawn| {
        let mut env = s.env;
        // Point the generated .zshrc at our fake user config.
        env.push(("RTTX_USER_ZDOTDIR".to_string(), home.path().to_string_lossy().into_owned()));
        env.push(("HOME".to_string(), home.path().to_string_lossy().into_owned()));
        env
    };

    {
        let s = shell_init::build(&zsh, state.path(), runtime_id, pane_id, false);
        // zsh keeps the default login shell, so provide the binary explicitly.
        let env = with_env(s);
        let mut pty = spawn(vec![zsh.clone()], env, home.path());
        drain(&mut pty, Duration::from_millis(500)).await;
        send(&mut pty, &format!("echo {marker}\n")).await;
        drain(&mut pty, Duration::from_millis(400)).await;

        let hist = rttx_server::state::layout::history_file(state.path(), runtime_id, pane_id);
        assert!(
            wait_for_file_contains(&hist, marker, Duration::from_secs(5)),
            "zsh INC_APPEND_HISTORY must flush to {}",
            hist.display()
        );
    }

    let s = shell_init::build(&zsh, state.path(), runtime_id, pane_id, false);
    let env = with_env(s);
    let mut pty = spawn(vec![zsh.clone()], env, home.path());
    drain(&mut pty, Duration::from_millis(500)).await;
    send(&mut pty, "fc -l 1\n").await;
    let out = drain(&mut pty, Duration::from_secs(2)).await;
    assert!(out.contains(marker), "respawned zsh must recall prior history, got: {out}");
}

/// fish: per-pane history session autosaves after each command, so it survives
/// a crash and is recalled after respawn.
#[tokio::test]
async fn fish_history_survives_crash() {
    let Some(fish) = find_shell("fish") else {
        eprintln!("SKIPPED: fish not installed");
        return;
    };

    let state = tempfile::TempDir::new().unwrap();
    let home = tempfile::TempDir::new().unwrap();
    let data = tempfile::TempDir::new().unwrap();

    let runtime_id = Uuid::new_v4();
    let pane_id = Uuid::new_v4();
    let marker = "RTTX_MARKER_FISH_5d80";

    let with_env = |s: shell_init::ShellSpawn| {
        let mut env = s.env;
        env.push(("HOME".to_string(), home.path().to_string_lossy().into_owned()));
        // Isolate fish's data dir so the test does not touch the real history.
        env.push(("XDG_DATA_HOME".to_string(), data.path().to_string_lossy().into_owned()));
        (s.command, env)
    };

    let hist = data.path().join(format!("fish/rttx_{}_history", pane_id.simple()));

    {
        let s = shell_init::build(&fish, state.path(), runtime_id, pane_id, false);
        let (command, env) = with_env(s);
        let mut pty = spawn(command, env, home.path());
        drain(&mut pty, Duration::from_millis(600)).await;
        send(&mut pty, &format!("echo {marker}\n")).await;
        drain(&mut pty, Duration::from_millis(500)).await;

        assert!(
            wait_for_file_contains(&hist, marker, Duration::from_secs(5)),
            "fish per-session history must autosave to {}",
            hist.display()
        );
    }

    let s = shell_init::build(&fish, state.path(), runtime_id, pane_id, false);
    let (command, env) = with_env(s);
    let mut pty = spawn(command, env, home.path());
    drain(&mut pty, Duration::from_millis(600)).await;
    send(&mut pty, "history\n").await;
    let out = drain(&mut pty, Duration::from_secs(2)).await;
    assert!(out.contains(marker), "respawned fish must recall prior history, got: {out}");
}

/// other (POSIX sh): `HISTFILE` is set best-effort even though POSIX shells do
/// not flush incrementally. We assert the env is wired, documenting the limit.
#[tokio::test]
async fn other_shell_sets_histfile_env() {
    let Some(sh) = find_shell("dash").or_else(|| find_shell("sh")) else {
        eprintln!("SKIPPED: no POSIX sh available");
        return;
    };
    let state = tempfile::TempDir::new().unwrap();
    let runtime_id = Uuid::new_v4();
    let pane_id = Uuid::new_v4();

    let s = shell_init::build(&sh, state.path(), runtime_id, pane_id, false);
    let hist = rttx_server::state::layout::history_file(state.path(), runtime_id, pane_id);
    assert!(s.command.is_empty(), "other shells keep the default login shell");
    assert_eq!(s.env, vec![("HISTFILE".to_string(), hist.to_string_lossy().into_owned())]);
}


/// Run a unique marker command in a fresh daemon-configured bash pane, wait for
/// it to flush to the per-pane HISTFILE, then drop the PTY (`kill_on_drop`
/// simulates a hard crash with no clean exit).
async fn run_marker_and_crash(
    bash: &str,
    state_dir: &Path,
    home_dir: &Path,
    runtime_id: Uuid,
    pane_id: Uuid,
    marker: &str,
) {
    let s = shell_init::build(bash, state_dir, runtime_id, pane_id, false);
    let mut env = s.env;
    env.push(("HOME".to_string(), home_dir.to_string_lossy().into_owned()));
    let mut pty = spawn(s.command, env, home_dir);
    drain(&mut pty, Duration::from_millis(400)).await;
    send(&mut pty, &format!("echo {marker}\n")).await;
    drain(&mut pty, Duration::from_millis(300)).await;
    // Trigger another prompt so PROMPT_COMMAND runs `history -a`.
    send(&mut pty, "true\n").await;
    drain(&mut pty, Duration::from_millis(300)).await;
    let hist = rttx_server::state::layout::history_file(state_dir, runtime_id, pane_id);
    assert!(
        wait_for_file_contains(&hist, marker, Duration::from_secs(5)),
        "pane history must flush to {}",
        hist.display()
    );
    // `pty` drops here → hard crash.
}

/// #987 acceptance: two panes in the same workspace keep *distinct* histories
/// keyed on their `PaneId`. A command run in one pane survives a crash in that
/// pane's history only and never leaks into the other pane ("arrow-up shows
/// commands specific to that pane, not shared across all panes"). Uses the
/// daemon's own `shell_init::build` config, so no manual shell configuration is
/// involved.
#[tokio::test]
async fn distinct_panes_keep_separate_history_across_crash() {
    let Some(bash) = find_shell("bash") else {
        eprintln!("SKIPPED: bash not installed");
        return;
    };

    let state = tempfile::TempDir::new().unwrap();
    let home = tempfile::TempDir::new().unwrap();
    // A benign user rc that does not flush history itself.
    std::fs::write(home.path().join(".bashrc"), "export PS1='$ '\n").unwrap();

    let runtime_id = Uuid::new_v4();
    let pane_one = Uuid::new_v4();
    let pane_two = Uuid::new_v4();
    let marker_one = "RTTX_PANE_ONE_a1b2";
    let marker_two = "RTTX_PANE_TWO_c3d4";

    run_marker_and_crash(&bash, state.path(), home.path(), runtime_id, pane_one, marker_one).await;
    run_marker_and_crash(&bash, state.path(), home.path(), runtime_id, pane_two, marker_two).await;

    let hist_one = rttx_server::state::layout::history_file(state.path(), runtime_id, pane_one);
    let hist_two = rttx_server::state::layout::history_file(state.path(), runtime_id, pane_two);
    let content_one = std::fs::read_to_string(&hist_one).unwrap_or_default();
    let content_two = std::fs::read_to_string(&hist_two).unwrap_or_default();

    assert!(content_one.contains(marker_one), "pane one keeps its own command: {content_one:?}");
    assert!(content_two.contains(marker_two), "pane two keeps its own command: {content_two:?}");
    // The crux: no cross-contamination between panes.
    assert!(
        !content_one.contains(marker_two),
        "pane one history must not contain pane two's command: {content_one:?}"
    );
    assert!(
        !content_two.contains(marker_one),
        "pane two history must not contain pane one's command: {content_two:?}"
    );

    // And a respawned pane recalls only its own history.
    let s = shell_init::build(&bash, state.path(), runtime_id, pane_one, false);
    let mut env = s.env;
    env.push(("HOME".to_string(), home.path().to_string_lossy().into_owned()));
    let mut pty = spawn(s.command, env, home.path());
    drain(&mut pty, Duration::from_millis(400)).await;
    send(&mut pty, "history\n").await;
    let out = drain(&mut pty, Duration::from_secs(2)).await;
    assert!(out.contains(marker_one), "respawned pane one recalls its own command: {out}");
    assert!(!out.contains(marker_two), "respawned pane one must not recall pane two's command: {out}");
}
