// Shared fixtures and helpers for the scripting-runtime test suites. Topic
// modules below `use super::*` to pull these in alongside the runtime exports.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;

use crate::scripting::components::billboard_emitter::BillboardEmitterComponent;
use crate::scripting::ctx::ScriptCtx;
use crate::scripting::data_descriptors::{
    CrossingCondition, CrossingDescriptor, EntityTypeDescriptor, ModThemeTokens, NamedReaction,
    PrimitiveDescriptor, ReactionDescriptor, SequenceStep,
};
use crate::scripting::data_registry::{ScopedCrossing, ScopedReaction};
use crate::scripting::error::ScriptError;
use crate::scripting::primitives::register_all;
use crate::scripting::primitives_registry::PrimitiveRegistry;
use crate::scripting::provenance::{
    DescriptorComponentKind, DescriptorProvenance, DescriptorSpawnPath,
};
use crate::scripting::registry::{ComponentKind, RegistryError, Transform};
use crate::scripting::sequence::SequencedPrimitiveRegistry;
use crate::scripting::slot_table::{
    SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue, StoreDeclaration,
    StoreDeclarationSet,
};
use crate::scripting::staged_manifest::StagedManifest;
#[cfg(debug_assertions)]
use crate::scripting::staged_manifest::StagedManifestBuildLane;
use crate::scripting::staged_manifest::{StagedManifestBuildResult, StagedManifestBuildStatus};

use postretro_level_format::data_script::DataScriptSection;

mod compile;
mod data_script;
mod dependencies;
mod lifecycle;
mod mod_init_errors;
mod mod_init_manifest;
mod mod_init_stores;
mod mod_init_ui;
mod range_follow;
mod staged_commit;

fn runtime() -> (ScriptRuntime, ScriptCtx) {
    let ctx = ScriptCtx::new();
    let mut registry = PrimitiveRegistry::new();
    register_all(&mut registry, ctx.clone());
    let rt = ScriptRuntime::new(&registry, &ScriptRuntimeConfig::default(), &ctx).unwrap();
    (rt, ctx)
}

fn descriptor(name: &str) -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        canonical_name: Some(name.to_string()),
        default_weapon: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: None,
        mesh: None,
        health: None,
        ai: None,
    }
}

fn map_entry(id: &str) -> ModMapEntry {
    ModMapEntry {
        id: id.to_string(),
        path: format!("maps/{id}.prl"),
        name: id.to_string(),
        tags: vec!["campaign".to_string()],
    }
}

fn global_reaction(name: &str) -> ScopedReaction {
    ScopedReaction {
        reaction: NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "moveGeometry".to_string(),
                tag: Some("reactor".to_string()),
                on_complete: None,
                args: serde_json::Value::Object(Default::default()),
            }),
        },
        levels: vec!["campaign".to_string()],
    }
}

fn global_sequence_reaction(name: &str, primitive: &str, levels: &[&str]) -> ScopedReaction {
    ScopedReaction {
        reaction: NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Sequence(vec![SequenceStep {
                id: crate::scripting::registry::EntityId::from_raw(0x0001_0000),
                primitive: primitive.to_string(),
                args: serde_json::Value::Null,
            }]),
        },
        levels: levels.iter().map(|level| level.to_string()).collect(),
    }
}

fn global_crossing(slot: &str) -> ScopedCrossing {
    ScopedCrossing {
        crossing: CrossingDescriptor {
            slot: slot.to_string(),
            condition: CrossingCondition::Below { threshold: 0.25 },
            max: 100.0,
            fire: vec!["lowHealth".to_string()],
        },
        levels: vec!["campaign".to_string()],
    }
}

fn emitter_component(sprite: &str, rate: f32) -> BillboardEmitterComponent {
    BillboardEmitterComponent {
        rate,
        burst: Some(1),
        spread: 0.0,
        lifetime: 1.0,
        velocity: [0.0, 1.0, 0.0],
        buoyancy: 0.0,
        drag: 0.0,
        size_over_lifetime: [1.0].into(),
        opacity_over_lifetime: [1.0].into(),
        color: [1.0, 1.0, 1.0],
        sprite: sprite.to_string(),
        spin_rate: 0.0,
        spin_animation: None,
    }
}

fn emitter_descriptor(name: &str) -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        emitter: Some(emitter_component("smoke", 5.0)),
        ..descriptor(name)
    }
}

fn number_store_declarations(namespace: &str, default: f32) -> StoreDeclarationSet {
    let mut declarations = StoreDeclarationSet::default();
    declarations
        .add(StoreDeclaration {
            namespace: namespace.to_string(),
            records: vec![(
                "value".to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(default)),
                    range: None,
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: crate::scripting::slot_table::ReplicationScope::None,
                }),
            )],
        })
        .unwrap();
    declarations
}

#[cfg(debug_assertions)]
fn set_latest_staged_generation(rt: &mut ScriptRuntime, generation: u64) {
    rt.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(generation));
}

#[cfg(debug_assertions)]
fn built_result(
    generation: u64,
    mod_root: &Path,
    name: &str,
    entities: Vec<EntityTypeDescriptor>,
    dependency_paths: Vec<PathBuf>,
) -> StagedManifestBuildResult {
    built_result_with_maps(
        generation,
        mod_root,
        name,
        entities,
        Vec::new(),
        dependency_paths,
    )
}

#[cfg(debug_assertions)]
fn built_result_with_maps(
    generation: u64,
    mod_root: &Path,
    name: &str,
    entities: Vec<EntityTypeDescriptor>,
    maps: Vec<ModMapEntry>,
    dependency_paths: Vec<PathBuf>,
) -> StagedManifestBuildResult {
    StagedManifestBuildResult {
        generation,
        mod_root: mod_root.to_path_buf(),
        status: StagedManifestBuildStatus::Built(Box::new(StagedManifest {
            name: name.to_string(),
            entities,
            maps,
            reactions: Vec::new(),
            crossings: Vec::new(),
            ui_trees: Vec::new(),
            theme: ModThemeTokens::default(),
            frontend: None,
            store_declarations: StoreDeclarationSet::default(),
            dependency_paths,
        })),
        diagnostics: Vec::new(),
    }
}

/// Write `content` to a temp file under the target test directory and
/// return its path. Using `std::env::temp_dir` rather than an external
/// crate keeps the test dependency-free.
fn temp_script(name: &str, content: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    // Nonce by pid + counter to avoid cross-test collisions.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    p.push(format!(
        "postretro_runtime_test_{}_{}_{name}",
        std::process::id(),
        n,
    ));
    fs::write(&p, content).unwrap();
    p
}

fn data_section(source_path: &str, body: &str) -> DataScriptSection {
    DataScriptSection {
        compiled_bytes: body.as_bytes().to_vec(),
        source_path: source_path.to_string(),
    }
}

/// RAII wrapper: removes the temp directory when dropped, so an assertion
/// panic doesn't leak state under `std::env::temp_dir()`.
struct TempModRoot(std::path::PathBuf);

impl std::ops::Deref for TempModRoot {
    type Target = std::path::Path;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for TempModRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_mod_root(name: &str) -> TempModRoot {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!(
        "postretro_mod_init_test_{}_{}_{name}",
        std::process::id(),
        n,
    ));
    std::fs::create_dir_all(&p).unwrap();
    TempModRoot(p)
}

/// Copy `scripts-build` next to the current test executable so
/// `TsCompilerPath::detect()` finds it via the next-to-engine arm of the
/// cascade. Returns `false` only when the source binary genuinely cannot be
/// found — callers should skip the test gracefully in that case. Panics if
/// the source is found but the copy itself fails, because that indicates an
/// environment problem (bad permissions, full disk) that masks real failures.
#[cfg(debug_assertions)]
fn install_scripts_build_next_to_current_exe() -> bool {
    let Ok(current_exe) = std::env::current_exe() else {
        return false;
    };
    let Some(target_dir) = current_exe.parent() else {
        return false;
    };
    let name = if cfg!(windows) {
        "scripts-build.exe"
    } else {
        "scripts-build"
    };
    let dest = target_dir.join(name);
    if dest.is_file() {
        return true;
    }
    let source = ensure_scripts_build();
    // Guard against copy-onto-self: on Linux `fs::copy` of a file onto
    // itself truncates it. Canonicalize both paths before comparing so
    // symlinks and relative segments don't produce false mismatches.
    if let (Ok(cs), Ok(cd)) = (source.canonicalize(), dest.canonicalize()) {
        if cs == cd {
            return true;
        }
    }
    // Concurrent tests may race; if another test already dropped the file
    // in place between our `is_file` check and `copy`, the copy still
    // succeeds (overwrites). Any other failure is a real environment bug.
    std::fs::copy(&source, &dest).unwrap_or_else(|e| {
        panic!(
            "scripts-build found at {} but copy to {} failed: {e}",
            source.display(),
            dest.display()
        )
    });
    true
}

/// Locate the freshly-built `scripts-build` binary. Mirrors the same
/// helper in `watcher.rs` tests. CARGO_MANIFEST_DIR is always set by cargo.
fn ensure_scripts_build() -> std::path::PathBuf {
    fn scripts_build_binary() -> Option<std::path::PathBuf> {
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let name = if cfg!(windows) {
            "scripts-build.exe"
        } else {
            "scripts-build"
        };
        let mut dir: Option<&std::path::Path> = Some(manifest.as_path());
        while let Some(d) = dir {
            for profile in ["debug", "release"] {
                let candidate = d.join("target").join(profile).join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
            dir = d.parent();
        }
        None
    }

    if let Some(p) = scripts_build_binary() {
        return p;
    }
    let status = std::process::Command::new(env!("CARGO"))
        .args(["build", "-p", "postretro-script-compiler"])
        .status()
        .expect("cargo build scripts-build");
    assert!(status.success(), "failed to build scripts-build");
    scripts_build_binary().expect("scripts-build should exist after build")
}
