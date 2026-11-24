// Tests: hot reload of a character's weapon-mount table. The load rules check
// the incoming descriptors, not the deferred active set (E21 row L4), and a
// table edit waits for the next level load under the mesh deferral (row L6).
// See: context/lib/entity_model.md §2 (MeshComponent)

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use log::Level;
use postretro_entities::data_descriptors::MountDescriptor;
use postretro_test_log_capture::LogCapture;

use super::*;
use crate::data_descriptors::{EntityTypeDescriptor, MeshDescriptor};
use crate::staged_manifest::build_staged_manifest;

const MESH_DEFERRAL_WARNING: &str = "[Scripting] staged descriptor refresh deferred components.mesh for `player`; active mesh bindings and model uploads remain in use until the next level load";

const WEAPON: &str = r#"{ damage: 1, range: 10, resolution: "hitscan", primary: { trigger: "press", recoveryMs: 100, steps: [{ kind: "shot" }] } }"#;

struct TempModRoot(PathBuf);

impl TempModRoot {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "postretro_weapon_mount_reload_{}_{}_{name}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&path).expect("temporary mod root should be created");
        Self(path)
    }
}

impl Drop for TempModRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A mod with one weapon named `weapon` and a player whose table mounts
/// `bake` for it.
fn write_mod(mod_root: &Path, weapon: &str, bake: &str) {
    fs::write(
        mod_root.join("start-script.js"),
        format!(
            r#"
                const rifle = defineEntity({{ canonicalName: "{weapon}", components: {{ weapon: {WEAPON} }} }});
                globalThis.__postretroModManifest = defineMod({{
                    name: "Mounts", id: "mounts", version: "1",
                    entities: [rifle, defineEntity({{ canonicalName: "player", components: {{
                        mesh: {{
                            model: "models/exo_red/model.gltf",
                            mounts: [{{ item: rifle, attachments: {{ hand_r: "{bake}" }} }}],
                        }},
                        inventory: {{ loadout: [rifle] }},
                    }} }})],
                }});
            "#
        ),
    )
    .expect("staged mod manifest should be written");
}

fn player_mounts(descriptors: &[EntityTypeDescriptor]) -> Option<Vec<MountDescriptor>> {
    descriptors
        .iter()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some("player"))
        .expect("the player registers")
        .mesh
        .as_ref()
        .and_then(|mesh| mesh.mounts.clone())
}

fn rifle_mount(weapon: &str, bake: &str) -> Option<Vec<MountDescriptor>> {
    Some(vec![MountDescriptor {
        item: weapon.to_string(),
        attachments: [("hand_r".to_string(), bake.to_string())]
            .into_iter()
            .collect(),
    }])
}

// L4: renaming a weapon and its table entry in one reload commits. The table
// is checked against the incoming descriptors; the active level keeps its
// deferred table, and the next level load gets the renamed one.
#[test]
fn a_reload_renaming_a_weapon_and_its_mount_entry_commits() {
    const BAKE: &str = "models/cyberpunk_weapons/rifle/exo_red.gltf";
    let mod_root = TempModRoot::new("rename");
    write_mod(&mod_root.0, "old_rifle", BAKE);
    let installed = build_staged_manifest(&mod_root.0, 1, &StagedManifestBuildConfig::default());
    let StagedManifestBuildStatus::Built(installed) = installed.status else {
        panic!("the installed mod must build: {:?}", installed.diagnostics);
    };
    let ctx = ScriptCtx::new();
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(installed.entities.clone());

    write_mod(&mod_root.0, "new_rifle", BAKE);
    let renamed = build_staged_manifest(&mod_root.0, 2, &StagedManifestBuildConfig::default());
    let primitives = PrimitiveRegistry::new();
    let mut runtime =
        ScriptRuntime::new(&primitives, &ScriptRuntimeConfig::default(), &ctx).unwrap();
    runtime.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(2));
    let capture = LogCapture::start();
    let outcome =
        runtime.commit_staged_manifest_result(&renamed, &ctx, &SequencedPrimitiveRegistry::new());
    assert!(
        matches!(
            outcome,
            StagedManifestCommitOutcome::Committed { generation: 2, .. }
        ),
        "{outcome:?}"
    );

    let active = ctx.data_registry.borrow().entities.clone();
    assert!(
        active
            .iter()
            .any(|descriptor| descriptor.canonical_name.as_deref() == Some("new_rifle")),
        "the renamed weapon is registered"
    );
    assert_eq!(
        player_mounts(&active),
        rifle_mount("old_rifle", BAKE),
        "the active level keeps the deferred table"
    );
    assert_eq!(
        player_mounts(
            runtime
                .deferred_mesh_descriptors()
                .expect("a snapshot is deferred")
        ),
        rifle_mount("new_rifle", BAKE),
        "the next level load gets the renamed entry"
    );
    capture.assert_logged_once(Level::Warn, MESH_DEFERRAL_WARNING);
}

// L6: an edit to the table alone is deferred with the existing
// `components.mesh` warning; the active descriptor keeps the pre-edit table.
#[test]
fn a_staged_weapon_mounts_edit_is_deferred_with_the_mesh_warning() {
    let descriptor = |bake: &str| EntityTypeDescriptor {
        life_states: None,
        ghost: false,
        faction: None,
        tolerance: None,
        canonical_name: Some("player".to_string()),
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: None,
        touchable: None,
        mesh: Some(MeshDescriptor {
            model: "models/exo_red/model.gltf".to_string(),
            shadow_only: true,
            attachments: Default::default(),
            shadow_bias_scale: 1.0,
            animations: Default::default(),
            default_state: None,
            locomotion: None,
            mounts: rifle_mount("reference_rifle", bake),
        }),
        health: None,
        behavior: None,
    };
    let old = vec![descriptor("models/rifle/exo_red.gltf")];
    let mut next = vec![descriptor("models/rifle/exo_red_v2.gltf")];

    let capture = LogCapture::start();
    assert!(defer_visual_asset_descriptor_refreshes(&old, &mut next));
    capture.assert_logged_once(Level::Warn, MESH_DEFERRAL_WARNING);
    assert_eq!(
        player_mounts(&next),
        rifle_mount("reference_rifle", "models/rifle/exo_red.gltf"),
        "the active level keeps the pre-edit entry"
    );
}
