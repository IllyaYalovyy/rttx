//! CLI entry point for rttx-server.

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use clap::{Parser, Subcommand};
use rttx_proto::v3;
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
    about = "Daemon workspace service for the rttx terminal emulator"
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
    /// Show daemon status; with a workspace id, show its per-pane detail
    Status {
        /// Workspace (runtime) id for a detailed per-pane view; omit for the
        /// fleet overview.
        runtime_id: Option<String>,
    },
    /// Remove all workspaces with no connected clients
    Clean,
    /// Terminate a specific workspace by ID
    Kill {
        /// Workspace ID (UUID) to terminate
        runtime_id: String,
    },
    /// Serve one client over stdin/stdout (for SSH)
    AttachStdio,
    /// Show the path to the daemon log file
    Logs,
    /// Show workspace memory diagnostics
    Diagnostics,
    /// Show resolved paths and configuration
    Config,
    /// Show profiling report from flight recorder data
    Profile {
        /// Dump all ring buffer events chronologically
        #[arg(long)]
        dump: bool,
        /// Read flight.prev.bin from previous daemon instance
        #[arg(long)]
        last_crash: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Continuously refresh every 2 seconds
        #[arg(long)]
        watch: bool,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Start { foreground: false }) {
        Command::Start { foreground } => start(foreground),
        Command::Stop => stop(),
        Command::Status { runtime_id } => status(runtime_id.as_deref()),
        Command::Clean => clean(),
        Command::Kill { runtime_id } => kill(&runtime_id),
        Command::AttachStdio => attach_stdio(),
        Command::Logs => {
            logs();
            Ok(())
        }
        Command::Diagnostics => diagnostics(),
        Command::Config => {
            config();
            Ok(())
        }
        Command::Profile { dump, last_crash, json, watch } => {
            profile(&ProfileOpts { dump, last_crash, json, watch })
        }
    }
}

struct ProfileOpts {
    dump: bool,
    last_crash: bool,
    json: bool,
    watch: bool,
}

fn start(foreground: bool) -> anyhow::Result<()> {
    let dev_mode = rttx_server::os::unix::dev_mode_enabled();

    let os = UnixOs;
    let runtime_dir = os.runtime_dir();
    let lock_path = runtime_dir.join("rttx-server.lock");
    let pid_path = runtime_dir.join("rttx-server.pid");

    // Acquire the single-instance lock before any other initialization.
    // The lock is inherited across fork (daemonix) and held until process exit.
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
        let daemon = daemonix::Daemonize::new().pid_file(&pid_path).working_directory("/");

        match daemon.start() {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Failed to daemonize: {e}");
                std::process::exit(1);
            }
        }
    }

    let state_dir = os.state_dir();
    let cache_dir = os.cache_dir();

    // Create ring writer and metrics before logging so the panic hook can use them.
    let metrics = Arc::new(rttx_server::metrics::DaemonMetrics::new());
    let ring = Arc::new(rttx_server::flight::RingWriter::open(&state_dir)?);
    let start_time = std::time::Instant::now();

    rttx_server::logging::init_logging_with_profiling(
        &cache_dir,
        "rttx-server",
        dev_mode,
        Arc::clone(&metrics),
        Arc::clone(&ring),
    );

    {
        let panic_metrics = Arc::clone(&metrics);
        let panic_ring = Arc::clone(&ring);
        let crash_report_path = cache_dir.join("crash-report.txt");

        std::panic::set_hook(Box::new(move |info| {
            let location = info.location().map_or_else(
                || "unknown location".to_string(),
                |loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()),
            );
            let payload = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "Box<dyn Any>".to_string());

            tracing::error!(location = %location, payload = %payload, "PANIC — daemon aborting");

            rttx_server::crash_report::record_panic_event(&panic_ring, start_time);
            rttx_server::crash_report::write_crash_report(
                &crash_report_path,
                &payload,
                &location,
                &panic_metrics,
                &panic_ring,
                start_time,
            );
        }));
    }

    tracing::info!("rttx-server {} ({}) starting", env!("CARGO_PKG_VERSION"), env!("GIT_HASH"),);

    if dev_mode {
        tracing::info!("Running in DEVELOPMENT mode");
        tracing::debug!(
            runtime_dir = %runtime_dir.display(),
            state_dir = %state_dir.display(),
            cache_dir = %cache_dir.display(),
        );
    }

    // Write PID file in foreground mode (daemonix writes it in daemon mode).
    if foreground {
        std::fs::create_dir_all(&runtime_dir)?;
        std::fs::write(&pid_path, std::process::id().to_string())?;
    }

    let rt = tokio::runtime::Runtime::new()?;
    let result = rt.block_on(async {
        let server = Arc::new(Mutex::new(Server::new(
            Box::new(os),
            Arc::clone(&metrics),
            Arc::clone(&ring),
        )));

        {
            let mut s = server.lock().await;
            s.load_persisted_state();
        }

        // Reconstruct workspaces: replay scrollback, spawn fresh shells.
        Server::reconstruct_workspaces(&server).await;

        // Start watchdog for hang detection.
        {
            let shutdown_rx = server.lock().await.shutdown_rx();
            rttx_server::watchdog::spawn_watchdog(
                Arc::clone(&server),
                Arc::clone(&metrics),
                Arc::clone(&ring),
                cache_dir.clone(),
                shutdown_rx,
                start_time,
            );
        }

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

fn config() {
    let dev_mode = rttx_server::os::unix::dev_mode_enabled();
    let os = UnixOs;

    let mode = if dev_mode { "development" } else { "production" };
    let socket = os.runtime_dir().join("rttx-server.sock");
    let state = os.state_dir();
    let scrollback = os.state_dir().join("workspaces");
    let log_dir = os.cache_dir();

    println!("Mode: {mode}");
    println!("Socket: {}", socket.display());
    println!("State: {}", state.display());
    println!("Scrollback: {}", scrollback.display());
    println!("Logs: {}", log_dir.display());
    println!("Protocol version: {}", rttx_proto::v3_handshake::V3_PROTOCOL_VERSION);
}

/// Connect to the daemon socket and perform a v3 handshake.
async fn v3_connect(
    socket_path: &std::path::Path,
) -> anyhow::Result<ipc::ClientConnection<tokio::net::UnixStream>> {
    let stream = tokio::net::UnixStream::connect(socket_path).await?;
    let mut conn = ipc::ClientConnection::from_stream(stream);

    let hello = rttx_proto::v3_handshake::build_client_hello(
        uuid::Uuid::new_v4(),
        "rttx-server-cli",
        env!("CARGO_PKG_VERSION"),
        &[
            v3::Capability::CoreWorkspaceLifecycle,
            v3::Capability::CorePaneLifecycle,
            v3::Capability::CoreTerminalIo,
            v3::Capability::CoreTerminalModes,
            v3::Capability::CorePasteIntent,
            v3::Capability::CoreFocusEvents,
            v3::Capability::OptDiagnostics,
            v3::Capability::OptWorkspaceInventory,
        ],
    );
    conn.send_v3_client_hello(&hello).await?;
    let _server_hello = conn.recv_v3_server_hello().await?;
    Ok(conn)
}

/// Receive the next v3 server envelope from the connection.
async fn recv_v3_response(
    conn: &mut ipc::ClientConnection<tokio::net::UnixStream>,
) -> anyhow::Result<v3::ServerEnvelope> {
    Ok(conn.recv_v3_envelope().await?)
}

fn profile(opts: &ProfileOpts) -> anyhow::Result<()> {
    let os = UnixOs;
    let state_dir = os.state_dir();
    let runtime_dir = os.runtime_dir();

    let filename = if opts.last_crash { "flight.prev.bin" } else { "flight.bin" };
    let flight_path = state_dir.join(filename);

    if !flight_path.exists() {
        let label =
            if opts.last_crash { "previous instance flight recorder" } else { "flight recorder" };
        eprintln!("No {label} found at {}", flight_path.display());
        if !opts.last_crash {
            eprintln!("Is the daemon running? Start it with: rttx-server start");
        }
        std::process::exit(1);
    }

    let pid = read_pid(&runtime_dir.join("rttx-server.pid"));

    if opts.watch {
        profile_watch(&flight_path, pid, opts.json, opts.dump)?;
    } else if opts.dump {
        profile_dump(&flight_path, opts.json)?;
    } else {
        profile_once(&flight_path, pid, opts.json)?;
    }

    Ok(())
}

fn profile_once(flight_path: &std::path::Path, pid: Option<u32>, json: bool) -> anyhow::Result<()> {
    let report = rttx_server::profile::generate_report(flight_path, pid)?;
    if json {
        let json_report: rttx_server::profile::JsonReport = (&report).into();
        println!("{}", serde_json::to_string_pretty(&json_report)?);
    } else {
        print!("{report}");
    }
    Ok(())
}

fn profile_dump(flight_path: &std::path::Path, json: bool) -> anyhow::Result<()> {
    let reader = rttx_server::flight::RingReader::open(flight_path)?;
    let events = reader.read_all();
    if json {
        println!("{}", rttx_server::profile::format_dump_json(&events));
    } else {
        print!("{}", rttx_server::profile::format_dump(&events));
    }
    Ok(())
}

fn profile_watch(
    flight_path: &std::path::Path,
    pid: Option<u32>,
    json: bool,
    dump: bool,
) -> anyhow::Result<()> {
    loop {
        // Clear screen.
        print!("\x1B[2J\x1B[H");
        if dump {
            profile_dump(flight_path, json)?;
        } else {
            profile_once(flight_path, pid, json)?;
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

fn read_pid(pid_path: &std::path::Path) -> Option<u32> {
    std::fs::read_to_string(pid_path).ok().and_then(|s| s.trim().parse().ok())
}

fn diagnostics() -> anyhow::Result<()> {
    let os = UnixOs;
    let socket_path = os.runtime_dir().join("rttx-server.sock");

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        if !ipc::is_server_running(&socket_path).await {
            println!("Daemon is not running");
            return Ok(());
        }

        let mut conn = v3_connect(&socket_path).await?;
        let id_gen = rttx_proto::v3_envelope::RequestIdGenerator::new();

        // GetDiagnostics.
        let env = rttx_proto::v3_envelope::build_client_envelope(
            &id_gen,
            v3::client_envelope::Command::GetDiagnostics(v3::GetDiagnostics {}),
        );
        conn.send_v3_envelope(&env).await?;

        let resp = recv_v3_response(&mut conn).await?;
        if let Some(v3::server_envelope::Payload::DiagnosticsReport(report)) = resp.payload {
            println!("Workspaces: {}", report.workspace_count);
            println!(
                "Panes: {} ({} active, {} exited)",
                report.total_pane_count, report.total_active_panes, report.total_exited_panes
            );
            println!("Connected clients: {}", report.client_count);
            println!("PTY writers: {}", report.pty_writer_count);
            println!("Total raw_bytes: {} bytes", report.total_raw_bytes);
            println!("Total pending_flush: {} bytes", report.total_pending_flush);

            if !report.workspaces.is_empty() {
                println!();
                for rt_info in &report.workspaces {
                    let id = rttx_proto::bytes_to_uuid(&rt_info.id)
                        .map_or_else(|_| "?".into(), |u| u.to_string());
                    println!("  Workspace \"{}\" ({}):", rt_info.name, &id[..8.min(id.len())]);
                    println!(
                        "    Panes: {} active, {} exited",
                        rt_info.active_pane_count, rt_info.exited_pane_count
                    );
                    println!("    Attached clients: {}", rt_info.attached_client_count);
                    for pane in &rt_info.panes {
                        let pid = rttx_proto::bytes_to_uuid(&pane.id)
                            .map_or_else(|_| "?".into(), |u| u.to_string());
                        let status = if pane.is_exited { "exited" } else { "active" };
                        println!(
                            "    Pane {} ({}): raw_bytes={}, pending_flush={}",
                            &pid[..8.min(pid.len())],
                            status,
                            pane.raw_bytes_len,
                            pane.pending_flush_len,
                        );
                    }
                }
            }
        } else {
            anyhow::bail!("unexpected response from daemon");
        }

        Ok(())
    })
}

/// SSH connection drops, so PTYs and workspaces survive GUI restarts.
fn attach_stdio() -> anyhow::Result<()> {
    let dev_mode = rttx_server::os::unix::dev_mode_enabled();
    let os = UnixOs;
    rttx_server::logging::init_file_logging(&os.cache_dir(), "rttx-server", dev_mode);

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

        let mut conn = v3_connect(&socket_path).await?;

        let shutdown = rttx_proto::v3_envelope::build_client_envelope(
            &rttx_proto::v3_envelope::RequestIdGenerator::new(),
            v3::client_envelope::Command::Shutdown(v3::Shutdown {}),
        );
        conn.send_v3_envelope(&shutdown).await?;
        println!("Shutdown signal sent");
        Ok(())
    })
}

fn status(runtime_id: Option<&str>) -> anyhow::Result<()> {
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

        let mut conn = v3_connect(&socket_path).await?;
        let id_gen = rttx_proto::v3_envelope::RequestIdGenerator::new();

        // ListWorkspaces.
        let env = rttx_proto::v3_envelope::build_client_envelope(
            &id_gen,
            v3::client_envelope::Command::ListWorkspaces(v3::ListWorkspaces {}),
        );
        conn.send_v3_envelope(&env).await?;

        let resp = recv_v3_response(&mut conn).await?;
        if let Some(v3::server_envelope::Payload::WorkspaceList(sl)) = resp.payload {
            if let Some(target) = runtime_id {
                match sl.workspaces.iter().find(|w| workspace_id_matches(&w.id, target)) {
                    Some(info) => print!("{}", format_workspace_detail(info)),
                    None => println!("Workspace '{target}' not found"),
                }
                return Ok(());
            }
            println!("Status: running");
            println!("Workspaces: {}", sl.workspaces.len());

            let total_panes: u32 = sl.workspaces.iter().map(|s| s.pane_count).sum();
            let total_clients: u32 = sl
                .workspaces
                .iter()
                .map(|s| u32::from(s.has_write_owner) + s.read_only_client_count)
                .sum();
            println!("Panes: {total_panes}");
            println!("Connected clients: {total_clients}");

            if !sl.workspaces.is_empty() {
                println!();
                println!(
                    "{:<38} {:<20} {:<12} {:<6} {:<8}",
                    "ID", "NAME", "POLICY", "PANES", "CLIENTS"
                );
                for rt_info in &sl.workspaces {
                    let id = rttx_proto::bytes_to_uuid(&rt_info.id)
                        .map_or_else(|_| "?".into(), |u| u.to_string());
                    let policy = match v3::WorkspacePolicy::try_from(rt_info.policy) {
                        Ok(v3::WorkspacePolicy::Persistent) => "persistent",
                        Ok(v3::WorkspacePolicy::Ephemeral) => "ephemeral",
                        _ => "unknown",
                    };
                    let clients =
                        u32::from(rt_info.has_write_owner) + rt_info.read_only_client_count;
                    println!(
                        "{:<38} {:<20} {:<12} {:<6} {:<8}",
                        id,
                        truncate(&rt_info.name, 20),
                        policy,
                        rt_info.pane_count,
                        clients,
                    );
                }
            }
        }

        Ok(())
    })
}

fn clean() -> anyhow::Result<()> {
    let os = UnixOs;
    let socket_path = os.runtime_dir().join("rttx-server.sock");

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        if !ipc::is_server_running(&socket_path).await {
            println!("Daemon is not running");
            return Ok(());
        }

        let mut conn = v3_connect(&socket_path).await?;
        let id_gen = rttx_proto::v3_envelope::RequestIdGenerator::new();

        // ListWorkspaces.
        let env = rttx_proto::v3_envelope::build_client_envelope(
            &id_gen,
            v3::client_envelope::Command::ListWorkspaces(v3::ListWorkspaces {}),
        );
        conn.send_v3_envelope(&env).await?;

        let resp = recv_v3_response(&mut conn).await?;
        let workspaces = match resp.payload {
            Some(v3::server_envelope::Payload::WorkspaceList(sl)) => sl.workspaces,
            _ => anyhow::bail!("unexpected response"),
        };

        let unused: Vec<_> = workspaces
            .iter()
            .filter(|s| !s.has_write_owner && s.read_only_client_count == 0)
            .collect();

        if unused.is_empty() {
            println!("No unused workspaces");
            return Ok(());
        }

        let mut cleaned = 0u32;
        for rt_info in &unused {
            let env = rttx_proto::v3_envelope::build_client_envelope(
                &id_gen,
                v3::client_envelope::Command::TerminateWorkspace(v3::TerminateWorkspace {
                    runtime_id: rt_info.id.clone(),
                }),
            );
            conn.send_v3_envelope(&env).await?;

            // Wait for WorkspaceTerminated.
            loop {
                let resp = recv_v3_response(&mut conn).await?;
                match resp.payload {
                    Some(v3::server_envelope::Payload::WorkspaceTerminated(_)) => {
                        let name = truncate(&rt_info.name, 40);
                        println!("Removed: {name}");
                        cleaned += 1;
                        break;
                    }
                    Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
                    other => anyhow::bail!("unexpected response: {other:?}"),
                }
            }
        }

        println!("Cleaned {cleaned} workspace{}", if cleaned == 1 { "" } else { "s" });
        Ok(())
    })
}

fn kill(runtime_id_str: &str) -> anyhow::Result<()> {
    let runtime_id: uuid::Uuid = runtime_id_str
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid workspace ID: not a valid UUID"))?;

    let os = UnixOs;
    let socket_path = os.runtime_dir().join("rttx-server.sock");

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        if !ipc::is_server_running(&socket_path).await {
            eprintln!("Daemon is not running");
            std::process::exit(1);
        }

        let mut conn = v3_connect(&socket_path).await?;
        let id_gen = rttx_proto::v3_envelope::RequestIdGenerator::new();

        // TerminateWorkspace.
        let env = rttx_proto::v3_envelope::build_client_envelope(
            &id_gen,
            v3::client_envelope::Command::TerminateWorkspace(v3::TerminateWorkspace {
                runtime_id: rttx_proto::uuid_to_bytes(runtime_id),
            }),
        );
        conn.send_v3_envelope(&env).await?;

        // Wait for WorkspaceTerminated or Error.
        let resp = recv_v3_response(&mut conn).await?;
        match resp.payload {
            Some(v3::server_envelope::Payload::WorkspaceTerminated(_)) => {
                println!("Workspace terminated.");
            }
            Some(v3::server_envelope::Payload::Error(e)) => {
                eprintln!("Error: {}", e.message);
                std::process::exit(1);
            }
            _ => anyhow::bail!("unexpected response from daemon"),
        }

        Ok(())
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_string() } else { format!("{}…", &s[..max - 1]) }
}

/// Match a workspace by its full UUID string or by a unique prefix (so a short
/// id from the `status` table can be passed to `status <id>`).
fn workspace_id_matches(id_bytes: &[u8], target: &str) -> bool {
    rttx_proto::bytes_to_uuid(id_bytes).is_ok_and(|u| {
        let s = u.to_string();
        s == target || s.starts_with(target)
    })
}

/// Render the per-pane detail view for a single workspace (`status <id>`).
fn format_workspace_detail(info: &v3::WorkspaceInfo) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let id = rttx_proto::bytes_to_uuid(&info.id).map_or_else(|_| "?".into(), |u| u.to_string());
    let policy = match v3::WorkspacePolicy::try_from(info.policy) {
        Ok(v3::WorkspacePolicy::Persistent) => "persistent",
        Ok(v3::WorkspacePolicy::Ephemeral) => "ephemeral",
        _ => "unknown",
    };
    let clients = u32::from(info.has_write_owner) + info.read_only_client_count;
    let _ = writeln!(out, "Workspace: {id}");
    let _ = writeln!(out, "  Name:     {}", info.name);
    let _ = writeln!(out, "  Policy:   {policy}");
    let _ = writeln!(out, "  Revision: {}", info.workspace_revision);
    let _ = writeln!(out, "  Clients:  {clients}");
    let _ = writeln!(out, "  Panes:    {}", info.pane_count);

    if info.panes.is_empty() {
        let _ = writeln!(out, "  (no pane detail reported)");
        return out;
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "  {:<10} {:<9} {:<24} CWD", "PANE", "SIZE", "TITLE");
    for p in &info.panes {
        let pid = rttx_proto::bytes_to_uuid(&p.id).map_or_else(|_| "?".into(), |u| u.to_string());
        let pid_short = &pid[..pid.len().min(8)];
        let size = format!("{}x{}", p.cols, p.rows);
        let title = if p.title.is_empty() { "—" } else { p.title.as_str() };
        let cwd = if p.cwd.is_empty() { "—" } else { p.cwd.as_str() };
        let _ =
            writeln!(out, "  {:<10} {:<9} {:<24} {}", pid_short, size, truncate(title, 24), cwd);

        let mut flags = Vec::new();
        if let Some(code) = p.exit_status {
            flags.push(format!("exited:{code}"));
        }
        if p.no_persist {
            flags.push("no-persist".to_string());
        }
        if p.reconstructed {
            flags.push("reconstructed".to_string());
        }
        if !flags.is_empty() {
            let _ = writeln!(out, "             flags: [{}]", flags.join(", "));
        }
    }
    out
}

async fn handle_signals(server: Arc<Mutex<Server>>) {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    use signal_hook_tokio::Signals;
    use tokio_stream::StreamExt;

    let Ok(mut signals) = Signals::new([SIGTERM, SIGINT, SIGHUP]) else {
        tracing::error!("Failed to register signal handlers");
        return;
    };

    if let Some(sig) = signals.next().await {
        tracing::info!(signal = sig, "Received signal, triggering cooperative shutdown");
        let s = server.lock().await;
        s.request_shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn sample_workspace_with_pane() -> v3::WorkspaceInfo {
        let ws_id = uuid::Uuid::parse_str("d7d04564-b2bf-4302-9495-e65c4df12ac6").unwrap();
        let pane_id = uuid::Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
        v3::WorkspaceInfo {
            id: rttx_proto::uuid_to_bytes(ws_id),
            name: "dev".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
            pane_count: 1,
            has_write_owner: true,
            read_only_client_count: 1,
            current_client_role: v3::WorkspaceClientRole::Writer as i32,
            workspace_revision: 7,
            reconstructed: false,
            active_pane_summary: String::new(),
            takeover_eligible: false,
            disabled_reason: String::new(),
            panes: vec![v3::PaneInfo {
                id: rttx_proto::uuid_to_bytes(pane_id),
                title: "bash".into(),
                cwd: "/home/user/project".into(),
                cols: 120,
                rows: 40,
                exit_status: Some(0),
                reconstructed: true,
                no_persist: false,
            }],
        }
    }

    #[test]
    fn workspace_id_matches_full_and_prefix() {
        let id = rttx_proto::uuid_to_bytes(
            uuid::Uuid::parse_str("d7d04564-b2bf-4302-9495-e65c4df12ac6").unwrap(),
        );
        assert!(workspace_id_matches(&id, "d7d04564-b2bf-4302-9495-e65c4df12ac6"));
        assert!(workspace_id_matches(&id, "d7d04564"), "a short prefix matches");
        assert!(!workspace_id_matches(&id, "ffffffff"));
    }

    #[test]
    fn format_workspace_detail_renders_panes_and_flags() {
        let detail = format_workspace_detail(&sample_workspace_with_pane());
        assert!(detail.contains("Workspace: d7d04564-b2bf-4302-9495-e65c4df12ac6"));
        assert!(detail.contains("Name:     dev"));
        assert!(detail.contains("persistent"));
        assert!(detail.contains("11111111"), "short pane id");
        assert!(detail.contains("120x40"), "pane size");
        assert!(detail.contains("bash"), "pane title");
        assert!(detail.contains("/home/user/project"), "pane cwd");
        assert!(detail.contains("exited:0"));
        assert!(detail.contains("reconstructed"));
    }

    #[test]
    fn format_workspace_detail_without_enriched_panes_notes_it() {
        let mut info = sample_workspace_with_pane();
        info.panes.clear();
        let detail = format_workspace_detail(&info);
        assert!(detail.contains("no pane detail reported"));
    }

    #[test]
    fn cli_parses_status_with_and_without_id() {
        let cli = Cli::try_parse_from(["rttx-server", "status"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Status { runtime_id: None })));
        let cli = Cli::try_parse_from(["rttx-server", "status", "d7d04564"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Status { runtime_id: Some(_) })));
    }

    #[test]
    fn cli_parses_clean_command() {
        let cli = Cli::try_parse_from(["rttx-server", "clean"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Clean)));
    }

    #[test]
    fn cli_parses_diagnostics_command() {
        let cli = Cli::try_parse_from(["rttx-server", "diagnostics"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Diagnostics)));
    }

    #[test]
    fn cli_parses_config_command() {
        let cli = Cli::try_parse_from(["rttx-server", "config"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Config)));
    }

    #[test]
    fn cli_parses_kill_command_with_uuid() {
        let cli =
            Cli::try_parse_from(["rttx-server", "kill", "d7d04564-b2bf-4302-9495-e65c4df12ac6"])
                .unwrap();
        match cli.command {
            Some(Command::Kill { runtime_id }) => {
                assert_eq!(runtime_id, "d7d04564-b2bf-4302-9495-e65c4df12ac6");
            }
            _ => panic!("expected Kill command"),
        }
    }

    #[test]
    fn cli_kill_requires_runtime_id_argument() {
        let result = Cli::try_parse_from(["rttx-server", "kill"]);
        assert!(result.is_err());
    }

    #[test]
    fn daemonize_builder_accepts_pid_file_and_working_directory() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("test.pid");
        // Verify the daemonix API surface we rely on compiles and builds correctly.
        let _daemon = daemonix::Daemonize::new().pid_file(&pid_path).working_directory("/");
    }

    #[test]
    fn signal_handler_registers_sighup() {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
        use signal_hook::iterator::Signals;
        let signals = Signals::new([SIGTERM, SIGINT, SIGHUP]);
        assert!(signals.is_ok(), "SIGTERM, SIGINT, SIGHUP must all be registerable");
    }

    #[test]
    fn cli_parses_profile_command_default() {
        let cli = Cli::try_parse_from(["rttx-server", "profile"]).unwrap();
        match cli.command {
            Some(Command::Profile { dump, last_crash, json, watch }) => {
                assert!(!dump);
                assert!(!last_crash);
                assert!(!json);
                assert!(!watch);
            }
            _ => panic!("expected Profile command"),
        }
    }

    #[test]
    fn cli_parses_profile_dump() {
        let cli = Cli::try_parse_from(["rttx-server", "profile", "--dump"]).unwrap();
        match cli.command {
            Some(Command::Profile { dump, .. }) => assert!(dump),
            _ => panic!("expected Profile command"),
        }
    }

    #[test]
    fn cli_parses_profile_last_crash() {
        let cli = Cli::try_parse_from(["rttx-server", "profile", "--last-crash"]).unwrap();
        match cli.command {
            Some(Command::Profile { last_crash, .. }) => assert!(last_crash),
            _ => panic!("expected Profile command"),
        }
    }

    #[test]
    fn cli_parses_profile_json() {
        let cli = Cli::try_parse_from(["rttx-server", "profile", "--json"]).unwrap();
        match cli.command {
            Some(Command::Profile { json, .. }) => assert!(json),
            _ => panic!("expected Profile command"),
        }
    }

    #[test]
    fn cli_parses_profile_watch() {
        let cli = Cli::try_parse_from(["rttx-server", "profile", "--watch"]).unwrap();
        match cli.command {
            Some(Command::Profile { watch, .. }) => assert!(watch),
            _ => panic!("expected Profile command"),
        }
    }

    #[test]
    fn cli_parses_profile_combined_flags() {
        let cli = Cli::try_parse_from(["rttx-server", "profile", "--json", "--watch"]).unwrap();
        match cli.command {
            Some(Command::Profile { json, watch, dump, last_crash }) => {
                assert!(json);
                assert!(watch);
                assert!(!dump);
                assert!(!last_crash);
            }
            _ => panic!("expected Profile command"),
        }
    }

    #[test]
    fn read_pid_from_valid_file() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("test.pid");
        std::fs::write(&pid_path, "12345\n").unwrap();
        assert_eq!(read_pid(&pid_path), Some(12345));
    }

    #[test]
    fn read_pid_missing_file_returns_none() {
        assert_eq!(read_pid(std::path::Path::new("/nonexistent/pid")), None);
    }

    #[test]
    fn global_allocator_is_mimalloc() {
        // Confirm the binary's global allocator is mimalloc. The type assertion
        // is a compile-time guarantee; the allocation exercises it at workspace.
        fn assert_mimalloc(_: &mimalloc::MiMalloc) {}
        assert_mimalloc(&GLOBAL);
        let v: Vec<u8> = vec![0u8; 4096];
        assert_eq!(v.len(), 4096);
    }

    #[test]
    fn cli_v3_handshake_includes_diagnostics_capability() {
        use rttx_proto::v3_handshake;

        let hello = v3_handshake::build_client_hello(
            uuid::Uuid::new_v4(),
            "rttx-server-cli",
            env!("CARGO_PKG_VERSION"),
            &[
                v3::Capability::CoreWorkspaceLifecycle,
                v3::Capability::CorePaneLifecycle,
                v3::Capability::CoreTerminalIo,
                v3::Capability::CoreTerminalModes,
                v3::Capability::CorePasteIntent,
                v3::Capability::CoreFocusEvents,
                v3::Capability::OptDiagnostics,
            ],
        );
        assert!(hello.capabilities.contains(&(v3::Capability::OptDiagnostics as i32)));
        assert_eq!(hello.min_protocol_version, v3_handshake::V3_PROTOCOL_VERSION);
        assert_eq!(hello.max_protocol_version, v3_handshake::V3_PROTOCOL_VERSION);
        assert_eq!(hello.client_name, "rttx-server-cli");
    }

    #[test]
    fn cli_rejects_removed_salvage_history_subcommand() {
        // The previous-iteration history-recovery subcommand no longer exists;
        // parsing it must fail rather than silently resolve to anything.
        assert!(Cli::try_parse_from(["rttx-server", "salvage-history"]).is_err());
    }
}
