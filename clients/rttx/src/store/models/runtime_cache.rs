//! Canonical runtime-cache document model (RFC-023 §3: `runtime-cache.json`).
//!
//! This is disposable cache. The application must behave correctly if this file
//! is deleted.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::store::envelope::Schema;

pub const SCHEMA: Schema = Schema::RuntimeCache;
pub const CURRENT_VERSION: u32 = 1;

/// Disposable client cache for runtime discovery and dismissal state.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeCache {
    /// Runtime IDs the user explicitly dismissed.
    #[serde(default)]
    pub dismissed_runtime_ids: BTreeSet<String>,
}


