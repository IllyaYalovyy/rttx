//! Canonical versioned document models for all client persistence domains (RFC-023 §3).
//!
//! Each model corresponds to one JSON document with a schema envelope.
//! Version 1 is the initial schema for all domains.

pub mod commands;
pub mod export;
pub mod hosts;
pub mod library;
pub mod preferences;
pub mod runtime_cache;
pub mod ui;
pub mod workspaces;
