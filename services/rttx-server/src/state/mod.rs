//! State storage modules for the rttx daemon.
//!
//! This module organizes the v2 per-runtime directory layout defined by
//! RFC-022. The v1 monolithic `state.json` path helpers remain in
//! [`crate::serialization`] until the migration is complete.

pub mod cleanup;
pub mod io;
pub mod layout;
pub mod migrations;
pub mod persistence;
pub mod types;
