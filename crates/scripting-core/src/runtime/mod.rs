// Top-level scripting runtime: owns both subsystems and dispatches by file
// extension. See: context/lib/scripting.md

mod compile;
mod core;
mod data_script;
mod input_block;
mod loading_screen;
mod mod_init;
mod mod_init_exec;
mod types;

pub use input_block::{ModInputBinding, ModInputBlock, ModInputCommand, ModInputGlyphs};
pub use loading_screen::ModLoading;
pub use types::{
    Frontend, MenuCamera, ModAttenuation, ModAttenuationCurve, ModAudioProfile, ModBloomProfile,
    ModBloomResolution, ModManifestResult, ModMapEntry, ModMoverDefaults, ModRenderProfile,
    ReloadSummary, ScriptRuntime, ScriptRuntimeConfig, StagedManifestCommitOutcome,
};
pub(crate) use types::{validate_mod_manifest_id, validate_mod_manifest_version};
