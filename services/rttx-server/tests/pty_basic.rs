//! Basic PTY integration tests.

use rttx_server::pty::{Pty, PtyConfig};
use uuid::Uuid;

#[tokio::test]
async fn spawn_shell_and_read_output() {
    let config = PtyConfig {
        command: vec!["/bin/sh".into(), "-c".into(), "echo hello".into()],
        cwd: None,
        env: Vec::new(),
        cols: 80,
        rows: 24,
    };
    let mut pty = Pty::spawn(Uuid::new_v4(), &config).expect("failed to spawn PTY");

    let mut output = Vec::new();
    let mut buf = [0u8; 1024];

    // Read until EOF or we find "hello".
    for _ in 0..50 {
        match pty.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => output.extend_from_slice(&buf[..n]),
        }
        if output.windows(5).any(|w| w == b"hello") {
            break;
        }
    }

    let text = String::from_utf8_lossy(&output);
    assert!(text.contains("hello"), "expected 'hello' in output, got: {text}");
}

#[tokio::test]
async fn pty_resize() {
    let config = PtyConfig {
        command: vec!["/bin/sh".into()],
        cwd: None,
        env: Vec::new(),
        cols: 80,
        rows: 24,
    };
    let mut pty = Pty::spawn(Uuid::new_v4(), &config).expect("failed to spawn PTY");

    // Resize should not error.
    pty.resize(120, 40).expect("resize failed");

    // Clean up.
    pty.kill().expect("kill failed");
}

#[tokio::test]
async fn dropping_pty_kills_child_process() {
    let config = PtyConfig {
        command: vec!["/bin/sh".into(), "-c".into(), "sleep 60".into()],
        cwd: None,
        env: Vec::new(),
        cols: 80,
        rows: 24,
    };

    let pid = {
        let pty = Pty::spawn(Uuid::new_v4(), &config).expect("failed to spawn PTY");
        pty.pid().expect("child must be running")
    };

    let proc_path = format!("/proc/{pid}");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::path::Path::new(&proc_path).exists() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    assert!(
        !std::path::Path::new(&proc_path).exists(),
        "dropping Pty must kill child process {pid}"
    );
}

#[test]
fn pty_sets_colorterm_truecolor() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        let config = PtyConfig {
            command: vec!["/bin/sh".into(), "-c".into(), "echo COLORTERM=$COLORTERM".into()],
            cwd: None,
            env: Vec::new(),
            cols: 80,
            rows: 24,
        };
        let mut pty = Pty::spawn(Uuid::new_v4(), &config).expect("failed to spawn PTY");

        let mut output = Vec::new();
        let mut buf = [0u8; 1024];

        for _ in 0..50 {
            match pty.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => output.extend_from_slice(&buf[..n]),
            }
            if output.windows(b"COLORTERM=truecolor".len()).any(|w| w == b"COLORTERM=truecolor") {
                break;
            }
        }

        let text = String::from_utf8_lossy(&output);
        assert!(
            text.contains("COLORTERM=truecolor"),
            "PTY must set COLORTERM=truecolor for consistent CLI color support, got: {text}"
        );
    });
}

#[tokio::test]
async fn pty_exit_status() {
    let config = PtyConfig {
        command: vec!["/bin/sh".into(), "-c".into(), "exit 42".into()],
        cwd: None,
        env: Vec::new(),
        cols: 80,
        rows: 24,
    };
    let mut pty = Pty::spawn(Uuid::new_v4(), &config).expect("failed to spawn PTY");

    // Drain output so the child can exit.
    let mut buf = [0u8; 1024];
    loop {
        match pty.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }

    let status = pty.wait().await.expect("wait failed");
    assert_eq!(status, 42);
}

#[test]
fn pty_propagates_colorfgbg_from_env() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        let config = PtyConfig {
            command: vec!["/bin/sh".into(), "-c".into(), "echo COLORFGBG=$COLORFGBG".into()],
            cwd: None,
            env: vec![("COLORFGBG".into(), "0;15".into())],
            cols: 80,
            rows: 24,
        };
        let mut pty = Pty::spawn(Uuid::new_v4(), &config).expect("failed to spawn PTY");

        let mut output = Vec::new();
        let mut buf = [0u8; 1024];
        let needle = b"COLORFGBG=0;15";

        for _ in 0..50 {
            match pty.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => output.extend_from_slice(&buf[..n]),
            }
            if output.windows(needle.len()).any(|w| w == needle) {
                break;
            }
        }

        let text = String::from_utf8_lossy(&output);
        assert!(
            text.contains("COLORFGBG=0;15"),
            "PTY must propagate COLORFGBG from env config, got: {text}"
        );
    });
}
