//! CLI entry point for rttx-server.

use clap::{Parser, Subcommand};
use rttx_proto::proto;
use rttx_server::ipc;
use rttx_server::os::OsInterface;
use rttx_server::os::unix::UnixOs;
use rttx_server::server::Server;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "rttx-server",
    version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("GIT_HASH"), ")"),
    about = "Daemon runtime service for the rttx terminal emulator"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the daemon
    Start {
        /// Run in foreground instead of daemonizing
        #[arg(long)]
        foreground: bool,
    },
    /// Stop the running daemon
    Stop,
    /// Show daemon status and active runtimes
    Status,
    /// Serve one client over stdin/stdout (for SSH)
    AttachStdio,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Start { foreground: false }) {
        Command::Start { foreground } => start(foreground),
        Command::Stop => stop(),
        Command::Status => status(),
        Command::AttachStdio => attach_stdio(),
    }
}

fn init_tracing(dev_mode: bool) {
    let default_level = if dev_mode { "debug" } else { "info" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    tracing_subscriber::fmt().with_writer(std::io::stderr).with_env_filter(filter).init();
}

fn start(foreground: bool) -> anyhow::Result<()> {
    let dev_mode = rttx_server::os::unix::dev_mode_enabled();

    let os = UnixOs;
    let runtime_dir = os.runtime_dir();
    let pid_path = runtime_dir.join("rttx-server.pid");

    // Check if already running via PID file.
    if is_running_via_pid(&pid_path) {
        let mode = if dev_mode { "rttx-server (dev)" } else { "rttx-server" };
        eprintln!("{mode} is already running");
        std::process::exit(1);
    }

    if !foreground {
        std::fs::create_dir_all(&runtime_dir)?;
        let daemon = daemonize::Daemonize::new().pid_file(&pid_path).working_directory(".");

        match daemon.start() {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Failed to daemonize: {e}");
                std::process::exit(1);
            }
        }
    }

    init_tracing(dev_mode);

    if dev_mode {
        tracing::info!("Starting rttx-server in DEVELOPMENT mode");
        tracing::debug!(runtime_dir = %runtime_dir.display(), cache_dir = %os.cache_dir().display());
    }

    // Write PID file in foreground mode too (daemonize writes it in daemon mode).
    if foreground {
        std::fs::create_dir_all(&runtime_dir)?;
        std::fs::write(&pid_path, std::process::id().to_string())?;
    }

    let rt = tokio::runtime::Runtime::new()?;
    let result = rt.block_on(async {
        let server = Arc::new(Mutex::new(Server::new(Box::new(os))));

        {
            let mut s = server.lock().await;
            s.load_persisted_state();
        }

        // Reconstruct sessions: replay scrollback, spawn fresh shells.
        Server::reconstruct_sessions(&server).await;

        {
            let sig_server = Arc::clone(&server);
            tokio::spawn(async move {
                handle_signals(sig_server).await;
            });
        }

        rttx_server::server::run(server).await
    });

    // Cleanup PID file on normal exit.
    let _ = std::fs::remove_file(&pid_path);
    result
}

/// Serve a single client over stdin/stdout.
///
/// Intended to be invoked via SSH: `ssh host rttx-server attach-stdio`.
/// Connects to the already-running local daemon and bridges the client's
/// stdin/stdout to the daemon socket. The daemon keeps running after the
/// SSH connection drops, so PTYs and runtimes survive GUI restarts.
fn attach_stdio() -> anyhow::Result<()> {
    let dev_mode = rttx_server::os::unix::dev_mode_enabled();
    init_tracing(dev_mode);

    let os = UnixOs;
    let socket_path = os.runtime_dir().join("rttx-server.sock");

    if !socket_path.exists() {
        anyhow::bail!(
            "daemon socket not found at {}. Start the daemon first with: rttx-server start",
            socket_path.display()
        );
    }

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let daemon = tokio::net::UnixStream::connect(&socket_path).await?;
        let (mut daemon_read, mut daemon_write) = tokio::io::split(daemon);

        let mut stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();

        tokio::select! {
            r = tokio::io::copy(&mut stdin, &mut daemon_write) => { r?; }
            r = tokio::io::copy(&mut daemon_read, &mut stdout) => { r?; }
        }

        Ok(())
    })
}

fn stop() -> anyhow::Result<()> {
    let os = UnixOs;
    let socket_path = os.runtime_dir().join("rttx-server.sock");

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        if !ipc::is_server_running(&socket_path).await {
            eprintln!("rttx-server is not running");
            std::process::exit(1);
        }

        let stream = tokio::net::UnixStream::connect(&socket_path).await?;
        let mut conn = ipc::ClientConnection::from_stream(stream);

        let shutdown = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Shutdown(proto::Shutdown {})),
        };
        conn.send_client_message(&shutdown).await?;
        println!("Shutdown signal sent");
        Ok(())
    })
}

fn status() -> anyhow::Result<()> {
    use bytes::BytesMut;
    use rttx_proto::{decode_frame, encode_frame};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let os = UnixOs;
    let socket_path = os.runtime_dir().join("rttx-server.sock");

    println!("rttx-server {} ({})", env!("CARGO_PKG_VERSION"), env!("GIT_HASH"));
    println!("Socket: {}", socket_path.display());

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        if !ipc::is_server_running(&socket_path).await {
            println!("Status: not running");
            return Ok(());
        }

        let mut stream = tokio::net::UnixStream::connect(&socket_path).await?;
        let mut buf = BytesMut::new();
        let mut read_buf = BytesMut::with_capacity(8192);

        // Hello.
        let hello = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Hello(proto::Hello {
                protocol_version: rttx_proto::PROTOCOL_VERSION,
                client_id: rttx_proto::uuid_to_bytes(uuid::Uuid::new_v4()),
            })),
        };
        encode_frame(&hello, &mut buf)?;
        stream.write_all(&buf).await?;
        stream.flush().await?;

        // Read HelloAck.
        loop {
            stream.read_buf(&mut read_buf).await?;
            if decode_frame::<proto::ServerMessage>(&mut read_buf).is_ok() {
                break;
            }
        }

        // ListSessions.
        buf.clear();
        let list = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
        };
        encode_frame(&list, &mut buf)?;
        stream.write_all(&buf).await?;
        stream.flush().await?;

        // Read SessionList.
        let resp: proto::ServerMessage = loop {
            stream.read_buf(&mut read_buf).await?;
            match decode_frame::<proto::ServerMessage>(&mut read_buf) {
                Ok(msg) => break msg,
                Err(rttx_proto::FrameError::Incomplete) => {}
                Err(e) => anyhow::bail!("decode error: {e}"),
            }
        };

        if let Some(proto::server_message::Msg::SessionList(sl)) = resp.msg {
            println!("Status: running");
            println!("Runtimes: {}", sl.sessions.len());

            let total_panes: usize = sl.sessions.iter().map(|s| s.panes.len()).sum();
            let total_clients: u32 = sl.sessions.iter().map(|s| s.attached_client_count).sum();
            println!("Panes: {total_panes}");
            println!("Connected clients: {total_clients}");

            if !sl.sessions.is_empty() {
                println!();
                println!(
                    "{:<38} {:<20} {:<12} {:<6} {:<8}",
                    "ID", "NAME", "POLICY", "PANES", "CLIENTS"
                );
                for session in &sl.sessions {
                    let id = rttx_proto::bytes_to_uuid(&session.id)
                        .map_or_else(|_| "?".into(), |u| u.to_string());
                    let policy = match proto::RuntimePolicy::try_from(session.policy) {
                        Ok(proto::RuntimePolicy::Persistent) => "persistent",
                        Ok(proto::RuntimePolicy::Ephemeral) => "ephemeral",
                        _ => "unknown",
                    };
                    println!(
                        "{:<38} {:<20} {:<12} {:<6} {:<8}",
                        id,
                        truncate(&session.name, 20),
                        policy,
                        session.panes.len(),
                        session.attached_client_count,
                    );
                }
            }
        }

        Ok(())
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_string() } else { format!("{}…", &s[..max - 1]) }
}

/// Check if a daemon is running by reading the PID file and probing the process.
fn is_running_via_pid(pid_path: &std::path::Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(pid_path) else {
        return false;
    };
    let Ok(pid) = contents.trim().parse::<i32>() else {
        return false;
    };
    // Check if the process exists by sending signal 0.
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

async fn handle_signals(server: Arc<Mutex<Server>>) {
    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook_tokio::Signals;
    use tokio_stream::StreamExt;

    let Ok(mut signals) = Signals::new([SIGTERM, SIGINT]) else {
        tracing::error!("Failed to register signal handlers");
        return;
    };

    if signals.next().await.is_some() {
        tracing::info!("Received OS shutdown signal, triggering cooperative shutdown");
        let s = server.lock().await;
        s.request_shutdown();
    }
}
