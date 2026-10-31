// SH base determinism: worker count, repeats and governor pauses; fixed gather directions.
// See: context/lib/build_pipeline.md §Build Cache (Determinism invariant)

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::LATER_BOUNCE_COUNT;
use super::bounce::later_bounce_progress_units;
use super::layer_set::LayerPlan;
use super::patches::PatchSet;
use super::test_scene::{Scene, point, room_lights};
use super::working_set_tests::scene_fixtures;
use crate::bake_control::BakeControl;
use crate::bake_counters::{Gather, TracedDirection, directions_traced};
use crate::governor::Governor;
use crate::map_data::MapLight;
use crate::reporter::StageProgress;
use crate::sh_bake::static_light_refs;
use crate::texel_footprint::{EmbeddedEmitters, NO_EMBEDDED_EMITTERS};

/// Every byte of id 34, animation tables included.
fn bytes(section: &OctahedralShVolumeSection) -> Vec<u8> {
    section.try_to_bytes().expect("id 34 serializes")
}

/// A cold bake on a fresh pool of `threads` workers.
fn cold(
    scene: &Scene,
    lights: &[MapLight],
    spacing: f32,
    fill: f32,
    embedded: &EmbeddedEmitters,
    threads: usize,
) -> Vec<u8> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("pool");
    bytes(&pool.install(|| {
        scene.bake_with(
            lights,
            spacing,
            fill,
            embedded,
            None,
            &BakeControl::unrestricted(),
        )
    }))
}

// Cold id-34 bytes are identical at one worker, two and eight, and
// across repeat runs, on every in-memory fixture the working-set sweep measures
// (the embedded-emitter fixture with its flags among them), at fill 0
// and 1. With `cost_tests::layer_entries_and_id_34_are_byte_identical_at_
// one_worker_and_many` (warm, every cache entry) and `tests::a_layer_is_
// byte_identical_at_one_worker_and_many` (one layer, flagged and not).
#[test]
fn cold_id_34_is_identical_at_any_worker_count_and_across_repeats() {
    for fixture in scene_fixtures() {
        for fill in [0.0, 1.0] {
            let bake = |threads| {
                cold(
                    &fixture.scene,
                    &fixture.lights,
                    fixture.spacing,
                    fill,
                    &fixture.embedded,
                    threads,
                )
            };
            let reference = bake(1);
            for (threads, run) in [(1, "repeat"), (2, "first"), (8, "first"), (8, "repeat")] {
                assert!(
                    bake(threads) == reference,
                    "{} fill {fill}: {threads} workers ({run}) moved id 34",
                    fixture.name
                );
            }
        }
    }
}

/// Where in the SH base bake a governor pause lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    PatchShading,
    LaterBounces,
    ProbeGather,
}

/// A cold bake on eight workers whose governor pauses once at the first
/// work item of each phase and resumes from another thread after a delay,
/// as the TUI's pause does. Returns id 34 and the phases paused in.
fn paused_cold_bake(scene: &Scene, lights: &[MapLight], fill: f32) -> (Vec<u8>, Vec<Phase>) {
    // Work items per phase: a permit per patch chunk of each layer's reach,
    // per patch chunk of each later bounce, then per probe.
    let reach = scene.reach(lights);
    let shading = scene.with_ctx(lights, |ctx| {
        LayerPlan::new(&static_light_refs(ctx), &reach, &NO_EMBEDDED_EMITTERS).progress_units()
    });
    let bounces = later_bounce_progress_units(PatchSet::build(&scene.geometry).len(), fill);
    assert!(shading > 0 && bounces > 0, "both phases run");
    let pause_at = BTreeMap::from([
        (1, Phase::PatchShading),
        (shading + 1, Phase::LaterBounces),
        (shading + bounces + 1, Phase::ProbeGather),
    ]);

    let governor = Arc::new(Governor::new(8, false));
    let entered = Arc::new(AtomicUsize::new(0));
    let paused = Arc::new(Mutex::new(Vec::new()));
    let weak: Weak<Governor> = Arc::downgrade(&governor);
    {
        let (entered, paused) = (Arc::clone(&entered), Arc::clone(&paused));
        governor.set_enter_hook(Arc::new(move || {
            let item = entered.fetch_add(1, Ordering::SeqCst) + 1;
            let (Some(&phase), Some(governor)) = (pause_at.get(&item), weak.upgrade()) else {
                return;
            };
            governor.set_paused(true);
            paused.lock().unwrap().push(phase);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(30));
                governor.set_paused(false);
            });
        }));
    }
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::clone(&governor), &progress);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(8)
        .build()
        .expect("pool");
    let section =
        pool.install(|| scene.bake_with(lights, 1.0, fill, &NO_EMBEDDED_EMITTERS, None, &control));
    let mut phases = paused.lock().unwrap().clone();
    phases.sort();
    (bytes(&section), phases)
}

// A governor pause during patch shading, during the later bounces
// and during the probe gather, each resumed from another thread, leaves
// cold id 34 byte-identical to an unpaused bake on one worker.
#[test]
fn a_governor_pause_in_any_phase_leaves_cold_id_34_identical() {
    let scene = Scene::new();
    let lights = room_lights();
    let reference = cold(&scene, &lights, 1.0, 1.0, &NO_EMBEDDED_EMITTERS, 1);
    let (paused, phases) = paused_cold_bake(&scene, &lights, 1.0);
    assert_eq!(
        phases,
        [Phase::PatchShading, Phase::LaterBounces, Phase::ProbeGather],
        "the governor paused in every phase"
    );
    assert!(paused == reference, "a pause moved id 34");
}

/// Every gather direction a cold bake of `lights` at `fill` traces.
fn traced(scene: &Scene, lights: &[MapLight], fill: f32) -> Vec<TracedDirection> {
    directions_traced(4, || {
        scene.bake_with(
            lights,
            1.0,
            fill,
            &NO_EMBEDDED_EMITTERS,
            None,
            &BakeControl::unrestricted(),
        )
    })
    .1
}

/// One gather's distinct (index, ray, direction) records, and how often
/// each was traced.
fn of(gather: Gather, traced: &[TracedDirection]) -> BTreeMap<(u64, u32, [u32; 3]), usize> {
    let mut records = BTreeMap::new();
    for &(g, index, ray, dir) in traced {
        if g == gather {
            *records.entry((index, ray, dir)).or_insert(0) += 1;
        }
    }
    records
}

// The probe gather and the later-bounce patch gather trace one
// fixed direction set whatever the lights or the fill: two light sets at
// two fills record the same directions for every probe and patch ray.
// Every probe traces the same set; each patch re-traces its own set every
// later bounce; fill 0 traces no patch ray and the same probe rays.
#[test]
fn gather_directions_do_not_depend_on_lights_or_fill() {
    let scene = Scene::new();
    let a = room_lights();
    let mut b = vec![point([5.0, 1.0, 1.5], 0.4, 6.0)];
    b[0].color = [0.2, 0.5, 1.0];
    b[0].intensity = 3.0;
    let runs = [(&a, 1.0), (&a, 0.5), (&b, 1.0), (&b, 0.5)].map(|(lights, fill)| {
        let traced = traced(&scene, lights, fill);
        (of(Gather::Probe, &traced), of(Gather::Patch, &traced))
    });
    let (probes, patches) = &runs[0];
    assert!(!probes.is_empty() && !patches.is_empty());
    for (run, (other_probes, other_patches)) in runs.iter().enumerate().skip(1) {
        assert!(other_probes == probes, "run {run}: probe directions moved");
        assert!(
            other_patches == patches,
            "run {run}: patch directions moved"
        );
    }

    // One direction per probe ray index, shared by every probe, each traced
    // once per bake.
    let mut by_ray: BTreeMap<u32, [u32; 3]> = BTreeMap::new();
    for (&(_, ray, dir), &count) in probes {
        assert_eq!(count, 1);
        assert_eq!(*by_ray.entry(ray).or_insert(dir), dir, "probe ray {ray}");
    }
    // One direction per (patch, ray), traced once per later bounce.
    let mut by_patch_ray: BTreeMap<(u64, u32), [u32; 3]> = BTreeMap::new();
    for (&(patch, ray, dir), &count) in patches {
        assert_eq!(
            count, LATER_BOUNCE_COUNT as usize,
            "patch {patch} ray {ray}"
        );
        assert_eq!(*by_patch_ray.entry((patch, ray)).or_insert(dir), dir);
    }

    let fill_zero = traced(&scene, &a, 0.0);
    assert!(
        of(Gather::Patch, &fill_zero).is_empty(),
        "fill 0 traces no bounce"
    );
    assert!(of(Gather::Probe, &fill_zero) == *probes);
}

/// `source` with `//` comments removed.
fn code(source: &str) -> String {
    source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `fn name` in `source`, signature through its closing brace at the same
/// indentation, comments removed.
fn function(source: &str, name: &str) -> String {
    let start = source
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("no fn {name}"));
    let line_start = source[..start].rfind('\n').map_or(0, |at| at + 1);
    let indent: String = source[line_start..start]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    let close = format!("\n{indent}}}\n");
    let end = start
        + source[start..]
            .find(&close)
            .unwrap_or_else(|| panic!("fn {name} never closes"))
        + close.len();
    code(&source[start..end])
}

// No gather-direction source reads cache radiance. The
// probe gather's set comes from `probe_gather_directions` over
// `sphere_directions`, made once per volume bake; a patch's from
// `cosine_directions`, turned by `patch_frame` about its normal by an angle
// from its seed id (`Patch::seed_id`, `patch_seed_id`).
// `fold_gather_constants` keys the gather's constants. None of them reads
// patch radiance, a layer, the bounce materials or a light, and each call
// site passes only constants and patch geometry.
#[test]
fn no_gather_direction_source_reads_cache_radiance() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let read = |path: &str| std::fs::read_to_string(src.join(path)).expect(path);
    let (trace, probe, sh_bake, bounce, patches) = (
        read("sh_bake/trace.rs"),
        read("sh_bake/probe.rs"),
        read("sh_bake.rs"),
        read("surface_radiance/bounce.rs"),
        read("surface_radiance/patches.rs"),
    );
    let sources = [
        ("sphere_directions", function(&trace, "sphere_directions")),
        (
            "probe_gather_directions",
            function(&probe, "probe_gather_directions"),
        ),
        (
            "fold_gather_constants",
            function(&sh_bake, "fold_gather_constants"),
        ),
        ("cosine_directions", function(&bounce, "cosine_directions")),
        ("patch_frame", function(&bounce, "patch_frame")),
        ("Patch::seed_id", function(&patches, "seed_id")),
        ("patch_seed_id", function(&patches, "patch_seed_id")),
    ];
    for (name, body) in &sources {
        for forbidden in [
            "radiance",
            "Radiance",
            "layer",
            "Layer",
            "BounceMaterials",
            "bounce_materials",
            "BounceSource",
            "source",
            "albedo",
            "light",
            "Light",
            "fill",
            "cache",
            "Cache",
        ] {
            assert!(!body.contains(forbidden), "{name} reads `{forbidden}`");
        }
    }

    // Each call site passes only constants and patch geometry.
    let calls = |source: &str, call: &str| -> Vec<String> {
        code(source)
            .lines()
            .filter(|line| line.contains(call) && !line.contains(&format!("fn {call}")))
            .map(|line| line.trim().to_owned())
            .collect()
    };
    let probe_sets = calls(&probe, "sphere_directions(");
    assert!(!probe_sets.is_empty());
    for call in &probe_sets {
        assert!(
            call.contains("sphere_directions(RAYS_PER_PROBE, SAMPLING_LATTICE_OFFSET)"),
            "a probe set from anything but its constants: {call}"
        );
    }
    assert_eq!(
        calls(&sh_bake, "probe_gather_directions("),
        ["let directions = probe_gather_directions();"]
    );
    assert_eq!(
        calls(&bounce, "cosine_directions("),
        ["let directions = cosine_directions(BOUNCE_RAYS_PER_PATCH);"]
    );
    assert_eq!(
        calls(&bounce, "patch_frame("),
        ["let [tangent, bitangent, normal] = patch_frame(patch.normal, patch.seed_id());"]
    );
}
