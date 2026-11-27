//! A daemon restart must leave panes usable when a saved working directory can
//! no longer be entered, such as a permission-000 mountpoint left behind until
//! its contents are copied back.
//!
//! Permission bits do not bind root, so a root test runner (the CI container)
//! starts the daemon as `nobody`.

mod common;

use common::*;
use std::fs::Permissions;
use std::os::unix::fs::{MetadataExt, PermissionsExt, chown};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};

const NOBODY: u32 = 65534;
const TIMEOUT: Duration = Duration::from_secs(15);

/// The directories one daemon installation lives in, across restarts.
struct Sandbox {
    root: tempfile::TempDir,
    binary: PathBuf,
    unprivileged_uid: Option<u32>,
}

impl Sandbox {
    fn new() -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let unprivileged_uid =
            (std::fs::metadata(root.path()).unwrap().uid() == 0).then_some(NOBODY);
        let mut binary = PathBuf::from(env!("CARGO_BIN_EXE_rttx-server"));
        if unprivileged_uid.is_some() {
            // The build directory need not be reachable by `nobody`.
            let copy = root.path().join("rttx-server");
            std::fs::copy(&binary, &copy).unwrap();
            binary = copy;
        }
        let sandbox = Self { root, binary, unprivileged_uid };
        sandbox.own(sandbox.root.path());
        for name in ["home", "run", "cache", "state"] {
            sandbox.create_dir(name);
        }
        sandbox
    }

    fn own(&self, path: &Path) {
        if let Some(uid) = self.unprivileged_uid {
            chown(path, Some(uid), Some(uid)).unwrap();
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().canonicalize().unwrap().join(name)
    }

    fn create_dir(&self, name: &str) -> PathBuf {
        let dir = self.path(name);
        std::fs::create_dir(&dir).unwrap();
        self.own(&dir);
        dir
    }

    async fn start_daemon(&self) -> (Child, TestClient) {
        let mut command = Command::new(&self.binary);
        command
            .args(["start", "--foreground"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", "/bin/sh")
            .env("HOME", self.path("home"))
            .env("XDG_RUNTIME_DIR", self.path("run"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env("XDG_STATE_HOME", self.path("state"))
            .current_dir(self.path("home"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some(uid) = self.unprivileged_uid {
            command.uid(uid).gid(uid);
        }
        let daemon = command.spawn().expect("spawn daemon");

        let socket = self.path("run").join("rttx-server/v1/rttx-server.sock");
        let deadline = tokio::time::Instant::now() + TIMEOUT;
        while tokio::net::UnixStream::connect(&socket).await.is_err() {
            assert!(tokio::time::Instant::now() < deadline, "daemon never accepted connections");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let mut client = TestClient::connect(&socket).await;
        client.handshake().await;
        (daemon, client)
    }
}

async fn stop_daemon(mut daemon: Child) {
    let pid = nix::unistd::Pid::from_raw(daemon.id().expect("daemon pid") as i32);
    nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM).expect("signal daemon");
    let status = tokio::time::timeout(TIMEOUT, daemon.wait())
        .await
        .expect("daemon did not exit")
        .expect("wait for daemon");
    assert!(status.success(), "daemon must shut down cleanly, got {status}");
}

/// Restores a directory's permissions so the sandbox can be removed.
struct Locked(PathBuf);

impl Locked {
    fn lock(dir: &Path) -> Self {
        std::fs::set_permissions(dir, Permissions::from_mode(0o000)).unwrap();
        Self(dir.to_path_buf())
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.0, Permissions::from_mode(0o755));
    }
}

/// Type `line` into a pane and wait until its shell has printed `expected`.
async fn run_in_pane(
    client: &mut TestClient,
    runtime_id: &[u8],
    pane_id: &[u8],
    line: &str,
    expected: &str,
) {
    send_input(client, runtime_id, pane_id, format!("{line}\n").as_bytes()).await;
    let mut output = Vec::new();
    let seen = tokio::time::timeout(TIMEOUT, async {
        while !String::from_utf8_lossy(&output).contains(expected) {
            if let Some(rttx_proto::v3::server_envelope::Payload::OutputDelta(delta)) =
                client.recv().await.payload
                && delta.pane_id == pane_id
            {
                output.extend(delta.data);
            }
        }
    })
    .await;
    assert!(
        seen.is_ok(),
        "pane never printed {expected:?}; got {}",
        String::from_utf8_lossy(&output)
    );
}

fn pane<'a>(
    snapshot: &'a rttx_proto::v3::WorkspaceSnapshot,
    pane_id: &[u8],
) -> &'a rttx_proto::v3::PaneSnapshot {
    snapshot.panes.iter().find(|pane| pane.pane_id == pane_id).expect("pane survives restart")
}

#[tokio::test]
async fn restart_gives_pane_of_inaccessible_mountpoint_a_shell_in_home() {
    let sandbox = Sandbox::new();
    let home = sandbox.path("home").to_str().unwrap().to_string();
    let mountpoint = sandbox.create_dir("Projects");
    let saved = mountpoint.to_str().unwrap();
    let accessible = sandbox.create_dir("accessible").to_str().unwrap().to_string();

    let (daemon, mut client) = sandbox.start_daemon().await;
    let runtime_id =
        create_workspace(&mut client, "mountpoints", rttx_proto::v3::WorkspacePolicy::Persistent)
            .await;
    let stranded = create_pane_with_cwd(&mut client, &runtime_id, Some(saved.to_string())).await;
    let unaffected = create_pane_with_cwd(&mut client, &runtime_id, Some(accessible.clone())).await;
    let before = attach_rw(&mut client, &runtime_id).await;
    // Wait for the prompt after the output: restore drops an unfinished line.
    run_in_pane(
        &mut client,
        &runtime_id,
        &stranded,
        "PS1='idle''> '; echo BEFORE_$((40+2))",
        "idle> ",
    )
    .await;
    stop_daemon(daemon).await;

    let _locked = Locked::lock(&mountpoint);

    let (daemon, mut client) = sandbox.start_daemon().await;
    let after = attach_rw(&mut client, &runtime_id).await;

    assert_eq!(after.tree, before.tree, "layout and pane identities must survive");
    let restored = pane(&after, &stranded);
    assert_eq!(restored.exit_status, None, "the pane must have a live shell");
    assert_eq!(restored.cwd, home);
    let screen = String::from_utf8_lossy(&restored.scrollback_tail);
    assert!(screen.contains("BEFORE_42"), "scrollback lost: {screen}");
    assert!(
        screen.contains(&format!(
            "Saved directory {saved} is unavailable: permission denied. \
             This shell started in your home directory {home}."
        )),
        "notice missing: {screen}"
    );
    run_in_pane(
        &mut client,
        &runtime_id,
        &stranded,
        "echo \"AFTER_$((40+2)) in $PWD\"",
        &format!("AFTER_42 in {home}\r\n"),
    )
    .await;

    let kept = pane(&after, &unaffected);
    assert_eq!(kept.cwd, accessible);
    assert!(!String::from_utf8_lossy(&kept.scrollback_tail).contains("[rttx]"));
    run_in_pane(
        &mut client,
        &runtime_id,
        &unaffected,
        "echo \"KEPT_$((40+2)) in $PWD\"",
        &format!("KEPT_42 in {accessible}\r\n"),
    )
    .await;

    let mode = std::fs::metadata(&mountpoint).expect("mountpoint must remain").mode();
    assert_eq!(mode & 0o7777, 0o000, "restore must not touch the saved directory");
    stop_daemon(daemon).await;
}
