use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use postretro_entities::components::health::HealthComponent;
use postretro_entities::data_descriptors::{LifeStatesDescriptor, MeshDescriptor};
use postretro_entities::provenance::{
    DescriptorComponentKind, DescriptorProvenance, DescriptorSpawnPath,
};
use postretro_entities::{ScriptCtx, Transform};
use postretro_foundation::{LifeStateProfile, PlayerMovementComponent, enter_life_state};

use crate::primitives_registry::PrimitiveRegistry;
use crate::runtime::{ScriptRuntime, ScriptRuntimeConfig, StagedManifestCommitOutcome};
use crate::sequence::SequencedPrimitiveRegistry;
use crate::staged_manifest::{
    StagedManifestBuildConfig, StagedManifestBuildLane, StagedManifestBuildStatus,
    build_staged_manifest,
};

// Regression: retained presentation descriptors pushed an otherwise valid
// 255-name incoming manifest over the id limit after live refresh had applied.
#[test]
fn life_state_staged_commit_rejects_retained_name_overflow_before_any_mutation() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "postretro_life_state_union_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    fs::create_dir_all(&root).unwrap();
    let write = |count| {
        fs::write(root.join("start-script.js"), format!(r#"
            const states = {{}};
            for (let n = 0; n < {count}; n++) states['state' + n] = {{}};
            const store = defineStore('reload', {{ score: {{ type: 'number', default: 1 }} }});
            globalThis.__postretroModManifest = defineMod({{
                name: 'Reload Union', id: 'reload-union', version: '1', stores: [store],
                entities: [{{ canonicalName: 'new_player', components: {{
                    movement: {{
                        capsule: {{ radius: 0.35, halfHeight: 0.9, eyeHeight: 1.1 }},
                        ground: {{ speed: {{ walk: 7, run: 11, crouch: 3 }}, accel: 12, stepHeight: 0.35, maxSlope: 45 }},
                        air: {{ forwardSteer: 0.3, accel: 2, maxControlSpeed: 4, bunnyHop: true, jumps: 1, jumpVelocity: 5, jumpCeiling: 2 }},
                        fall: {{ terminalVelocity: 50 }}
                    }},
                    lifeStates: {{ initial: 'state0', states }}
                }} }}, {{ canonicalName: 'guard', components: {{ health: {{ max: 250 }} }} }}]
            }});
        "#)).unwrap();
    };
    write(255);
    let result = build_staged_manifest(&root, 1, &StagedManifestBuildConfig::default());
    let StagedManifestBuildStatus::Built(manifest) = &result.status else {
        panic!("valid incoming 255-name manifest must build: {result:?}");
    };
    let incoming = manifest.entities.clone();
    let mut old_player = incoming[0].clone();
    old_player.canonical_name = Some("old_player".into());
    old_player.life_states = Some(LifeStatesDescriptor {
        initial: "oldOnly".into(),
        states: BTreeMap::from([("oldOnly".into(), LifeStateProfile::default())]),
        rejoin: BTreeMap::new(),
        state_ids: BTreeMap::new(),
        table_revision: 0,
    });
    old_player.mesh = Some(MeshDescriptor {
        model: "models/old.gltf".into(),
        shadow_only: false,
        attachments: Default::default(),
        shadow_bias_scale: 1.0,
        animations: Default::default(),
        default_state: None,
        locomotion: None,
    });
    let mut old_guard = incoming[1].clone();
    old_guard.health.as_mut().unwrap().max = 100.0;
    let ctx = ScriptCtx::new();
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![old_player, old_guard]);
    let before_descriptors = ctx.data_registry.borrow().entities.clone();
    let before_names = ctx.data_registry.borrow().life_state_names.clone();
    let before_revision = ctx.data_registry.borrow().life_state_table_revision;
    let before_generation = ctx.data_registry.borrow().entity_types_generation();
    let (pawn, guard) = {
        let mut registry = ctx.registry.borrow_mut();
        let pawn = registry.spawn(Transform {
            position: glam::Vec3::new(0.0, 1.25, 0.0),
            ..Default::default()
        });
        let mut movement = PlayerMovementComponent::from_descriptor(
            before_descriptors[0].movement.as_ref().unwrap(),
        );
        let block = before_descriptors[0].life_states.as_ref().unwrap();
        let (id, profile) = block.initial_state();
        enter_life_state(&mut movement, id, profile, block.table_revision);
        movement.life_state_elapsed_ticks = 77;
        registry.set_component(pawn, movement).unwrap();
        let guard = registry.spawn(Transform::default());
        registry
            .set_component(
                guard,
                HealthComponent::from_descriptor(before_descriptors[1].health.as_ref().unwrap()),
            )
            .unwrap();
        for (entity, name, kind) in [
            (pawn, "old_player", DescriptorComponentKind::Movement),
            (guard, "guard", DescriptorComponentKind::Health),
        ] {
            registry
                .set_component(
                    entity,
                    DescriptorProvenance {
                        canonical_name: name.into(),
                        owned_components: BTreeSet::from([kind]),
                        map_overrides: BTreeSet::new(),
                        spawn_path: DescriptorSpawnPath::PlayerSpawn,
                    },
                )
                .unwrap();
        }
        (pawn, guard)
    };
    let before_movement = ctx
        .registry
        .borrow()
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .clone();
    let before_health = ctx
        .registry
        .borrow()
        .get_component::<HealthComponent>(guard)
        .unwrap()
        .clone();
    let before_transform = *ctx
        .registry
        .borrow()
        .get_component::<Transform>(pawn)
        .unwrap();
    let slots_before = ctx.slot_table.borrow().len();
    let primitives = PrimitiveRegistry::new();
    let mut runtime = ScriptRuntime::new(
        &primitives,
        &ScriptRuntimeConfig {
            skip_identity_enforcement: true,
            ..Default::default()
        },
        &ctx,
    )
    .unwrap();
    runtime.deferred_mesh_descriptors = Some(before_descriptors.clone());
    runtime.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(1));
    let outcome =
        runtime.commit_staged_manifest_result(&result, &ctx, &SequencedPrimitiveRegistry::new());
    let StagedManifestCommitOutcome::Rejected { reason, .. } = outcome else {
        panic!("effective 256-name union must reject: {outcome:?}");
    };
    assert!(
        reason.contains("entities[2].components.lifeStates.states.oldOnly"),
        "{reason}"
    );
    assert!(reason.contains("255"));
    assert!(reason.contains("retained presentation descriptors"));
    let data = ctx.data_registry.borrow();
    assert_eq!(data.entities, before_descriptors);
    assert_eq!(data.life_state_names, before_names);
    assert_eq!(data.life_state_table_revision, before_revision);
    assert_eq!(data.entity_types_generation(), before_generation);
    drop(data);
    assert_eq!(
        ctx.registry
            .borrow()
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap(),
        &before_movement
    );
    assert_eq!(
        ctx.registry
            .borrow()
            .get_component::<HealthComponent>(guard)
            .unwrap(),
        &before_health
    );
    assert_eq!(
        ctx.registry
            .borrow()
            .get_component::<Transform>(pawn)
            .unwrap(),
        &before_transform
    );
    assert_eq!(ctx.slot_table.borrow().len(), slots_before);
    assert!(ctx.slot_table.borrow().get("reload.score").is_none());
    assert_eq!(
        runtime.deferred_mesh_descriptors.as_ref(),
        Some(&before_descriptors)
    );
    assert!(runtime.active_mod_init_dependencies.is_none());
    assert!(runtime.committed_store_slots.is_empty());
    assert!(runtime.store_identity.is_none());
    assert!(runtime.committed_mod_identity.is_none());
    assert_eq!(runtime.committed_staged_manifest_generation(), None);

    // At the exact active-level bound the authored next-level snapshot remains
    // independent: promoting it drops the retained descriptor and its old name.
    write(254);
    let result = build_staged_manifest(&root, 2, &StagedManifestBuildConfig::default());
    runtime.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(2));
    assert!(matches!(
        runtime.commit_staged_manifest_result(&result, &ctx, &SequencedPrimitiveRegistry::new()),
        StagedManifestCommitOutcome::Committed { .. }
    ));
    assert_eq!(ctx.data_registry.borrow().life_state_names.len(), 255);
    assert!(runtime.install_deferred_mesh_descriptors(&ctx));
    assert_eq!(ctx.data_registry.borrow().life_state_names.len(), 254);
    assert!(
        !ctx.data_registry
            .borrow()
            .entities
            .iter()
            .any(|descriptor| descriptor.canonical_name.as_deref() == Some("old_player"))
    );
    fs::remove_dir_all(root).unwrap();
}
