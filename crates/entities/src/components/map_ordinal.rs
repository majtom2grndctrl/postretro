//! Position in the authored PRL map-entity list, stamped before filtering.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapOrdinal(pub u32);
