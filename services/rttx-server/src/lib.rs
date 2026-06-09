//! rttx-server: persistent runtime daemon for the rttx terminal emulator.

pub mod crash_report;
pub mod diagnostics;
pub mod engine;
pub mod flight;
pub mod instrument;
pub mod ipc;
pub mod logging;
pub mod metrics;
pub mod os;
pub mod pane;
pub mod pane_tree;
pub mod profile;
pub mod profiling;
pub mod protocol;
pub mod pty;
pub mod runtime;
pub mod screen;
pub mod server;
pub mod single_instance;
pub mod state;
pub mod watchdog;
