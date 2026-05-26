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
    pub daemon_binary_path: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
}

/// Top-level hosts catalog.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostCatalog {
    #[serde(default)]
    pub hosts: Vec<HostRecord>,
}

// ── Conversions to/from the existing domain type ────────────

impl From<HostRecord> for crate::host::Host {
    fn from(rec: HostRecord) -> Self {
        Self {
            key: rec.key,
            name: rec.name,
            kind: match rec.kind {
                HostKind::Local => crate::host::HostKind::Local,
                HostKind::Remote => crate::host::HostKind::Remote,
            },
            ssh_target: rec.ssh_target,
            daemon_binary_path: rec.daemon_binary_path,
        }
    }
}

impl From<&crate::host::Host> for HostRecord {
    fn from(host: &crate::host::Host) -> Self {
        Self {
            key: host.key.clone(),
            name: host.name.clone(),
            kind: match host.kind {
                crate::host::HostKind::Local => HostKind::Local,
                crate::host::HostKind::Remote => HostKind::Remote,
            },
            ssh_target: host.ssh_target.clone(),
            daemon_binary_path: host.daemon_binary_path.clone(),
            labels: Vec::new(),
        }
    }
}
