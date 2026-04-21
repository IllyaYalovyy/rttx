//! Canonical hosts document model (RFC-023 §3: `hosts.json`).

use serde::{Deserialize, Serialize};

use crate::store::envelope::Schema;

pub const SCHEMA: Schema = Schema::Hosts;
pub const CURRENT_VERSION: u32 = 1;

/// Kind of host endpoint.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HostKind {
    #[default]
    Local,
    Remote,
}

/// A saved endpoint record. `local` is a reserved built-in and not persisted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostRecord {
    pub key: String,
    pub name: String,
    pub kind: HostKind,
    #[serde(default)]
    pub ssh_target: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
}

/// Top-level hosts catalog.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostCatalog {
    #[serde(default)]
    pub hosts: Vec<HostRecord>,
}
