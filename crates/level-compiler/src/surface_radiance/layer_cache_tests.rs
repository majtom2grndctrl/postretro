// Per-light layer cache tests: content keys, edits, lost entries, stops, eviction and permits.
// See: context/lib/build_pipeline.md §Build Cache

use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;

use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::bounce::DEFAULT_BOUNCE_FILL;
use super::layer::SH_PATCH_LAYER_STAGE_ID;
use super::patches::PatchSet;
use super::reach::PatchReachIndex;
use super::sum::PatchRadiance;
use super::test_scene::{Scene, animated, annex_light, entries, point, room_lights};
use super::volume::SH_BASE_VOLUME_STAGE_ID;
use crate::bake_control::BakeControl;
use crate::bake_counters::{BakeCounts, counted};
use crate::cache::StageCache;
use crate::governor::{Governor, permits_held_on_this_thread};
use crate::map_data::MapLight;
use crate::reporter::StageProgress;
use crate::sh_bake::static_light_refs;
use crate::texel_footprint::NO_EMBEDDED_EMITTERS;

fn same_id_34(a: &OctahedralShVolumeSection, b: &OctahedralShVolumeSection, context: &str) {
    assert_eq!(a.compact_atlas, b.compact_atlas, "{context}: probe tiles");
    assert_eq!(a.probes, b.probes, "{context}: depth moments");
}

fn shaded_and_cached(counts: &BakeCounts) -> (u64, u64) {
    (counts.layers_shaded, counts.layers_cached)
}

/// A fresh handle on the cache in `dir`, so its access tally is one build's.
fn open(dir: &Path) -> StageCache {
    StageCache::new(dir).expect("cache dir")
}

// After one light's colour edit only its layer
// re-shades, and every shadow ray the warm build traces is that layer's:
// the re-gather traces none. A reorder or no edit hits the gathered volume
// and reads no layer; with that entry gone, it hits every layer and traces
// no shadow ray at all.
#[test]
fn one_light_edit_reshades_only_its_layer_and_the_regather_traces_no_shadow_ray() {
    let scene = Scene::new();
    let lights = room_lights();
    let dir = tempfile::tempdir().unwrap();
    scene.bake(&lights, Some(&open(dir.path())), 4);

    let mut reversed = lights.clone();
    reversed.reverse();
    for (edit, set) in [("no edit", &lights), ("reorder", &reversed)] {
        let cold = scene.bake(set, None, 4).0;
        let cache = open(dir.path());
        let (warm, counts) = scene.bake(set, Some(&cache), 4);
        assert_eq!(shaded_and_cached(&counts), (0, 0), "{edit}: {counts:?}");
        assert_eq!((counts.shadow_rays, counts.gather_rays), (0, 0), "{edit}");
        assert_eq!(cache.test_access(SH_BASE_VOLUME_STAGE_ID).read_hits, 1);
        same_id_34(&warm, &cold, edit);

        scene.forget_volume(dir.path(), set);
        let (warm, counts) = scene.bake(set, Some(&open(dir.path())), 4);
        assert_eq!(shaded_and_cached(&counts), (0, 4), "{edit}: {counts:?}");
        assert_eq!(counts.shadow_rays, 0, "{edit}: {counts:?}");
        assert!(counts.gather_rays > 0, "the gather re-runs");
        same_id_34(&warm, &cold, edit);
    }

    let mut edited = lights.clone();
    edited[1].color = [0.2, 0.9, 0.4];
    let (warm, counts) = scene.bake(&edited, Some(&open(dir.path())), 4);
    assert_eq!(shaded_and_cached(&counts), (1, 3), "{counts:?}");
    let (_, alone) = counted(4, || {
        scene.with_ctx(&edited[1..2], |ctx| {
            PatchRadiance::single_bounce(
                &ctx.ray_ctx(),
                &PatchSet::build(&scene.geometry),
                &static_light_refs(ctx),
                &NO_EMBEDDED_EMITTERS,
                &BakeControl::unrestricted(),
            )
        })
    });
    assert!(alone.shadow_rays > 0);
    assert_eq!(counts.shadow_rays, alone.shadow_rays, "{counts:?}");
    same_id_34(&warm, &scene.bake(&edited, None, 4).0, "colour edit");
}

/// Seed a fresh cache with a warm bake of `base`, then bake `edited` warm
/// over it: `shaded` layers re-shade and `cached` hit, id 34 equals a cold
/// bake of `edited`, and every layer the edited bake read or wrote equals
/// its cold layer byte for byte.
fn assert_edit(
    scene: &Scene,
    edit: &str,
    base: &[MapLight],
    edited: &[MapLight],
    expected: (u64, u64),
) {
    let dir = tempfile::tempdir().unwrap();
    scene.bake(base, Some(&open(dir.path())), 4);
    let cache = open(dir.path());
    let (warm, counts) = scene.bake(edited, Some(&cache), 4);
    assert_eq!(shaded_and_cached(&counts), expected, "{edit}: {counts:?}");
    same_id_34(&warm, &scene.bake(edited, None, 4).0, edit);
    for (key, cold) in scene.cold_layers(edited, &NO_EMBEDDED_EMITTERS) {
        assert_eq!(cache.get(&key).as_ref(), Some(&cold), "{edit}");
    }
}

// Removing a static light, inserting one ahead
// of the others, adding a duplicate, adding then removing a light whose
// reach holds faces no other light reaches, or flipping one to animated
// (as `_animated 1` translates it) serves every unchanged light's layer
// from cache, equal to its cold layer.
#[test]
fn unchanged_lights_layers_hit_across_light_set_edits() {
    let scene = Scene::new();
    let lights = room_lights();

    let removed: Vec<MapLight> = [0, 2, 3].map(|i| lights[i].clone()).into();
    assert_edit(&scene, "remove", &lights, &removed, (0, 3));

    let mut inserted = vec![point([1.0, 1.5, 5.0], 0.15, 4.0)];
    inserted.extend(lights.iter().cloned());
    assert_edit(&scene, "insert ahead", &lights, &inserted, (1, 4));

    let mut duplicated = lights.clone();
    duplicated.push(lights[2].clone());
    assert_edit(&scene, "duplicate", &lights, &duplicated, (0, 4));

    let room_only = lights[..3].to_vec();
    let patches = PatchSet::build(&scene.geometry);
    let index = PatchReachIndex::new(patches.patches());
    let annex: HashSet<u32> = index.in_reach(&annex_light()).into_iter().collect();
    assert!(!annex.is_empty());
    for light in &room_only {
        assert!(
            index
                .in_reach(light)
                .iter()
                .all(|patch| !annex.contains(patch)),
            "no room light reaches the annex"
        );
    }
    assert_edit(
        &scene,
        "add unique-reach light",
        &room_only,
        &lights,
        (1, 3),
    );
    assert_edit(
        &scene,
        "remove unique-reach light",
        &lights,
        &room_only,
        (0, 3),
    );

    let mut flipped = lights.clone();
    flipped[1] = animated(flipped[1].clone());
    assert_edit(&scene, "flip to animated", &lights, &flipped, (0, 3));
}

// A removed, corrupt or undecodable layer entry
// re-shades alone, to the bytes it lost; the rest hit. While the gathered
// volume's entry survives it serves id 34 and no layer is read: the summed
// digest did not move.
#[test]
fn a_removed_or_corrupt_layer_reshades_alone_to_its_lost_bytes() {
    let scene = Scene::new();
    let lights = room_lights();
    let dir = tempfile::tempdir().unwrap();
    let (cold, _) = scene.bake(&lights, None, 4);
    scene.bake(&lights, Some(&open(dir.path())), 4);
    let snapshot = entries(dir.path());
    let entry = |light: &MapLight| dir.path().join(scene.key(light).as_filename());

    std::fs::remove_file(entry(&lights[1])).unwrap();
    let mut flipped = std::fs::read(entry(&lights[2])).unwrap();
    *flipped.last_mut().unwrap() ^= 0x5a;
    std::fs::write(entry(&lights[2]), flipped).unwrap();
    open(dir.path()).put(&scene.key(&lights[0]), b"not a patch layer");
    let (warm, counts) = scene.bake(&lights, Some(&open(dir.path())), 4);
    assert_eq!(shaded_and_cached(&counts), (0, 0), "volume hit: {counts:?}");
    same_id_34(&warm, &cold, "volume hit");
    for lost in ["removed, corrupt and undecodable", "none"] {
        scene.forget_volume(dir.path(), &lights);
        let (warm, counts) = scene.bake(&lights, Some(&open(dir.path())), 4);
        let expected = if lost == "none" { (0, 4) } else { (3, 1) };
        assert_eq!(shaded_and_cached(&counts), expected, "{lost}: {counts:?}");
        assert_eq!(
            entries(dir.path()),
            snapshot,
            "{lost}: re-shaded and re-gathered bytes repeat"
        );
        same_id_34(&warm, &cold, lost);
    }
}

// A warm build stopped mid patch shading (a governor
// enter hook that refuses every work item once the first layer is written)
// keeps the layers it finished, written as each finished and before the
// sum; the rerun re-shades only the rest.
#[test]
fn a_build_stopped_mid_patch_shading_reshades_only_unwritten_layers() {
    let scene = Scene::new();
    let lights = room_lights();
    let dir = tempfile::tempdir().unwrap();
    let stopped = open(dir.path());
    let governor = Arc::new(Governor::new(1, false));
    let watched = stopped.clone();
    governor.set_enter_hook(Arc::new(move || {
        if watched.test_access(SH_PATCH_LAYER_STAGE_ID).writes > 0 {
            panic!("build stopped mid patch shading");
        }
    }));
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(governor, &progress);
    let (outcome, _) = counted(1, || {
        catch_unwind(AssertUnwindSafe(|| {
            scene.bake_with(
                &lights,
                1.0,
                DEFAULT_BOUNCE_FILL,
                &NO_EMBEDDED_EMITTERS,
                Some(&stopped),
                &control,
            )
        }))
    });
    assert!(outcome.is_err(), "the hook stops the build");
    let written = stopped.test_access(SH_PATCH_LAYER_STAGE_ID).writes as u64;
    assert!((1..4).contains(&written), "{written} of 4 layers written");

    let (warm, counts) = scene.bake(&lights, Some(&open(dir.path())), 4);
    assert_eq!(
        shaded_and_cached(&counts),
        (4 - written, written),
        "{counts:?}"
    );
    same_id_34(&warm, &scene.bake(&lights, None, 4).0, "rerun");
}

/// One warm build of `lights` against the map at `input`, as the compiler
/// opens its cache: sweep to `budget`, bake, record the success. Returns
/// id 34, the counts, and how many of the build's layers the sweep kept.
fn build(
    scene: &Scene,
    dir: &Path,
    input: &Path,
    lights: &[MapLight],
    budget: u64,
) -> (OctahedralShVolumeSection, BakeCounts, u64) {
    let cache = StageCache::open_for_build(dir, input, budget).expect("open build");
    let kept = lights
        .iter()
        .filter(|light| dir.join(scene.key(light).as_filename()).exists())
        .count() as u64;
    let (section, counts) = scene.bake(lights, Some(&cache), 4);
    cache.finish_successful_build(budget);
    (section, counts, kept)
}

// After A, then B, then a revert to A, the revert hits
// every A layer the eviction sweep kept and re-shades the rest; with or
// without eviction its id 34 is a cold bake of A. Eviction moves hit
// counts, never bytes. A's gathered volume is deleted before the revert so
// the revert reads its layers.
#[test]
fn a_revert_hits_every_layer_the_eviction_sweep_kept() {
    let scene = Scene::new();
    let a = room_lights();
    let mut b = a.clone();
    b[1].intensity = 1.5;
    let (cold, _) = scene.bake(&a, None, 4);
    let mut reverts = Vec::new();
    for budget in [u64::MAX, 0] {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("room.map");
        std::fs::write(&input, "").unwrap();
        let dir = root.path().join("cache");
        build(&scene, &dir, &input, &a, u64::MAX);
        build(&scene, &dir, &input, &b, u64::MAX);
        scene.forget_volume(&dir, &a);
        let (revert, counts, kept) = build(&scene, &dir, &input, &a, budget);
        let expected_kept = if budget == 0 { 3 } else { 4 };
        assert_eq!(
            kept, expected_kept,
            "budget {budget}: B's record spares B's layers"
        );
        assert_eq!(
            shaded_and_cached(&counts),
            (4 - kept, kept),
            "budget {budget}"
        );
        same_id_34(&revert, &cold, "revert");
        reverts.push(revert);
    }
    same_id_34(&reverts[0], &reverts[1], "with and without eviction");
}

// No layer keys on `_bounce_fill`, so a fill change re-shades no
// layer; id 34 moves with the fill and equals a cold bake at the new one.
#[test]
fn changing_the_bounce_fill_reshades_no_layer() {
    let scene = Scene::new();
    let lights = room_lights();
    let dir = tempfile::tempdir().unwrap();
    let (full, counts) = scene.bake_at(&lights, 1.0, Some(&open(dir.path())), 4);
    assert_eq!(shaded_and_cached(&counts), (4, 0), "{counts:?}");
    let (half, counts) = scene.bake_at(&lights, 0.5, Some(&open(dir.path())), 4);
    assert_eq!(shaded_and_cached(&counts), (0, 4), "{counts:?}");
    assert_ne!(full.compact_atlas, half.compact_atlas, "id 34 moves");
    same_id_34(&half, &scene.bake_at(&lights, 0.5, None, 4).0, "fill 0.5");
}

// Every layer and gathered-volume cache read and write runs on a
// thread holding no governor permit: a cold fill, a volume hit, and a
// volume miss whose layers all hit.
#[test]
fn layer_cache_reads_and_writes_run_outside_governor_permits() {
    let governor = Arc::new(Governor::new(1, false));
    // The instrument counts a held permit, so its zero below means something.
    {
        let _permit = governor.enter();
        assert_eq!(permits_held_on_this_thread(), 1);
    }
    assert_eq!(permits_held_on_this_thread(), 0);

    let scene = Scene::new();
    let lights = room_lights();
    let dir = tempfile::tempdir().unwrap();
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::clone(&governor), &progress);
    let bake = || {
        counted(4, || {
            scene.bake_with(
                &lights,
                1.0,
                DEFAULT_BOUNCE_FILL,
                &NO_EMBEDDED_EMITTERS,
                Some(&open(dir.path())),
                &control,
            )
        })
        .1
    };
    let fill = bake();
    let hit = bake();
    scene.forget_volume(dir.path(), &lights);
    let layers_hit = bake();
    assert_eq!(
        (fill.layer_cache_io, fill.volume_cache_io),
        (8, 2),
        "four layer misses and writes, a volume miss and write: {fill:?}"
    );
    assert_eq!(
        (hit.layer_cache_io, hit.volume_cache_io),
        (0, 1),
        "one volume hit: {hit:?}"
    );
    assert_eq!(
        (layers_hit.layer_cache_io, layers_hit.volume_cache_io),
        (4, 2),
        "four layer hits, a volume miss and write: {layers_hit:?}"
    );
    for counts in [fill, hit, layers_hit] {
        assert_eq!(counts.layer_cache_io_under_permit, 0, "{counts:?}");
        assert_eq!(counts.volume_cache_io_under_permit, 0, "{counts:?}");
    }
    assert_eq!(governor.active(), 0);
}
