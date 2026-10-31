// Gathered id-34 volume entry tests: what re-keys it, what it rebuilds, and misses that stay exact.
// See: context/lib/build_pipeline.md §Build Cache

use std::path::Path;

use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::layer::SH_PATCH_LAYER_STAGE_ID;
use super::test_scene::{Scene, animated, point, room_lights};
use super::volume::SH_BASE_VOLUME_STAGE_ID;
use crate::bake_counters::BakeCounts;
use crate::cache::{CacheKey, CacheTestAccess, StageCache};
use crate::map_data::MapLight;
use crate::surface_materials::stats::{BounceMaterials, MaterialStats};

fn open(dir: &Path) -> StageCache {
    StageCache::new(dir).expect("cache dir")
}

fn shaded_and_cached(counts: &BakeCounts) -> (u64, u64) {
    (counts.layers_shaded, counts.layers_cached)
}

fn volume(cache: &StageCache) -> CacheTestAccess {
    cache.test_access(SH_BASE_VOLUME_STAGE_ID)
}

/// One warm bake of `lights` over `scene` into `dir`: id 34, what it spent
/// and the volume entry's accesses.
fn warm(
    scene: &Scene,
    dir: &Path,
    lights: &[MapLight],
) -> (OctahedralShVolumeSection, BakeCounts, CacheTestAccess) {
    let cache = open(dir);
    let (section, counts) = scene.bake(lights, Some(&cache), 4);
    (section, counts, volume(&cache))
}

fn assert_volume_hit(access: CacheTestAccess, context: &str) {
    assert_eq!(
        (access.read_attempts, access.read_hits, access.writes),
        (1, 1, 0),
        "{context}: the volume entry hits: {access:?}"
    );
}

fn assert_volume_miss(access: CacheTestAccess, context: &str) {
    assert_eq!(
        (access.read_attempts, access.read_hits, access.writes),
        (1, 0, 1),
        "{context}: the volume entry misses and is written: {access:?}"
    );
}

/// Whether every RGBA16F texel of `section`'s atlas has zero RGB. Alpha
/// marks a stored texel, not light.
fn irradiance_is_zero(section: &OctahedralShVolumeSection) -> bool {
    section
        .compact_atlas
        .as_chunks::<8>()
        .0
        .iter()
        .all(|texel| texel[..6].iter().all(|&byte| byte == 0))
}

/// The room with one tinted texture on every face, at `saturation`.
pub(super) fn coloured(mean: [u8; 3], saturation: f32) -> Scene {
    Scene::new().with_materials(
        BounceMaterials::new(vec![MaterialStats::with_snapped_mean(mean)], saturation).unwrap(),
    )
}

/// The room with the annex floor lifted 1 cm: a geometry edit no light's
/// reach count notices.
pub(super) fn lifted_annex() -> Scene {
    let base = Scene::new();
    let mut geometry = base.geometry.clone();
    let annex = *geometry.face_index_ranges.last().unwrap();
    let first = annex.index_offset as usize;
    let corners: std::collections::BTreeSet<u32> = geometry.geometry.indices
        [first..first + annex.index_count as usize]
        .iter()
        .copied()
        .collect();
    for corner in corners {
        geometry.geometry.vertices[corner as usize].position[1] += 0.01;
    }
    Scene::of(geometry)
}

// A second warm bake of the same lights hits the gathered volume: it reads
// no layer, traces no ray and returns the cold bytes, animation tables
// included.
#[test]
fn a_rebuild_hits_the_volume_entry_and_reads_no_layer() {
    let scene = Scene::new();
    let lights = room_lights();
    let dir = tempfile::tempdir().unwrap();
    let (cold, _) = scene.bake(&lights, None, 4);
    let (first, _, access) = warm(&scene, dir.path(), &lights);
    assert_volume_miss(access, "first build");
    assert_eq!(first, cold);
    let cache = open(dir.path());
    let (second, counts) = scene.bake(&lights, Some(&cache), 4);
    assert_volume_hit(volume(&cache), "rebuild");
    assert_eq!(
        cache.test_access(SH_PATCH_LAYER_STAGE_ID),
        CacheTestAccess::default()
    );
    assert_eq!(shaded_and_cached(&counts), (0, 0), "{counts:?}");
    assert_eq!(
        (counts.shadow_rays, counts.gather_rays, counts.bounce_rays),
        (0, 0, 0)
    );
    assert_eq!(second, cold);
}

// AC 14, research P4, P5: a geometry edit, a texture-mean edit or a
// saturation edit re-keys every layer and the volume entry, so the warm
// bake re-shades every layer and equals a cold bake of the edit; restoring
// the original hits the volume entry.
#[test]
fn geometry_mean_and_saturation_edits_rekey_every_layer_and_the_volume() {
    let lights = room_lights();
    for (edit, base, edited) in [
        ("geometry", Scene::new(), lifted_annex()),
        (
            "texture mean",
            coloured([200, 40, 10], 1.0),
            coloured([40, 200, 10], 1.0),
        ),
        (
            "saturation",
            coloured([200, 40, 10], 1.0),
            coloured([200, 40, 10], 0.5),
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        warm(&base, dir.path(), &lights);
        for light in &lights {
            assert_ne!(
                base.key(light).as_filename(),
                edited.key(light).as_filename(),
                "{edit}: every layer re-keys"
            );
        }
        assert_ne!(
            base.volume_key(&lights).as_filename(),
            edited.volume_key(&lights).as_filename(),
            "{edit}: the volume re-keys"
        );
        let (section, counts, access) = warm(&edited, dir.path(), &lights);
        assert_volume_miss(access, edit);
        assert_eq!(shaded_and_cached(&counts), (4, 0), "{edit}: {counts:?}");
        assert_eq!(section, edited.bake(&lights, None, 4).0, "{edit}");

        let (restored, counts, access) = warm(&base, dir.path(), &lights);
        assert_volume_hit(access, edit);
        assert_eq!(shaded_and_cached(&counts), (0, 0), "{edit}: {counts:?}");
        assert_eq!(restored, base.bake(&lights, None, 4).0, "{edit} restored");
    }
}

// AC 14: the fill is not in any layer key but is in the volume's, so a
// fill change misses the volume, re-shades no layer and moves id 34, warm
// as cold; −0.0 and 0.0 are one fill.
#[test]
fn a_fill_change_misses_the_volume_and_reshades_no_layer() {
    let scene = Scene::new();
    let lights = room_lights();
    let dir = tempfile::tempdir().unwrap();
    let (full, _) = scene.bake_at(&lights, 1.0, Some(&open(dir.path())), 4);
    let cache = open(dir.path());
    let (half, counts) = scene.bake_at(&lights, 0.5, Some(&cache), 4);
    assert_volume_miss(volume(&cache), "fill 0.5");
    assert_eq!(shaded_and_cached(&counts), (0, 4), "{counts:?}");
    assert_ne!(full.compact_atlas, half.compact_atlas, "id 34 moves");
    assert_eq!(half, scene.bake_at(&lights, 0.5, None, 4).0);
    assert_eq!(
        scene.volume_key_at(&lights, 1.0, 0.0).as_filename(),
        scene.volume_key_at(&lights, 1.0, -0.0).as_filename(),
    );
}

// AC 15, research P17, P21: an animated light added, re-timed or removed
// re-shades no layer and hits the volume, whose probe tiles and depth
// moments stay put; the descriptors and slot table rebuild from the build's
// own lights and equal a cold bake's. (The retired grouped path pinned its
// parallel descriptor builder to the cold bake; one builder now serves
// both, and this pins the hit to the cold bake.)
#[test]
fn animated_only_edits_hit_the_volume_and_rebuild_its_animation_tables() {
    let scene = Scene::new();
    let statics = room_lights();
    let dir = tempfile::tempdir().unwrap();
    let (base, _, _) = warm(&scene, dir.path(), &statics);

    let pulse = animated(point([5.0, 2.0, 1.0], 0.1, 4.0));
    let mut retimed_pulse = pulse.clone();
    retimed_pulse.animation.as_mut().unwrap().phase = 0.5;
    let with = |extra: &[MapLight]| {
        let mut lights = statics.clone();
        lights.insert(1, extra[0].clone());
        lights.extend(extra[1..].iter().cloned());
        lights
    };
    let second = animated(point([1.0, 1.0, 5.0], 0.0, 3.0));
    let edits = [
        ("added", with(std::slice::from_ref(&pulse))),
        ("re-timed", with(std::slice::from_ref(&retimed_pulse))),
        ("another added", with(&[retimed_pulse.clone(), second])),
        ("removed", statics.clone()),
    ];
    let mut previous_tables = None;
    for (edit, lights) in edits {
        let (section, counts, access) = warm(&scene, dir.path(), &lights);
        assert_volume_hit(access, edit);
        assert_eq!(shaded_and_cached(&counts), (0, 0), "{edit}: {counts:?}");
        assert_eq!(section.compact_atlas, base.compact_atlas, "{edit}: tiles");
        assert_eq!(section.probes, base.probes, "{edit}: depth moments");
        let cold = scene.bake(&lights, None, 4).0;
        assert_eq!(section, cold, "{edit}: descriptors and slot table");
        let tables = (section.animation_descriptors, section.slot_for_map_light);
        assert_ne!(previous_tables.as_ref(), Some(&tables), "{edit} moves them");
        previous_tables = Some(tables);
    }
}

// Research P20: a brush edit that moves only leaf solidity leaves every
// face and so every layer as it was, but flips probe validity, which the
// layer geometry digest does not cover and the volume's cell-id fingerprint
// does: the volume misses on validity alone and the warm bake equals a cold
// bake of the edit.
#[test]
fn a_validity_only_edit_hits_every_layer_and_misses_the_volume() {
    let lights = room_lights();
    let base = Scene::new();
    let edited = Scene::new().with_solid_beyond(10.0);
    let dir = tempfile::tempdir().unwrap();
    let (before, _, _) = warm(&base, dir.path(), &lights);
    let (after, counts, access) = warm(&edited, dir.path(), &lights);
    assert_volume_miss(access, "validity edit");
    assert_eq!(shaded_and_cached(&counts), (0, 4), "{counts:?}");
    let invalid = |section: &OctahedralShVolumeSection| {
        section
            .probes
            .iter()
            .filter(|probe| probe.validity == 0)
            .count()
    };
    assert!(invalid(&after) > invalid(&before), "probes turn invalid");
    assert_eq!(after, edited.bake(&lights, None, 4).0);
}

// AC 17, research P6, P14: a warm build into an empty cache directory
// misses every layer and the volume and gives the cold bytes. A directory
// holding a pre-change grouped `sh_group` entry is no different: the volume
// has its own stage id and epoch, so no pre-change entry can answer it.
#[test]
fn a_warm_build_over_a_pre_change_cache_misses_every_layer_and_the_volume() {
    assert_ne!(SH_BASE_VOLUME_STAGE_ID, "sh_group");
    let scene = Scene::new();
    let lights = room_lights();
    let dir = tempfile::tempdir().unwrap();
    let grouped = CacheKey::new("sh_group", 4, b"a pre-change probe group");
    open(dir.path()).put(&grouped, b"grouped SH payload");
    assert_ne!(
        grouped.as_filename(),
        scene.volume_key(&lights).as_filename()
    );
    let (section, counts, access) = warm(&scene, dir.path(), &lights);
    assert_volume_miss(access, "first warm build");
    assert_eq!(shaded_and_cached(&counts), (4, 0), "{counts:?}");
    assert_eq!(section, scene.bake(&lights, None, 4).0);
    assert_eq!(
        open(dir.path()).get(&grouped).as_deref(),
        Some(b"grouped SH payload".as_slice()),
        "the pre-change entry is left to age out"
    );
}

// A corrupt entry, one that is not a volume, and one that decodes to
// another grid are each a miss: the bake re-gathers from its layers, all of
// which hit, and rewrites the entry with the cold bytes.
#[test]
fn an_undecodable_or_foreign_volume_entry_is_a_miss() {
    let scene = Scene::new();
    let lights = room_lights();
    let dir = tempfile::tempdir().unwrap();
    let (cold, _) = scene.bake(&lights, None, 4);
    warm(&scene, dir.path(), &lights);
    let entry = dir.path().join(scene.volume_key(&lights).as_filename());
    let good = std::fs::read(&entry).unwrap();

    let coarse = scene.volume_key_at(&lights, 2.0, super::DEFAULT_BOUNCE_FILL);
    scene.bake_with(
        &lights,
        2.0,
        super::DEFAULT_BOUNCE_FILL,
        &crate::texel_footprint::NO_EMBEDDED_EMITTERS,
        Some(&open(dir.path())),
        &crate::bake_control::BakeControl::unrestricted(),
    );
    let foreign = open(dir.path()).get(&coarse).expect("the 2 m volume entry");

    let mut flipped = good.clone();
    *flipped.last_mut().unwrap() ^= 0x5a;
    for (tamper, write) in [
        (
            "corrupt",
            Box::new(|| std::fs::write(&entry, &flipped).unwrap()) as Box<dyn Fn()>,
        ),
        (
            "not a volume",
            Box::new(|| open(dir.path()).put(&scene.volume_key(&lights), b"not a volume")),
        ),
        (
            "another grid",
            Box::new(|| open(dir.path()).put(&scene.volume_key(&lights), &foreign)),
        ),
    ] {
        write();
        let (section, counts, access) = warm(&scene, dir.path(), &lights);
        // A corrupt entry fails the cache's own hash; the others pass it
        // and fail to decode as this grid's volume. Either way it is
        // re-gathered and rewritten.
        assert_eq!(
            (access.read_attempts, access.writes),
            (1, 1),
            "{tamper}: {access:?}"
        );
        assert_eq!(shaded_and_cached(&counts), (0, 4), "{tamper}: {counts:?}");
        assert_eq!(section, cold, "{tamper}");
        assert_eq!(std::fs::read(&entry).unwrap(), good, "{tamper}: rewritten");
    }
}

// AC 28, research P9: with no static light (a map lit only by emissive
// textures has none, since emissive lights nothing) no layer is planned,
// shaded or written, and id 34's irradiance is all zero, warm and cold. The
// probe rays still trace, so the depth moments are the lit map's.
#[test]
fn no_static_light_bakes_zero_irradiance_and_allocates_no_layer() {
    let scene = Scene::new().with_materials(
        BounceMaterials::new(vec![MaterialStats::with_snapped_mean([255; 3])], 1.0).unwrap(),
    );
    let lit = scene.bake(&room_lights(), None, 4).0;
    for (case, lights) in [
        ("no light", Vec::new()),
        (
            "animated only",
            vec![animated(point([4.0, 2.0, 3.0], 0.1, 6.0))],
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let cache = open(dir.path());
        let (section, counts) = scene.bake(&lights, Some(&cache), 4);
        assert_eq!(
            (counts.layers_shaded, counts.patch_light_evaluations),
            (0, 0),
            "{case}"
        );
        assert_eq!(cache.test_access(SH_PATCH_LAYER_STAGE_ID).writes, 0);
        assert!(!section.compact_atlas.is_empty());
        assert!(irradiance_is_zero(&section), "{case}: all-zero irradiance");
        assert!(counts.gather_rays > 0, "{case}: the probes still trace");
        assert_eq!(section.probes, lit.probes, "{case}: depth moments");
        assert_eq!(
            section,
            scene.bake(&lights, None, 4).0,
            "{case}: warm = cold"
        );
    }
}

// Eviction: a volume hit reads no layer, yet the layers it summarizes must
// survive the next sweep, so the next light edit re-shades one layer rather
// than all. The hit marks each one used, as a lightmap section-memo hit
// marks its partitions; the hit build's record then spares them all.
#[test]
fn a_volume_hit_marks_every_layer_it_sums_used_so_eviction_spares_them() {
    let scene = Scene::new();
    let lights = room_lights();
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("room.map");
    std::fs::write(&input, "").unwrap();
    let dir = root.path().join("cache");
    for expect_hit in [false, true] {
        let cache = StageCache::open_for_build(&dir, &input, u64::MAX).expect("open build");
        scene.bake(&lights, Some(&cache), 4);
        assert_eq!(volume(&cache).read_hits, usize::from(expect_hit));
        cache.finish_successful_build(u64::MAX);
    }
    // A zero budget evicts every entry no map's record spares.
    StageCache::open_for_build(&dir, &input, 0).expect("sweep");
    for light in &lights {
        assert!(
            dir.join(scene.key(light).as_filename()).exists(),
            "the hit build's record spares every layer"
        );
    }
    assert!(dir.join(scene.volume_key(&lights).as_filename()).exists());
}
