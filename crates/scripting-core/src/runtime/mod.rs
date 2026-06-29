// Top-level scripting runtime: owns both subsystems and dispatches by file
// extension. See: context/lib/scripting.md

mod compile;
mod core;
mod data_script;
mod mod_init;
mod mod_init_exec;
mod types;

pub use types::{
    Frontend, MenuCamera, ModManifestResult, ModMapEntry, ReloadSummary, ScriptRuntime,
    ScriptRuntimeConfig, StagedManifestCommitOutcome,
};
