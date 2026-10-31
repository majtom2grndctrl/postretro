// SH base cost and order tests: shadow rays stay in patch direct, layers follow content.
// See: context/lib/build_pipeline.md §Build Cache

use std::path::Path;

use super::bounce::DEFAULT_BOUNCE_FILL;
use super::layer::SH_PATCH_LAYER_STAGE_ID;
use super::layer_set::LayerPlan;
use super::patches::{PatchSet, Side};
use super::sum::PatchRadiance;
use super::test_scene::{Scene, entries, point, room_lights};
use crate::bake_control::BakeControl;
use crate::bake_counters::counted;
use crate::cache::StageCache;
use crate::sh_bake::static_light_refs;
use crate::texel_footprint::NO_EMBEDDED_EMITTERS;

fn same_id_34(
    a: &postretro_level_format::sh_volume::OctahedralShVolumeSection,
    b: &postretro_level_format::sh_volume::OctahedralShVolumeSection,
    context: &str,
) {
    assert_eq!(a.compact_atlas, b.compact_atlas, "{context}: probe tiles");
    assert_eq!(a.probes, b.probes, "{context}: depth moments");
}

// A cold bake traces every shadow ray in patch direct. Its total
// equals patch direct's alone and does not move when probe spacing halves,
// while the gather's rays grow with the probe count. `RAYS_PER_PROBE` is a
// code constant, so spacing is the probe-side knob a test can turn.
#[test]
fn a_cold_bake_traces_no_shadow_ray_from_the_probe_gather() {
    let scene = Scene::new();
    let lights = room_lights();
    let bake = |spacing| {
        counted(4, || {
            scene.bake_with(
                &lights,
                spacing,
                DEFAULT_BOUNCE_FILL,
                &NO_EMBEDDED_EMITTERS,
                None,
                &BakeControl::unrestricted(),
            )
        })
        .1
    };
    let (_, direct) = counted(4, || {
        scene.with_ctx(&lights, |ctx| {
            let patches = PatchSet::build(&scene.geometry);
            PatchRadiance::single_bounce(
                &ctx.ray_ctx(),
                &patches,
                &static_light_refs(ctx),
                &NO_EMBEDDED_EMITTERS,
                &BakeControl::unrestricted(),
            )
        })
    });
    let coarse = bake(1.0);
    let fine = bake(0.5);
    assert!(
        direct.shadow_rays > 0 && direct.gather_rays == 0,
        "{direct:?}"
    );
    assert_eq!(coarse.shadow_rays, direct.shadow_rays, "{coarse:?}");
    assert_eq!(fine.shadow_rays, direct.shadow_rays, "{fine:?}");
    assert!(
        fine.gather_rays > 4 * coarse.gather_rays,
        "halved spacing gathers more: {coarse:?} vs {fine:?}"
    );
    assert!(coarse.patch_lookups > 0 && coarse.patch_lookups <= coarse.gather_rays);
}

// Later bounces gather patch to patch and read the cache as the
// probe gather does, so fill 1 traces exactly fill 0's shadow rays, every
// one in patch direct, and the probe gather's rays and lookups do not move.
#[test]
fn later_bounces_trace_no_shadow_ray() {
    let scene = Scene::new();
    let lights = room_lights();
    let (_, single) = scene.bake_at(&lights, 0.0, None, 4);
    let (_, multi) = scene.bake_at(&lights, 1.0, None, 4);
    assert_eq!(multi.shadow_rays, single.shadow_rays, "{multi:?}");
    assert!(single.shadow_rays > 0);
    assert_eq!(single.bounce_rays, 0, "{single:?}");
    assert!(multi.bounce_rays > 0, "{multi:?}");
    assert_eq!(multi.gather_rays, single.gather_rays);
    assert_eq!(multi.patch_lookups, single.patch_lookups);
}

/// `source` with comment lines removed: the gate reads code, not prose.
fn code(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of `fn name` in `source`, through its closing brace.
fn function(source: &str, name: &str) -> String {
    let start = source
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("no fn {name}"));
    let end = start
        + source[start..]
            .find("\n}\n")
            .unwrap_or_else(|| panic!("fn {name} never closes"));
    source[start..end].to_owned()
}

// The probe gather's patch path and the later bounces
// call no visibility routine. It covers the gather (`gather.rs`), the patch
// lookups it reads (`patches.rs`), the gather's hit (`gather_hit`), the
// probe bake's patch arm, and the later bounces (`bounce.rs`) with their
// hit (`bounce_hit`).
#[test]
fn the_patch_gather_path_calls_no_visibility_routine() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let read = |path: &str| std::fs::read_to_string(src.join(path)).expect(path);
    let probe = read("sh_bake/probe.rs");
    let patch_arms: Vec<&str> = probe
        .lines()
        .filter(|line| line.contains("RadianceSource::Patches("))
        .collect();
    assert_eq!(patch_arms.len(), 1, "{patch_arms:?}");
    assert!(
        patch_arms[0].contains("=> patches.sample("),
        "{patch_arms:?}"
    );
    let sources = [
        ("gather.rs", code(&read("surface_radiance/gather.rs"))),
        ("patches.rs", code(&read("surface_radiance/patches.rs"))),
        (
            "gather_hit",
            function(&read("sh_bake/trace.rs"), "gather_hit"),
        ),
        ("patch arm", patch_arms[0].to_owned()),
        ("bounce.rs", code(&read("surface_radiance/bounce.rs"))),
        (
            "bounce_hit",
            function(&read("sh_bake/trace.rs"), "bounce_hit"),
        ),
    ];
    for (name, source) in &sources {
        for routine in [
            "segment_clear",
            "visibility",
            "sample_radiance",
            "bounce_contribution",
            "light_contribution",
            "area_sample_target",
            "hard_ray_target",
        ] {
            assert!(!source.contains(routine), "{name} calls {routine}");
        }
    }
}

// Doubling the static lights leaves the gather's rays and patch
// lookups alone, and patch direct evaluates exactly the patches in each
// light's reach, not every patch per light.
#[test]
fn doubling_static_lights_leaves_the_gather_and_bounds_patch_direct_by_reach() {
    let scene = Scene::new();
    let lights = room_lights();
    let mut doubled = lights.clone();
    doubled.extend(lights.iter().map(|light| {
        let mut moved = light.clone();
        moved.origin.x += 0.5;
        moved.origin.z += 0.3;
        moved
    }));
    let patch_count = PatchSet::build(&scene.geometry).len() as u64;
    let (_, one) = scene.bake(&lights, None, 4);
    let (_, two) = scene.bake(&doubled, None, 4);
    assert_eq!(one.gather_rays, two.gather_rays);
    assert_eq!(one.patch_lookups, two.patch_lookups);
    for (set, counts) in [(&lights, one), (&doubled, two)] {
        let reach: u64 = scene.reach(set).iter().sum();
        assert_eq!(counts.patch_light_evaluations, reach, "{counts:?}");
        assert!(
            counts.patch_light_evaluations < set.len() as u64 * patch_count,
            "the per-patch range early-out no longer visits every patch"
        );
    }
    assert!(two.patch_light_evaluations > one.patch_light_evaluations);
}

// The layers sum in content order, so reversing the static lights'
// entity order leaves id 34's probe tiles and depth moments unchanged.
#[test]
fn reversing_static_light_order_leaves_cold_id_34_identical() {
    let scene = Scene::new();
    let lights = room_lights();
    let mut reversed = lights.clone();
    reversed.reverse();
    let (forward, _) = scene.bake(&lights, None, 4);
    let (backward, _) = scene.bake(&reversed, None, 4);
    assert!(forward.compact_atlas.iter().any(|&byte| byte != 0));
    same_id_34(&forward, &backward, "reversed");
}

// A duplicated static light adds its layer once per copy, doubling
// its share of the patch radiance in its reach.
#[test]
fn a_duplicated_static_light_doubles_patch_radiance_in_its_reach() {
    let scene = Scene::new();
    let lights = room_lights();
    let patches = PatchSet::build(&scene.geometry);
    let radiance = |set: &[crate::map_data::MapLight]| {
        scene.with_ctx(set, |ctx| {
            PatchRadiance::single_bounce(
                &ctx.ray_ctx(),
                &patches,
                &static_light_refs(ctx),
                &NO_EMBEDDED_EMITTERS,
                &BakeControl::unrestricted(),
            )
        })
    };
    let base = radiance(&lights);
    let mut duplicated = lights.clone();
    duplicated.push(lights[1].clone());
    let doubled = radiance(&duplicated);
    let alone = radiance(&lights[1..2]);
    let mut lit = 0;
    for patch in 0..patches.len() as u32 {
        for side in Side::BOTH {
            let (base, doubled, alone) = (
                base.get(patch, side),
                doubled.get(patch, side),
                alone.get(patch, side),
            );
            let error = (doubled - (base + alone)).abs().max_element();
            assert!(
                error <= 1.0e-5 * doubled.max_element().max(1.0e-6),
                "patch {patch} {side:?}: {doubled} vs {base} + {alone}"
            );
            lit += usize::from(alone.max_element() > 0.0);
        }
    }
    assert!(lit > 0, "the light reaches some patch");
}

// A static light whose reach holds no patch shades
// nothing, plans no layer, writes no entry, and leaves id 34 as the map
// without it bakes it, cold and warm.
#[test]
fn a_light_whose_reach_holds_no_patch_allocates_no_layer() {
    let scene = Scene::new();
    let lights = room_lights();
    let mut with_far = lights.clone();
    with_far.push(point([40.0, 10.0, 40.0], 0.2, 1.0));
    assert_eq!(scene.reach(&with_far).last(), Some(&0));
    let planned = |set: &[crate::map_data::MapLight]| {
        scene.with_ctx(set, |ctx| {
            LayerPlan::new(
                &static_light_refs(ctx),
                &scene.reach(set),
                &NO_EMBEDDED_EMITTERS,
            )
            .layers
            .len()
        })
    };
    assert_eq!(planned(&with_far), planned(&lights));

    let (without, plain) = scene.bake(&lights, None, 4);
    let (cold, counts) = scene.bake(&with_far, None, 4);
    same_id_34(&cold, &without, "cold");
    assert_eq!(counts.layers_shaded, plain.layers_shaded);
    assert_eq!(
        counts.patch_light_evaluations,
        plain.patch_light_evaluations
    );

    let dir = tempfile::tempdir().unwrap();
    let cache = StageCache::new(dir.path()).unwrap();
    let (warm, counts) = scene.bake(&with_far, Some(&cache), 4);
    same_id_34(&warm, &without, "warm");
    let access = cache.test_access(SH_PATCH_LAYER_STAGE_ID);
    assert_eq!(counts.layers_shaded, plain.layers_shaded);
    assert_eq!(access.writes as u64, plain.layers_shaded);
    assert_eq!(access.read_attempts as u64, plain.layers_shaded);
}

// Determinism: layer entries, the gathered-volume entry and id 34 are
// byte-identical at one worker and at many, and warm equals cold.
#[test]
fn layer_entries_and_id_34_are_byte_identical_at_one_worker_and_many() {
    let scene = Scene::new();
    let lights = room_lights();
    let (cold, _) = scene.bake(&lights, None, 1);
    let bake = |threads| {
        let dir = tempfile::tempdir().unwrap();
        let cache = StageCache::new(dir.path()).unwrap();
        let (section, counts) = scene.bake(&lights, Some(&cache), threads);
        assert_eq!(counts.layers_shaded, 4, "{counts:?}");
        (section, entries(dir.path()))
    };
    let (one, one_entries) = bake(1);
    let (many, many_entries) = bake(8);
    assert_eq!(one_entries.len(), 5, "four layers and the gathered volume");
    assert_eq!(one_entries, many_entries);
    same_id_34(&one, &cold, "warm at one worker");
    same_id_34(&many, &cold, "warm at eight workers");
}
