//! Owned data transferred from the staged mod-init worker to the main thread.

use std::path::PathBuf;

use super::super::data_descriptors::{
    EntityTypeDescriptor, ImpactEventDescriptor, ModThemeTokens, RegisteredUiTree,
    TriggerPoolDescriptor,
};
use super::super::data_registry::{ScopedCrossing, ScopedReaction};
use super::super::runtime::{Frontend, ModMapEntry};
use super::super::slot_table::StoreDeclarationSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StagedManifestDiagnosticSeverity {
    Info,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedManifestDiagnostic {
    pub severity: StagedManifestDiagnosticSeverity,
    pub message: String,
}

impl StagedManifestDiagnostic {
    pub(crate) fn info(message: impl Into<String>) -> Self {
        Self {
            severity: StagedManifestDiagnosticSeverity::Info,
            message: message.into(),
        }
    }

    pub(crate) fn error(message: impl Into<String>) -> Self {
        Self {
            severity: StagedManifestDiagnosticSeverity::Error,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StagedManifest {
    pub name: String,
    pub entities: Vec<EntityTypeDescriptor>,
    pub maps: Vec<ModMapEntry>,
    pub reactions: Vec<ScopedReaction>,
    pub crossings: Vec<ScopedCrossing>,
    pub events: Vec<ImpactEventDescriptor>,
    pub trigger_events: Vec<super::super::data_descriptors::TriggerEventDescriptor>,
    pub trigger_pools: Vec<TriggerPoolDescriptor>,
    pub ui_trees: Vec<RegisteredUiTree>,
    pub theme: ModThemeTokens,
    pub frontend: Option<Frontend>,
    pub store_declarations: StoreDeclarationSet,
    /// Canonical mod-init source dependencies carried across the worker→main
    /// thread boundary. The descriptor registry write and watcher classifier
    /// update both happen on the main thread in `commit_staged_manifest_result`,
    /// where the engine registry is mutably owned; this field is what makes the
    /// dependency set available at that commit point.
    pub dependency_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StagedManifestBuildStatus {
    Built(Box<StagedManifest>),
    NoStartScript,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StagedManifestBuildResult {
    pub generation: u64,
    pub mod_root: PathBuf,
    pub status: StagedManifestBuildStatus,
    pub diagnostics: Vec<StagedManifestDiagnostic>,
}
