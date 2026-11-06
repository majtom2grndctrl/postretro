// Top-level scripting runtime: owns both subsystems and dispatches by file
// extension. See: context/lib/scripting.md

mod compile;
mod core;
mod data_script;
mod input_block;
mod loading_screen;
mod mod_init;
mod mod_init_exec;
#[cfg(debug_assertions)]
mod staged_preflight;
mod types;

pub use input_block::{ModInputBinding, ModInputBlock, ModInputCommand, ModInputGlyphs};
pub use loading_screen::ModLoading;
pub use types::{
    Frontend, MenuCamera, ModAttenuation, ModAttenuationCurve, ModAudioProfile, ModBloomProfile,
    ModBloomResolution, ModGhostAudio, ModGhostTransparency, ModManifestResult, ModMapEntry,
    ModMoverDefaults, ModReflectionProfile, ModReflectionStyle, ModRenderProfile, ReloadSummary,
    ScriptRuntime, ScriptRuntimeConfig, StagedManifestCommitOutcome,
};
pub(crate) use types::{
    check_entity_defense_references, validate_mod_manifest_id, validate_mod_manifest_version,
};

#[cfg(all(test, debug_assertions))]
#[path = "assist_validation_tests.rs"]
mod assist_validation_tests;
