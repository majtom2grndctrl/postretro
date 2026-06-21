// Staged manifest commit outcomes: stale, failed, replacement, store reconcile.

use std::collections::BTreeSet;

use super::*;

#[test]
#[cfg(debug_assertions)]
fn staged_manifest_commit_discards_stale_generation_without_mutating() {
    let (mut rt, ctx) = runtime();
    let dir = temp_mod_root("commit_stale");
    set_latest_staged_generation(&mut rt, 2);
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![descriptor("old")]);
    ctx.data_registry
        .borrow_mut()
        .replace_maps(vec![map_entry("old_map")]);
    ctx.data_registry
        .borrow_mut()
        .replace_global_reactions(vec![global_reaction("oldLoad")]);
    ctx.data_registry
        .borrow_mut()
        .replace_global_crossings(vec![global_crossing("old.health")]);

    let outcome = rt.commit_staged_manifest_result(
        &built_result(
            1,
            &dir,
            "Stale",
            vec![descriptor("new")],
            vec![dir.join("start-script.js")],
        ),
        &ctx,
        &SequencedPrimitiveRegistry::new(),
    );

    assert_eq!(
        outcome,
        StagedManifestCommitOutcome::DiscardedStale {
            generation: 1,
            latest_requested: Some(2),
        }
    );
    assert_eq!(
        ctx.data_registry.borrow().entities,
        vec![descriptor("old")],
        "stale result must leave committed descriptors active",
    );
    assert_eq!(
        ctx.data_registry.borrow().maps,
        vec![map_entry("old_map")],
        "stale result must leave committed map catalog active",
    );
    assert_eq!(
        ctx.data_registry.borrow().global_reactions,
        vec![global_reaction("oldLoad")],
        "stale result must leave committed global reactions active",
    );
    assert_eq!(
        ctx.data_registry.borrow().global_crossings,
        vec![global_crossing("old.health")],
        "stale result must leave committed global crossings active",
    );
}

#[test]
#[cfg(debug_assertions)]
fn staged_manifest_commit_failed_latest_preserves_snapshot() {
    let (mut rt, ctx) = runtime();
    let dir = temp_mod_root("commit_failed");
    set_latest_staged_generation(&mut rt, 3);
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![descriptor("old")]);
    ctx.data_registry
        .borrow_mut()
        .replace_maps(vec![map_entry("old_map")]);
    ctx.data_registry
        .borrow_mut()
        .replace_global_reactions(vec![global_reaction("oldLoad")]);
    ctx.data_registry
        .borrow_mut()
        .replace_global_crossings(vec![global_crossing("old.health")]);

    let outcome = rt.commit_staged_manifest_result(
        &StagedManifestBuildResult {
            generation: 3,
            mod_root: dir.to_path_buf(),
            status: StagedManifestBuildStatus::Failed,
            diagnostics: Vec::new(),
        },
        &ctx,
        &SequencedPrimitiveRegistry::new(),
    );

    assert_eq!(
        outcome,
        StagedManifestCommitOutcome::FailedBuild { generation: 3 }
    );
    assert_eq!(ctx.data_registry.borrow().entities, vec![descriptor("old")]);
    assert_eq!(ctx.data_registry.borrow().maps, vec![map_entry("old_map")]);
    assert_eq!(
        ctx.data_registry.borrow().global_reactions,
        vec![global_reaction("oldLoad")]
    );
    assert_eq!(
        ctx.data_registry.borrow().global_crossings,
        vec![global_crossing("old.health")]
    );
}

// Regression: a failed seed build left the dependency set None silently, so
// every later change-check returned false and hot reload was dead but the
// game booted fine. The failure is now logged; this pins the observable
// contract that no dependency set is installed.
#[test]
#[cfg(debug_assertions)]
fn failed_seed_install_leaves_no_active_dependency_set() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("seed_failed");

    rt.install_active_dependencies_from_staged_result(&StagedManifestBuildResult {
        generation: 0,
        mod_root: dir.to_path_buf(),
        status: StagedManifestBuildStatus::Failed,
        diagnostics: Vec::new(),
    });

    assert!(rt.active_mod_init_dependencies.is_none());
    assert!(!rt.changed_paths_affect_active_mod_init_manifest(&[dir.join("start-script.ts")]));
}

#[test]
#[cfg(debug_assertions)]
fn staged_manifest_commit_built_snapshot_replaces_whole_descriptor_manifest() {
    let (mut rt, ctx) = runtime();
    let dir = temp_mod_root("commit_replacement");
    let entry = dir.join("start-script.js");
    fs::write(&entry, "").unwrap();
    set_latest_staged_generation(&mut rt, 5);
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![descriptor("removed"), descriptor("kept_old_shape")]);
    ctx.data_registry
        .borrow_mut()
        .replace_maps(vec![map_entry("old_map")]);
    ctx.data_registry
        .borrow_mut()
        .replace_global_reactions(vec![global_reaction("oldLoad")]);
    ctx.data_registry
        .borrow_mut()
        .replace_global_crossings(vec![global_crossing("old.health")]);
    let replacement = descriptor("kept_new_shape");
    let replacement_map = map_entry("new_map");
    let replacement_reaction = global_reaction("newLoad");
    let replacement_crossing = global_crossing("new.health");
    let mut staged = built_result_with_maps(
        5,
        &dir,
        "Replacement",
        vec![replacement.clone()],
        vec![replacement_map.clone()],
        vec![entry.clone()],
    );
    let StagedManifestBuildStatus::Built(manifest) = &mut staged.status else {
        unreachable!()
    };
    manifest.reactions = vec![replacement_reaction.clone()];
    manifest.crossings = vec![replacement_crossing.clone()];

    let outcome =
        rt.commit_staged_manifest_result(&staged, &ctx, &SequencedPrimitiveRegistry::new());

    assert_eq!(
        outcome,
        StagedManifestCommitOutcome::Committed {
            generation: 5,
            descriptor_count: 1,
            applied_actions: 0,
            dropped_missing_targets: 0,
        }
    );
    assert_eq!(ctx.data_registry.borrow().entities, vec![replacement]);
    assert_eq!(ctx.data_registry.borrow().maps, vec![replacement_map]);
    assert_eq!(
        ctx.data_registry.borrow().global_reactions,
        vec![replacement_reaction]
    );
    assert_eq!(
        ctx.data_registry.borrow().global_crossings,
        vec![replacement_crossing]
    );
    assert!(rt.changed_paths_affect_active_mod_init_manifest(&[entry]));
}

#[test]
#[cfg(debug_assertions)]
fn staged_manifest_commit_filters_invalid_global_sequence_reactions() {
    let (mut rt, ctx) = runtime();
    let dir = temp_mod_root("commit_reaction_validation");
    let entry = dir.join("start-script.js");
    fs::write(&entry, "").unwrap();
    set_latest_staged_generation(&mut rt, 6);
    let mut sequence_registry = SequencedPrimitiveRegistry::new();
    sequence_registry.register("knownSequence", |_id, _args| Ok(()));

    let valid_primitive = global_reaction("primitiveGlobal");
    let valid_sequence =
        global_sequence_reaction("sequenceGlobal", "knownSequence", &["campaign", "boss"]);
    let invalid_sequence = global_sequence_reaction("invalidGlobal", "ghostSequence", &["boss"]);
    let mut staged = built_result(
        6,
        &dir,
        "ValidatedReactions",
        Vec::new(),
        vec![entry.clone()],
    );
    let StagedManifestBuildStatus::Built(manifest) = &mut staged.status else {
        unreachable!()
    };
    manifest.reactions = vec![
        valid_primitive.clone(),
        valid_sequence.clone(),
        invalid_sequence,
    ];

    let outcome = rt.commit_staged_manifest_result(&staged, &ctx, &sequence_registry);

    assert!(matches!(
        outcome,
        StagedManifestCommitOutcome::Committed { generation: 6, .. }
    ));
    assert_eq!(
        ctx.data_registry.borrow().global_reactions,
        vec![valid_primitive, valid_sequence],
    );
    assert!(rt.changed_paths_affect_active_mod_init_manifest(&[entry]));
}

#[test]
#[cfg(debug_assertions)]
fn staged_manifest_commit_preserves_compatible_store_values() {
    let (mut rt, ctx) = runtime();
    let dir = temp_mod_root("commit_store_compatible");
    let entry = dir.join("start-script.js");
    fs::write(&entry, "").unwrap();
    set_latest_staged_generation(&mut rt, 7);

    let declarations = number_store_declarations("session", 1.0);
    let plan = ctx
        .slot_table
        .borrow()
        .plan_reconcile(&declarations)
        .unwrap();
    ctx.slot_table.borrow_mut().apply_reconcile_plan(plan);
    ctx.slot_table
        .borrow_mut()
        .get_mut("session.value")
        .unwrap()
        .value = Some(SlotValue::Number(0.25));

    let mut result = built_result(
        7,
        &dir,
        "CompatibleStore",
        vec![descriptor("new")],
        vec![entry],
    );
    let StagedManifestBuildStatus::Built(manifest) = &mut result.status else {
        unreachable!()
    };
    manifest.store_declarations = number_store_declarations("session", 1.0);

    assert!(matches!(
        rt.commit_staged_manifest_result(&result, &ctx, &SequencedPrimitiveRegistry::new()),
        StagedManifestCommitOutcome::Committed { generation: 7, .. }
    ));
    assert_eq!(
        ctx.slot_table
            .borrow()
            .get("session.value")
            .and_then(|slot| slot.value.as_ref())
            .cloned(),
        Some(SlotValue::Number(0.25))
    );
}

#[test]
#[cfg(debug_assertions)]
fn staged_manifest_commit_rejects_schema_change_without_partial_commit() {
    let (mut rt, ctx) = runtime();
    let dir = temp_mod_root("commit_store_changed");
    let entry = dir.join("start-script.js");
    fs::write(&entry, "").unwrap();
    set_latest_staged_generation(&mut rt, 8);
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![descriptor("old")]);

    let existing = number_store_declarations("session", 1.0);
    let plan = ctx.slot_table.borrow().plan_reconcile(&existing).unwrap();
    ctx.slot_table.borrow_mut().apply_reconcile_plan(plan);

    let mut changed = number_store_declarations("session", 2.0);
    changed
        .add(
            number_store_declarations("new_namespace", 1.0)
                .iter()
                .next()
                .unwrap()
                .clone(),
        )
        .unwrap();
    let mut result = built_result(
        8,
        &dir,
        "ChangedStore",
        vec![descriptor("new")],
        vec![entry],
    );
    let StagedManifestBuildStatus::Built(manifest) = &mut result.status else {
        unreachable!()
    };
    manifest.store_declarations = changed;

    assert!(matches!(
        rt.commit_staged_manifest_result(&result, &ctx, &SequencedPrimitiveRegistry::new()),
        StagedManifestCommitOutcome::Rejected { generation: 8, .. }
    ));
    assert_eq!(ctx.data_registry.borrow().entities, vec![descriptor("old")]);
    assert!(ctx.slot_table.borrow().get("new_namespace.value").is_none());
    assert_eq!(
        ctx.slot_table
            .borrow()
            .get("session.value")
            .and_then(|slot| slot.value.as_ref()),
        Some(&SlotValue::Number(1.0))
    );
}

#[test]
#[cfg(debug_assertions)]
fn staged_manifest_commit_no_start_script_replaces_descriptors_and_applies_removal() {
    let (mut rt, ctx) = runtime();
    let dir = temp_mod_root("commit_no_start");
    fs::create_dir_all(dir.join("actors")).unwrap();
    set_latest_staged_generation(&mut rt, 4);
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![emitter_descriptor("smoke")]);
    let id = {
        let mut registry = ctx.registry.borrow_mut();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(id, emitter_component("smoke", 5.0))
            .unwrap();
        registry
            .set_component(
                id,
                DescriptorProvenance {
                    canonical_name: "smoke".to_string(),
                    owned_components: BTreeSet::from([DescriptorComponentKind::Emitter]),
                    map_overrides: BTreeSet::new(),
                    spawn_path: DescriptorSpawnPath::MapPlacement,
                },
            )
            .unwrap();
        id
    };

    let outcome = rt.commit_staged_manifest_result(
        &StagedManifestBuildResult {
            generation: 4,
            mod_root: dir.to_path_buf(),
            status: StagedManifestBuildStatus::NoStartScript,
            diagnostics: Vec::new(),
        },
        &ctx,
        &SequencedPrimitiveRegistry::new(),
    );

    assert_eq!(
        outcome,
        StagedManifestCommitOutcome::Committed {
            generation: 4,
            descriptor_count: 0,
            applied_actions: 1,
            dropped_missing_targets: 0,
        }
    );
    assert!(ctx.data_registry.borrow().entities.is_empty());
    assert!(matches!(
        ctx.registry
            .borrow()
            .get_component::<BillboardEmitterComponent>(id),
        Err(RegistryError::ComponentNotFound {
            kind: ComponentKind::BillboardEmitter,
            ..
        })
    ));
    assert!(rt.changed_paths_affect_active_mod_init_manifest(&[dir.join("start-script.ts")]));
    assert!(!rt.changed_paths_affect_active_mod_init_manifest(&[dir.join("actors/smoke.ts")]));
}
