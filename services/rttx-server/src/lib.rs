//! rttx-server: persistent runtime daemon for the rttx terminal emulator.

pub mod diagnostics;
pub mod engine;
pub mod ipc;
pub mod logging;
pub mod os;
pub mod pane;
pub mod protocol;
pub mod pty;
pub mod runtime;
pub mod screen;
pub mod server;
pub mod single_instance;
pub mod state;
