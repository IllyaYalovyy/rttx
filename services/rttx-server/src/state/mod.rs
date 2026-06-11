//! State storage modules for the rttx daemon.
//!
//! This module organizes the v2 per-workspace directory layout defined by
//! RFC-022.

pub mod cleanup;
pub mod io;
pub mod layout;
pub mod migrations;
pub mod persistence;
pub mod types;
