// GPU proofs of pair atomicity, the drain contract's generations, and the no-op drain.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use super::*;

// A pair missing a shadowmask group, or carrying a short
// one, fails whole — none of its planes upload and its entry never turns
// resident — while a well-formed pair in the same drain installs.
#[test]
fn a_pair_missing_a_shadowmask_group_fails_whole_and_its_entry_stays_non_resident() {
    let Some(mut stream) = Stream::new(
        "a_pair_missing_a_shadowmask_group_fails_whole_and_its_entry_stays_non_resident",
        &[(8, 8), (8, 8), (8, 8)],
        1,
    ) else {
        return;
    };
    let whole = stream.prepared(0);
    let whole_bytes = whole.payload.irradiance.len()
        + whole.payload.direction.len()
        + whole
            .payload
            .shadowmask
            .as_ref()
            .map_or(0, |[a, b]| a.len() + b.len());
    let mut no_groups = stream.prepared(1);
    no_groups.payload.shadowmask = None;
    let mut short_group = stream.prepared(2);
    short_group
        .payload
        .shadowmask
        .as_mut()
        .expect("fixture keeps id 42")[1]
        .truncate(8);

    let targets = vec![mandatory(0), mandatory(1), mandatory(2)];
    let outcome = stream.drain(stream.reset(targets, vec![whole, no_groups, short_group]));
    assert_eq!(outcome.installed, vec![0]);
    assert_eq!(outcome.failed, vec![1, 2]);
    assert!(outcome.refused.is_empty());
    let counters = stream.state.counters();
    assert_eq!(counters.failed_installs, 2);
    assert_eq!(counters.last_drain_uploads, 1);
    assert_eq!(counters.last_drain_install_bytes, whole_bytes as u64);
    assert_eq!(
        counters.last_drain_table_writes, 1,
        "only block 0's entry changed"
    );
    for block in [1, 2] {
        assert!(matches!(
            stream.state.model().table_entry(block),
            BlockTableEntry::Missing { .. }
        ));
    }
    stream.assert_samples_own_texels();
}

// Evicting A and placing B in A's region happen in one submission; the
// table read back shows A non-resident and B on its own texels, never A
// through B's texels. Each plane fills a mask layer, so the mask pool holds two.
#[test]
fn an_evicted_block_turns_non_resident_in_the_submission_that_fills_its_region() {
    let Some(mut stream) = Stream::sized(
        "an_evicted_block_turns_non_resident_in_the_submission_that_fills_its_region",
        &[(WIDE, 1100), (WIDE, 1100)],
        PerPool {
            lightmap: 1,
            mask: 2,
        },
        u32::MAX,
    ) else {
        return;
    };
    stream.drain(stream.reset(vec![band(0, 7)], vec![stream.prepared(0)]));
    let a_region = stream.state.model().placement(0).expect("A resident");
    let submissions = stream.state.counters().submissions;

    let outcome = stream.drain(stream.delta(vec![mandatory(1)], vec![], vec![stream.prepared(1)]));
    assert_eq!(outcome.installed, vec![1]);
    assert_eq!(outcome.evicted, vec![0]);
    assert_eq!(
        stream.state.model().placement(1),
        Some(a_region),
        "B lands in A's region"
    );
    assert_eq!(stream.state.counters().submissions, submissions + 1);
    assert_eq!(stream.state.counters().last_drain_table_writes, 2);
    stream.assert_samples_own_texels();
}

// A batch from another generation, or with another level's identity, is
// rejected before anything changes; the next valid batch still drains.
#[test]
fn a_batch_from_another_generation_is_rejected_without_mutating_state() {
    let Some(mut stream) = Stream::new(
        "a_batch_from_another_generation_is_rejected_without_mutating_state",
        &[(8, 8), (8, 8)],
        1,
    ) else {
        return;
    };
    stream.generation = 2;
    stream.drain(stream.reset(vec![mandatory(0)], vec![stream.prepared(0)]));
    let table = stream.table();
    let counters = stream.state.counters();

    stream.generation = 1;
    let stale = stream.reset(vec![mandatory(1)], vec![stream.prepared(1)]);
    assert_eq!(
        stream.try_drain(stale).unwrap_err(),
        LightmapResidencyDrainError::StaleGeneration {
            current: 2,
            received: 1
        }
    );
    stream.generation = 3;
    let unreset = stream.delta(vec![mandatory(1)], vec![], vec![stream.prepared(1)]);
    assert_eq!(
        stream.try_drain(unreset).unwrap_err(),
        LightmapResidencyDrainError::GenerationResetRequired {
            current: 2,
            received: 3
        }
    );
    stream.generation = 2;
    let mut foreign = stream.delta(vec![mandatory(1)], vec![], vec![stream.prepared(1)]);
    foreign.content_tag = [9; 32];
    assert!(matches!(
        stream.try_drain(foreign),
        Err(LightmapResidencyDrainError::InvalidBatch(_))
    ));
    let mut mixed = stream.delta(vec![mandatory(1)], vec![], vec![stream.prepared(1)]);
    mixed.ready[0].generation = 1;
    assert!(matches!(
        stream.try_drain(mixed),
        Err(LightmapResidencyDrainError::InvalidBatch(_))
    ));

    assert_eq!(
        stream.state.counters(),
        counters,
        "no rejected batch counted"
    );
    assert!(!stream.state.model().is_resident(1));
    assert_eq!(stream.table(), table, "no rejected batch wrote the table");

    stream.generation = 3;
    let outcome =
        stream.drain(stream.reset(vec![mandatory(0), mandatory(1)], vec![stream.prepared(1)]));
    assert_eq!(outcome.installed, vec![1]);
    stream.assert_samples_own_texels();
}

// A new generation mid-level comes from a fresh controller that holds nothing
// resident: its first drain frees every placement in the same submission,
// reports none of them evicted, and installs only what its batch carries.
#[test]
fn a_new_generation_starts_from_an_empty_residency_without_reporting_evictions() {
    let Some(mut stream) = Stream::new(
        "a_new_generation_starts_from_an_empty_residency_without_reporting_evictions",
        &[(8, 8), (8, 8), (8, 8)],
        1,
    ) else {
        return;
    };
    stream.drain(stream.reset(
        vec![mandatory(0), mandatory(1)],
        vec![stream.prepared(0), stream.prepared(1)],
    ));
    let before = stream.state.counters();

    stream.generation = 2;
    let outcome =
        stream.drain(stream.reset(vec![mandatory(1), mandatory(2)], vec![stream.prepared(2)]));
    assert_eq!(outcome.installed, vec![2]);
    assert!(outcome.evicted.is_empty(), "{:?}", outcome.evicted);
    assert!(!stream.state.model().is_resident(0));
    assert!(
        !stream.state.model().is_resident(1),
        "the new controller never read block 1"
    );
    let counters = stream.state.counters();
    assert_eq!(counters.last_drain_table_writes, 3);
    assert_eq!(counters.evictions, before.evictions);
    assert_eq!(
        counters.pool_texture_sets,
        PerPool::both(1),
        "the textures stand"
    );
    stream.assert_samples_own_texels();
}

// A drain that changes nothing issues no table write,
// no upload and no submission; a reclassification writes nothing either, and
// a removal writes exactly its one entry.
#[test]
fn a_no_op_drain_issues_no_table_writes_no_uploads_and_no_submission() {
    let Some(mut stream) = Stream::new(
        "a_no_op_drain_issues_no_table_writes_no_uploads_and_no_submission",
        &[(8, 8), (8, 8), (8, 8)],
        1,
    ) else {
        return;
    };
    stream.drain(stream.reset(
        vec![mandatory(0), mandatory(1)],
        vec![stream.prepared(0), stream.prepared(1)],
    ));
    let installed = stream.state.counters();
    assert_eq!(installed.last_drain_table_writes, 2);
    assert_eq!(
        installed.table_entries_written,
        4 + 2,
        "the install table, then two"
    );

    for batch in [
        stream.delta(vec![], vec![], vec![]),
        stream.delta(vec![band(1, 3)], vec![], vec![]),
    ] {
        stream.drain(batch);
        let counters = stream.state.counters();
        assert_eq!(counters.last_drain_table_writes, 0);
        assert_eq!(counters.last_drain_uploads, 0);
        assert_eq!(counters.last_drain_install_bytes, 0);
        assert_eq!(
            counters.submissions, installed.submissions,
            "nothing submitted"
        );
        assert_eq!(
            counters.table_entries_written,
            installed.table_entries_written
        );
    }
    stream.drain(stream.delta(vec![], vec![1], vec![]));
    let counters = stream.state.counters();
    assert_eq!(counters.last_drain_table_writes, 1);
    assert_eq!(counters.submissions, installed.submissions + 1);
    stream.assert_samples_own_texels();
}
