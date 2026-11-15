//! Mover probe channel through the binary: id 57's bytes across cold builds and worker counts, and warm builds over the
//! per-light direct memos and the per-pose bounce memo: a mover edit re-bakes only the
//! lights reaching its old or new pose, no id-41 or id-45 entry, and only that mover's moved poses; a static-light edit only that light's direct
//! entries but every mover's bounce states; an animated-light edit only its id-45 entries; a
//! retexture no direct entry but every bounce state; and every warm PRL equals the edited map's
//! cold PRL. Over one cache, edits and their revert build warm equal to cold, and the revert
//! hits every memo, with id 57's payloads resident or streamed with id 50's cells. A ramp edit
//! hits every bounce memo and re-decides which mid-pose states skip, warm equal to cold.
//! See: context/lib/build_pipeline.md §Build Cache, §Compiler pipeline step 12 (Mover probe channel)

mod mover_cache_support;
mod prl_build_support;

use mover_cache_support::*;
use postretro_level_format::SectionId;
use postretro_level_format::mover_probe_channel::{
    MoverChannelBlock, MoverLightNamespace, MoverPayloadPlacement, MoverProbeChannelSection,
};

// Direct half: id 57 is byte-identical across two cold builds
// and across one and four workers; every namespace carries entries, and the
// runtime loader accepts the section.
#[test]
fn the_section_is_identical_across_cold_builds_and_worker_counts() {
    let scene = Scene::default();
    let first = build(&scene, CacheMode::Cold, 4);
    let second = build(&scene, CacheMode::Cold, 4);
    let one_worker = build(&scene, CacheMode::Cold, 1);
    let bytes = section(&first.prl, SectionId::MoverProbeChannel).expect("the movers gate lights");
    assert_eq!(
        section(&second.prl, SectionId::MoverProbeChannel).as_ref(),
        Some(&bytes)
    );
    assert_eq!(
        section(&one_worker.prl, SectionId::MoverProbeChannel).as_ref(),
        Some(&bytes)
    );
    let channel = MoverProbeChannelSection::from_bytes(&bytes).expect("decodes");
    assert!(
        !channel.bounce_entries.is_empty() && !channel.depth_records.is_empty(),
        "the section carries bounce and depth blocks too"
    );
    for namespace in [
        MoverLightNamespace::StaticLight,
        MoverLightNamespace::AnimatedBaked,
        MoverLightNamespace::BakeOnlyStaticLight,
    ] {
        assert!(
            channel
                .direct_records
                .iter()
                .any(|record| record.namespace == namespace),
            "a record in {namespace:?}"
        );
    }
    assert!(
        channel
            .direct_records
            .iter()
            .filter(|record| record.namespace == MoverLightNamespace::BakeOnlyStaticLight)
            .all(|record| record.light_index == 0 && record.carries_own_tile),
        "the one bake-only light is roster index 0 and carries its own tile"
    );
    let world = postretro_level_loader::load_prl(first.prl.to_str().expect("UTF-8 path"))
        .expect("the runtime loads the packed channel");
    assert_eq!(world.mover_probe_channel(), Some(&channel));
}

// Direct half: a map with no mover packs no id 57.
#[test]
fn a_map_without_movers_packs_no_section() {
    let built = build(
        &Scene {
            movers: false,
            ..Scene::default()
        },
        CacheMode::Cold,
        JOBS,
    );
    assert!(section(&built.prl, SectionId::MoverProbeChannel).is_none());
}

// Direct half: moving the plate's last waypoint traces nothing
// for ids 34, 35, 41 and 45 and packs the edited map's cold bytes. It moves
// every mid pose and changes the plate's pose count (4.88 m of path, six
// poses, to 3.25 m, five), so every light's key, which folds each pose,
// re-bakes; the first-pose light included.
#[test]
fn moving_a_movers_last_waypoint_rebakes_only_the_lights_reaching_its_poses() {
    let warm = warm_equals_cold(&Scene {
        plate_last: [0, 128, 104],
        ..Scene::default()
    });
    assert_logged(&warm, "[cache] sh_base_volume hit");
    assert_logged(&warm, "[cache] direct_sh_volume hit");
    assert_all_hit(&warm, ENTITY_SHADOW_DIRECT);
    assert_all_hit(&warm, ANIMATED_DIRECT);
    assert_eq!(
        miss_share(&warm),
        (base_probed(), base_probed()),
        "every light re-bakes: every mid pose moved\n{warm}"
    );
    // Bounce half: only the plate's moved poses re-bake, from the cached
    // layers; its first terminus, posed where it was, hits.
    assert_eq!(bounce_share(&warm), (1, MOVERS), "{warm}");
    assert_eq!(
        mover_memo(&warm, "plate"),
        "miss: first terminus hit, last miss; re-bakes pose(s) 1, 2, 3, 4 of 5",
        "{warm}"
    );
    assert!(bounce_memo_line(&warm, "shutter", "hit"), "{warm}");
}

// Direct half: editing one static light's colour re-bakes no
// other light's entries, in id 57 or id 41, and no id-45 entry.
#[test]
fn editing_one_static_lights_colour_rebakes_only_its_entries() {
    let warm = warm_equals_cold(&Scene {
        lit_color: "255 128 64",
        ..Scene::default()
    });
    assert_eq!(miss_share(&warm), (1, base_probed()), "{warm}");
    assert_eq!(
        bounce_share(&warm),
        (MOVERS, MOVERS),
        "every mover's bounce re-bakes"
    );
    assert!(
        base_probed_by(ENTITY_SHADOW_DIRECT) > 1,
        "the fixture selects more than one entity-shadow light"
    );
    assert_eq!(
        memo_miss_share(&warm, ENTITY_SHADOW_DIRECT),
        (1, base_probed_by(ENTITY_SHADOW_DIRECT)),
        "{warm}"
    );
    assert_all_hit(&warm, ANIMATED_DIRECT);
}

// Editing the animated light's reach re-bakes its id-45 entries and no id-41
// entry.
#[test]
fn editing_the_animated_light_rebakes_only_its_animated_direct_entries() {
    let warm = warm_equals_cold(&Scene {
        animated_range: 240,
        ..Scene::default()
    });
    assert_eq!(
        memo_miss_share(&warm, ANIMATED_DIRECT),
        (1, base_probed_by(ANIMATED_DIRECT)),
        "{warm}"
    );
    assert_all_hit(&warm, ENTITY_SHADOW_DIRECT);
}

// Direct half: retexturing a static surface moves nothing the
// direct half, id 41 or id 45 traces, so every light hits.
#[test]
fn retexturing_a_static_surface_rebakes_no_direct_entry() {
    let warm = warm_equals_cold(&Scene {
        floor_top_scale: 2,
        ..Scene::default()
    });
    for stage in [MOVER_DIRECT, ENTITY_SHADOW_DIRECT, ANIMATED_DIRECT] {
        assert_all_hit(&warm, stage);
    }
}

// Direct half: duplicating a static light, appended so no
// other light's seed shifts, re-bakes only the duplicate's entries, in id 57
// and id 41.
#[test]
fn duplicating_a_static_light_rebakes_only_the_duplicate() {
    let warm = warm_equals_cold(&Scene {
        appended_light: true,
        ..Scene::default()
    });
    assert_eq!(miss_share(&warm), (1, base_probed() + 1), "{warm}");
    assert_eq!(
        memo_miss_share(&warm, ENTITY_SHADOW_DIRECT),
        (1, base_probed_by(ENTITY_SHADOW_DIRECT) + 1),
        "{warm}"
    );
    assert_all_hit(&warm, ANIMATED_DIRECT);
    assert_eq!(
        bounce_share(&warm),
        (MOVERS, MOVERS),
        "every mover's bounce re-bakes"
    );
}

// Bounce half: retexturing a static surface changes the
// bounce albedo id 34 reads, so every mover's bounce states re-bake, while
// no direct entry does.
#[test]
fn retexturing_a_static_surface_rebakes_every_movers_bounce() {
    let warm = warm_equals_cold(&Scene {
        floor_top: RETEXTURE,
        ..Scene::default()
    });
    assert_logged(&warm, "[cache] sh_base_volume miss");
    assert_eq!(bounce_share(&warm), (MOVERS, MOVERS), "{warm}");
    for stage in [MOVER_DIRECT, ENTITY_SHADOW_DIRECT, ANIMATED_DIRECT] {
        assert_all_hit(&warm, stage);
    }
}

/// The plate's skip line: its kept and skipped (probe, mid pose) states.
fn plate_skips(logs: &str) -> &str {
    logs.lines()
        .find(|line| line.contains("probe channel skipping: mover `kinematic_mover 'plate'`"))
        .unwrap_or_else(|| panic!("a skip line for the plate\n{logs}"))
}

// Skipping: the plate bakes mid poses, and its redundant mid-pose states
// skip after every memo read, judged after its ramp. Authoring ramp (0.6,
// 0.9) changes no memo key, so every bounce memo hits, yet the skips are
// re-decided (poses the ramp squeezes out are never read), and the warm PRL
// equals the edited map's cold PRL byte for byte.
#[test]
fn a_ramp_edit_re_decides_skips_over_the_memoized_states_and_warm_equals_cold() {
    let ramped = Scene {
        plate_ramp: Some(("0.6", "0.9")),
        ..Scene::default()
    };
    let warm = warm_equals_cold(&ramped);
    assert_eq!(bounce_share(&warm), (0, MOVERS), "{warm}");
    let (before, after) = (plate_skips(&base().1.logs), plate_skips(&warm));
    let counts = |line: &str| {
        let from = line.find("bounce keeps").expect("the skip counts");
        line[from..line.rfind("; ").expect("the timing")].to_string()
    };
    assert_ne!(
        counts(before),
        counts(after),
        "the ramp re-decides the plate's skips"
    );
    assert!(
        !after.contains("; 0 of the skipped never read past the ramp"),
        "the ramp squeezes stored poses out: {after}"
    );
}

// Over one cache, a mover edit, then a static-light edit
// on top of it, then reverting both, each build warm equal to cold; the
// revert hits every direct, bounce and id-34 memo.
#[test]
fn edits_and_their_revert_over_one_cache_build_warm_equal_to_cold() {
    let cache = tempfile::tempdir().expect("create edit-order cache");
    copy_dir(&base().0, cache.path());
    let moved = Scene {
        plate_last: [0, 128, 104],
        ..Scene::default()
    };
    let warm = warm_over(&moved, cache.path());
    assert_eq!(bounce_share(&warm), (1, MOVERS), "{warm}");
    let relit = Scene {
        lit_color: "255 128 64",
        ..moved
    };
    let warm = warm_over(&relit, cache.path());
    assert_eq!(bounce_share(&warm), (MOVERS, MOVERS), "{warm}");
    let warm = warm_over(&Scene::default(), cache.path());
    assert_logged(&warm, "[cache] sh_base_volume hit");
    assert_logged(&warm, "[cache] direct_sh_volume hit");
    for stage in [MOVER_DIRECT, ENTITY_SHADOW_DIRECT, ANIMATED_DIRECT] {
        assert_all_hit(&warm, stage);
    }
    assert_eq!(
        bounce_share(&warm),
        (0, MOVERS),
        "the revert hits every bounce memo\n{warm}"
    );
}

// With streamed payloads: with the resident bar at zero
// both of id 57's payloads stream with id 50's cells; over a cache that build
// filled, a mover edit, then its revert, each build warm equal to cold, and the
// revert hits every direct and bounce memo.
#[test]
fn a_streamed_channel_builds_warm_equal_to_cold() {
    const STREAM_BOTH: &[&str] = &["--mover-payload-resident-bar", "0"];
    let cache = tempfile::tempdir().expect("create streamed cache");
    let filled = build_with(
        &Scene::default(),
        CacheMode::Fill(cache.path()),
        JOBS,
        STREAM_BOTH,
    );
    let channel = MoverProbeChannelSection::from_bytes(
        &section(&filled.prl, SectionId::MoverProbeChannel).expect("id 57"),
    )
    .expect("id 57 decodes");
    for block in MoverChannelBlock::BOTH {
        assert_eq!(
            channel.placement(block),
            MoverPayloadPlacement::Streamed,
            "{block}"
        );
    }
    let moved = Scene {
        plate_last: [0, 128, 104],
        ..Scene::default()
    };
    let warm = warm_over_with(&moved, cache.path(), STREAM_BOTH);
    assert_eq!(bounce_share(&warm), (1, MOVERS), "{warm}");
    let warm = warm_over_with(&Scene::default(), cache.path(), STREAM_BOTH);
    for stage in [MOVER_DIRECT, ENTITY_SHADOW_DIRECT, ANIMATED_DIRECT] {
        assert_all_hit(&warm, stage);
    }
    assert_eq!(bounce_share(&warm), (0, MOVERS), "{warm}");
}
