//! rttx-server: persistent session daemon for the rttx terminal emulator.

pub mod engine;
pub mod ipc;
pub mod logging;
pub mod os;
pub mod pane;
pub mod protocol;
pub mod pty;
pub mod screen;
pub mod serialization;
pub mod server;
pub mod session;
pub mod single_instance;
