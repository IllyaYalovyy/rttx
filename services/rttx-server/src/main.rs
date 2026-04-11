//! CLI entry point for rttx-server.

use clap::{Parser, Subcommand};
use rttx_proto::proto;
use rttx_server::ipc;
use rttx_server::os::OsInterface;
use rttx_server::os::unix::UnixOs;
use rttx_server::server::Server;
use std::sync::Arc;
use tokio::sync::Mutex;

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
    /// Remove all sessions with no connected clients
    Clean,
    /// Serve one client over stdin/stdout (for SSH)
    AttachStdio,
    /// Show the path to the daemon log file
    Logs,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Start { foreground: false }) {
        Command::Start { foreground } => start(foreground),
        Command::Stop => stop(),
        Command::Status => status(),
        Command::Clean => clean(),
        Command::AttachStdio => attach_stdio(),
        Command::Logs => {
            logs();
            Ok(())
        }
    }
}

fn init_tracing(dev_mode: bool, log_dir: &std::path::Path) {
    rttx_server::logging::init_file_logging(log_dir, "rttx-server", dev_mode);
}

fn start(foreground: bool) -> anyhow::Result<()> {
    let dev_mode = rttx_server::os::unix::dev_mode_enabled();

    let os = UnixOs;
    let runtime_dir = os.runtime_dir();
    let lock_path = runtime_dir.join("rttx-server.lock");
    let pid_path = runtime_dir.join("rttx-server.pid");

    // Acquire the single-instance lock before any other initialization.
    // The lock is inherited across fork (daemonize) and held until process exit.
    let _instance_guard =
        match rttx_server::single_instance::SingleInstanceGuard::try_acquire(&lock_path) {
            Ok(guard) => guard,
            Err(rttx_server::single_instance::SingleInstanceError::AlreadyRunning) => {
                let mode = if dev_mode { "rttx-server (dev)" } else { "rttx-server" };
                eprintln!("{mode} is already running");
                std::process::exit(1);
            }
            Err(e) => return Err(e.into()),
        };

    if !foreground {
        let daemon = daemonize::Daemonize::new().pid_file(&pid_path).working_directory(".");

        match daemon.start() {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Failed to daemonize: {e}");
                std::process::exit(1);
            }
        }
    }

    init_tracing(dev_mode, &os.cache_dir());

    if dev_mode {
        tracing::info!("Starting rttx-server in DEVELOPMENT mode");
        tracing::debug!(runtime_dir = %runtime_dir.display(), cache_dir = %os.cache_dir().display());
    }

    // Write PID file in foreground mode (daemonize writes it in daemon mode).
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

    // Cleanup PID file on normal exit. Lock file is cleaned up by guard drop.
    let _ = std::fs::remove_file(&pid_path);
    result
}

/// Serve a single client over stdin/stdout.
///
/// Intended to be invoked via SSH: `ssh host rttx-server attach-stdio`.
/// Connects to the already-running local daemon and bridges the client's
/// stdin/stdout to the daemon socket. The daemon keeps running after the
fn logs() {
    let os = UnixOs;
    let log_dir = os.cache_dir();
    println!("{}", log_dir.display());
}

/// SSH connection drops, so PTYs and runtimes survive GUI restarts.
fn attach_stdio() -> anyhow::Result<()> {
    let dev_mode = rttx_server::os::unix::dev_mode_enabled();
    let os = UnixOs;
    init_tracing(dev_mode, &os.cache_dir());

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

fn clean() -> anyhow::Result<()> {
    use bytes::BytesMut;
    use rttx_proto::{decode_frame, encode_frame};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let os = UnixOs;
    let socket_path = os.runtime_dir().join("rttx-server.sock");

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        if !ipc::is_server_running(&socket_path).await {
            println!("Daemon is not running");
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

        loop {
            stream.read_buf(&mut read_buf).await?;
            if decode_frame::<proto::ServerMessage>(&mut read_buf).is_ok() {
                break;
            }
        }

        // ListSessions.
        buf.clear();
        encode_frame(
            &proto::ClientMessage {
                msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
            },
            &mut buf,
        )?;
        stream.write_all(&buf).await?;
        stream.flush().await?;

        let resp: proto::ServerMessage = loop {
            stream.read_buf(&mut read_buf).await?;
            match decode_frame::<proto::ServerMessage>(&mut read_buf) {
                Ok(msg) => break msg,
                Err(rttx_proto::FrameError::Incomplete) => {}
                Err(e) => anyhow::bail!("decode error: {e}"),
            }
        };

        let sessions = match resp.msg {
            Some(proto::server_message::Msg::SessionList(sl)) => sl.sessions,
            _ => anyhow::bail!("unexpected response"),
        };

        let unused: Vec<_> = sessions.iter().filter(|s| s.attached_client_count == 0).collect();

        if unused.is_empty() {
            println!("No unused sessions");
            return Ok(());
        }

        let mut cleaned = 0u32;
        for session in &unused {
            buf.clear();
            encode_frame(
                &proto::ClientMessage {
                    msg: Some(proto::client_message::Msg::TerminateSession(
                        proto::TerminateSession { session_id: session.id.clone() },
                    )),
                },
                &mut buf,
            )?;
            stream.write_all(&buf).await?;
            stream.flush().await?;

            // Wait for SessionTerminated.
            loop {
                stream.read_buf(&mut read_buf).await?;
                match decode_frame::<proto::ServerMessage>(&mut read_buf) {
                    Ok(msg) => {
                        if matches!(msg.msg, Some(proto::server_message::Msg::SessionTerminated(_)))
                        {
                            let name = truncate(&session.name, 40);
                            println!("Removed: {name}");
                            cleaned += 1;
                            break;
                        }
                    }
                    Err(rttx_proto::FrameError::Incomplete) => {}
                    Err(e) => anyhow::bail!("decode error: {e}"),
                }
            }
        }

        println!("Cleaned {cleaned} session{}", if cleaned == 1 { "" } else { "s" });
        Ok(())
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_string() } else { format!("{}…", &s[..max - 1]) }
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn cli_parses_clean_command() {
        let cli = Cli::try_parse_from(["rttx-server", "clean"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Clean)));
    }
}
