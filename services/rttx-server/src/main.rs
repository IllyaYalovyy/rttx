//! CLI entry point for rttx-server.
//!
//! Supports `start`, `stop`, and `--foreground` commands.

use rttx_proto::proto;
use rttx_server::ipc;
use rttx_server::os::OsInterface;
use rttx_server::os::unix::UnixOs;
use rttx_server::server::Server;
use std::sync::Arc;
use tokio::sync::Mutex;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map_or("start", String::as_str);
    let foreground = args.iter().any(|a| a == "--foreground");

    match command {
        "start" => start(foreground),
        "stop" => stop(),
        "attach-stdio" => attach_stdio(),
        "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        other => {
            eprintln!("Unknown command: {other}");
            print_usage();
            std::process::exit(1);
        }
    }
}

fn print_usage() {
    eprintln!("Usage: rttx-server <command> [options]");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  start [--foreground]  Start the daemon");
    eprintln!("  stop                  Stop the running daemon");
    eprintln!("  attach-stdio          Serve one client over stdin/stdout (for SSH)");
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

    // Initialize logging. In dev mode, default to debug level.
    let default_level = if dev_mode { "debug" } else { "info" };
    let env_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| default_level.to_string());
    pretty_env_logger::formatted_builder().parse_filters(&env_filter).init();

    if dev_mode {
        log::info!("Starting rttx-server in DEVELOPMENT mode");
        log::debug!("Runtime dir: {}", runtime_dir.display());
        log::debug!("Cache dir: {}", os.cache_dir().display());
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
/// The daemon must already be running on the remote host. This process
/// connects to the local daemon socket and bridges the client's
/// stdin/stdout to it, but that would add latency. Instead, we load
/// state and serve directly — the stdio process IS the server for this
/// client.
fn attach_stdio() -> anyhow::Result<()> {
    // Stderr is available for logging (stdout is the protocol channel).
    let dev_mode = rttx_server::os::unix::dev_mode_enabled();
    let default_level = if dev_mode { "debug" } else { "info" };
    let env_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| default_level.to_string());
    pretty_env_logger::formatted_builder().parse_filters(&env_filter).init();

    let os = UnixOs;
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let server = Arc::new(Mutex::new(Server::new(Box::new(os))));

        {
            let mut s = server.lock().await;
            s.load_persisted_state();
        }

        Server::reconstruct_sessions(&server).await;

        let ser_server = Arc::clone(&server);
        let mut ser_shutdown_rx = {
            let s = server.lock().await;
            s.shutdown_rx()
        };
        tokio::spawn(async move {
            rttx_server::server::serialization_loop(
                ser_server,
                std::time::Duration::from_secs(1),
                &mut ser_shutdown_rx,
            )
            .await;
        });

        rttx_server::server::handle_stdio_client(server).await
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
        log::error!("Failed to register signal handlers");
        return;
    };

    if signals.next().await.is_some() {
        log::info!("Received OS shutdown signal, triggering cooperative shutdown");
        let s = server.lock().await;
        s.request_shutdown();
    }
}
