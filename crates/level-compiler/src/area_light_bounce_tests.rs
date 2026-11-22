// Animated area lights in the per-hit bounce and the delta bake (id 27): the one-sample polygon
// arm averages to the weighted visibility, the delta matches the static patch bounce, and the
// affinity cells and per-probe clip read the reach sphere.
// See: context/lib/build_pipeline.md §Compiler pipeline (Area lights)

use glam::{DVec3, Vec3};
use postretro_level_format::delta_sh_volumes::{DeltaShVolumesSection, PROBES_PER_CELL};
use postretro_level_format::probe_depth_tiles::MAX_PROBE_DEPTH_TILES;

use crate::affinity_grid::AFFINITY_FACTOR;
use crate::area_light::test_lights::{area_light, ceiling_panel};
use crate::bake_control::BakeControl;
use crate::delta_sh_bake::{DeltaBakeInputs, bake_delta_sh_volumes};
use crate::fixture_pipeline::{FixturePipeline, load_map};
use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::lightmap_bake::{DEFAULT_AREA_SAMPLE_COUNT, soft_visibility_one_sample};
use crate::map_data::{LightAnimation, MapLight};
use crate::reflection_probes::placement_test_fixtures::{UNIT, entity, sealed_room};
use crate::sh_bake::bounce_parity_tests::{Tiles, delta_base_slots, delta_tiles, rgba16f_tiles};
use crate::sh_bake::{ShBakeCtx, ShConfig, probe_is_valid_pub, soft_visibility_seed};
use crate::texel_footprint::NO_EMBEDDED_EMITTERS;

// --- AC 35 (research.md O23): the one-sample polygon arm ---------------------

/// Seeds the one-sample arm is averaged over, beside every lattice index.
const SEEDS: u64 = 64;

/// A 4 × 4 m panel at height 1 m over the origin, facing down, reaching far.
fn panel_over_origin() -> MapLight {
    area_light(
        vec![ceiling_panel([-2.0, 2.0], [-2.0, 2.0], 1.0)],
        1.0,
        Some(50.0),
    )
}

/// The one-sample visibility at `(point, normal)` for every lattice index
/// under each of [`SEEDS`] production seeds, `clear(target)` standing in for
/// the shadow ray.
fn one_sample_values(
    light: &MapLight,
    point: Vec3,
    normal: Vec3,
    clear: impl Fn(Vec3) -> bool,
) -> Vec<f64> {
    let count = DEFAULT_AREA_SAMPLE_COUNT;
    (0..SEEDS)
        .flat_map(|ray| (0..count).map(move |index| (ray, index)))
        .map(|(ray, index)| {
            let seed = soft_visibility_seed(3, ray, 7);
            let value = soft_visibility_one_sample(
                point,
                normal,
                light,
                seed,
                |lattice| {
                    assert_eq!(lattice, count);
                    index
                },
                count,
                |_, to| clear(to),
            );
            f64::from(value)
        })
        .collect()
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

/// Dense midpoint quadrature of cosθ_e·cosθ_r / r² over the part of the
/// down-facing panel at height 1 (x, z ∈ [−2, 2]) where `keep(x)` holds, for
/// a receiver at `p` facing `n`, its horizon clipping each element.
fn panel_quadrature(p: DVec3, n: DVec3, keep: impl Fn(f64) -> bool) -> f64 {
    let steps = 1200;
    let cell = 4.0 / f64::from(steps);
    let mut sum = 0.0;
    for i in 0..steps {
        let x = -2.0 + (f64::from(i) + 0.5) * cell;
        if !keep(x) {
            continue;
        }
        for k in 0..steps {
            let z = -2.0 + (f64::from(k) + 0.5) * cell;
            let to = DVec3::new(x, 1.0, z) - p;
            let r2 = to.length_squared();
            let dir = to / r2.sqrt();
            let cos_e = dir.dot(DVec3::Y).max(0.0);
            let cos_r = dir.dot(n).max(0.0);
            sum += cos_e * cos_r / r2 * cell * cell;
        }
    }
    sum
}

// AC 35: at an open bounce hit 1 m under a 4 × 4 m panel, the polygon arm's
// one-sample visibility averages to 1 within 1e-2 over every lattice index
// and 64 seeds, though no single value is 0 or 1. With the panel's far half
// occluded it averages the dense-quadrature ratio of the shadowed to the
// unshadowed term. A hit whose horizon clips half the panel averages 1 too.
#[test]
fn the_one_sample_polygon_arm_averages_to_the_weighted_visibility() {
    let light = panel_over_origin();
    // 1 m under the panel, 1 m toward its x < 0 half: the near half.
    let (point, up) = (Vec3::new(-1.0, 0.0, 0.0), Vec3::Y);

    let open = one_sample_values(&light, point, up, |_| true);
    let open_mean = mean(&open);
    assert!(
        (open_mean - 1.0).abs() <= 1.0e-2,
        "open: mean {open_mean}, not 1"
    );
    assert!(
        open.iter().all(|&v| v != 0.0 && v != 1.0),
        "an open single value read exactly 0 or 1"
    );
    let (lo, hi) = open
        .iter()
        .fold((f64::INFINITY, 0.0_f64), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    assert!(hi - lo > 0.2, "single values barely vary: {lo}..{hi}");

    let far_blocked = one_sample_values(&light, point, up, |to| to.x < 0.0);
    let (p, n) = (point.as_dvec3(), up.as_dvec3());
    let ratio = panel_quadrature(p, n, |x| x < 0.0) / panel_quadrature(p, n, |_| true);
    let blocked_mean = mean(&far_blocked);
    assert!(
        (blocked_mean - ratio).abs() <= 1.0e-2,
        "far half occluded: mean {blocked_mean}, quadrature ratio {ratio}"
    );
    assert!(ratio > 0.6 && ratio < 0.95, "the halves differ: {ratio}");

    // A face whose horizon cuts the panel in half: samples behind it weigh 0.
    let side = one_sample_values(&light, Vec3::ZERO, Vec3::X, |_| true);
    let side_mean = mean(&side);
    eprintln!(
        "open {open_mean} (singles {lo}..{hi}); far half blocked {blocked_mean} vs {ratio}; horizon-clipped {side_mean}"
    );
    assert!(
        (side_mean - 1.0).abs() <= 1.0e-2,
        "horizon-clipped: mean {side_mean}, not 1"
    );
    assert!(side.contains(&0.0), "no sample fell behind the horizon");
}

// --- room fixtures -----------------------------------------------------------

/// A sealed room spanning `0..max` map units, compiled through the
/// production front end. Its geometry vertices span the interior, so the SH
/// grid starts at the interior's engine-space minimum.
fn sealed_room_fixture(max: [f64; 3]) -> FixturePipeline {
    let dir = tempfile::tempdir().expect("temp dir");
    let map = dir.path().join("room.map");
    let text = format!(
        "{}\n{{\n\"classname\" \"info_player_start\"\n\"origin\" \"32 32 24\"\n}}\n",
        entity("worldspawn", "", &sealed_room([0.0; 3], max))
    );
    std::fs::write(&map, text).expect("write room map");
    load_map(&map)
}

/// The geometry's engine-space vertex bounds: the SH grid's origin and far corner.
fn geometry_bounds(fixture: &FixturePipeline) -> (DVec3, DVec3) {
    fixture.geometry.geometry.vertices.iter().fold(
        (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
        |(lo, hi), v| {
            let p = Vec3::from(v.position).as_dvec3();
            (lo.min(p), hi.max(p))
        },
    )
}

/// A 4 × 4 m panel flush in the ceiling at `ceiling` m, centred on `(x, z)`.
fn flush_panel(x: f64, z: f64, ceiling: f64, intensity: f32, reach: Option<f32>) -> MapLight {
    area_light(
        vec![ceiling_panel([x - 2.0, x + 2.0], [z - 2.0, z + 2.0], ceiling)],
        intensity,
        reach,
    )
}

/// `light` animated at a constant brightness of 1: curve value 1 every frame.
fn animated(light: &MapLight) -> MapLight {
    let mut light = light.clone();
    light.animation = Some(LightAnimation {
        period: 1.0,
        phase: 0.0,
        brightness: Some(vec![1.0, 1.0]),
        color: None,
        direction: None,
        start_active: true,
    });
    light
}

fn delta_section(
    fixture: &FixturePipeline,
    lights: &[MapLight],
    config: &ShConfig,
) -> DeltaShVolumesSection {
    let animated_lights = AnimatedBakedLights::from_lights(lights);
    assert_eq!(animated_lights.len(), 1, "the panel is the one animated light");
    bake_delta_sh_volumes(
        &DeltaBakeInputs {
            bounce_materials: None,
            bvh: &fixture.bvh,
            primitives: &fixture.primitives,
            geometry: &fixture.geometry,
            tree: &fixture.tree,
            exterior_leaves: &fixture.exterior_leaves,
            portals: &fixture.portals,
            animated_lights: &animated_lights,
        },
        config,
    )
    .expect("an animated light bakes a delta section")
}

/// Sum over every texel and channel of `tile`, each channel times `scale`.
fn tile_sum(tile: &[[f32; 3]], scale: [f32; 3]) -> [f64; 3] {
    tile.iter().fold([0.0; 3], |sum, texel| {
        std::array::from_fn(|c| sum[c] + f64::from(texel[c]) * f64::from(scale[c]))
    })
}

// --- AC 34: the animated delta matches the static bounce -----------------------

// AC 34: on a map with `_bounce_fill 0`, an animated flush ceiling panel's SH
// bounce delta at curve 1 (unit-radiance transport times the light's colour
// and radiance), summed over the probes in its reach, matches the same light
// baked static, whose bounce reaches the SH base through its surface-radiance
// patch layer, within 2% per channel.
#[test]
fn an_animated_panels_bounce_delta_matches_its_static_bounce() {
    let fixture = sealed_room_fixture([256.0, 256.0, 128.0]);
    let (lo, hi) = geometry_bounds(&fixture);
    let centre = (lo + hi) * 0.5;
    let static_light = flush_panel(centre.x, centre.z, hi.y, 2.0, None);
    assert!(
        (hi.y - 128.0 * UNIT).abs() < 1.0e-6,
        "the panel lies in the ceiling's plane"
    );
    assert!(
        f64::from(static_light.falloff_range) > (hi - lo).length(),
        "every probe in the room lies in the panel's reach"
    );
    let config = ShConfig {
        probe_spacing: crate::sh_bake::DEFAULT_PROBE_SPACING,
    };
    let control = BakeControl::unrestricted();

    // Static: the production SH base at fill 0, bounce one alone.
    let static_lights_vec = vec![static_light.clone()];
    let static_lights = StaticBakedLights::from_lights(&static_lights_vec);
    let no_animated = AnimatedBakedLights::from_lights(&[]);
    let ctx = ShBakeCtx {
        bounce_materials: None,
        bvh: &fixture.bvh,
        primitives: &fixture.primitives,
        geometry: &fixture.geometry,
        tree: &fixture.tree,
        exterior_leaves: &fixture.exterior_leaves,
        static_lights: &static_lights,
        animated_lights: &no_animated,
        total_light_count: 1,
    };
    let refs = vec![&static_light];
    let (handoff, _) = crate::surface_radiance::plan_sh_base(
        &fixture.geometry,
        &refs,
        0,
        config.probe_spacing,
        0.0,
        u64::MAX,
    )
    .expect("the SH base fits");
    let base = crate::surface_radiance::bake_sh_base(
        &ctx,
        &config,
        handoff,
        &NO_EMBEDDED_EMITTERS,
        None,
        &control,
        MAX_PROBE_DEPTH_TILES,
    )
    .section;
    let base_tiles: Tiles = rgba16f_tiles(&base);

    // Animated: the id-27 delta, at curve 1.
    let animated_lights_vec = vec![animated(&static_light)];
    let delta = delta_section(&fixture, &animated_lights_vec, &config);
    let deltas = delta_tiles(&delta);
    let slots = delta_base_slots(&delta, &base);
    let scale = static_light
        .color
        .map(|c| c * static_light.intensity);

    let (mut animated_sum, mut static_sum) = ([0.0; 3], [0.0; 3]);
    let mut paired = 0;
    for (tile, slot) in deltas.iter().zip(&slots) {
        let lit = tile_sum(tile, scale);
        let Some(slot) = *slot else {
            assert_eq!(lit, [0.0; 3], "a lit delta probe has no base probe");
            continue;
        };
        let reference = tile_sum(&base_tiles[slot], [1.0; 3]);
        for c in 0..3 {
            animated_sum[c] += lit[c];
            static_sum[c] += reference[c];
        }
        paired += 1;
    }
    let valid_base = base.probes.iter().filter(|p| p.validity != 0).count();
    assert_eq!(paired, valid_base, "every valid probe holds the panel's delta");
    for c in 0..3 {
        let relative = animated_sum[c] / static_sum[c] - 1.0;
        eprintln!(
            "channel {c}: delta {} vs static {} over {paired} probes ({relative:+.4})",
            animated_sum[c], static_sum[c]
        );
        assert!(static_sum[c] > 0.0, "the static panel bounces nothing");
        assert!(
            relative.abs() <= 0.02,
            "channel {c}: delta {} vs static {} ({relative:+.4})",
            animated_sum[c],
            static_sum[c]
        );
    }
}

// --- AC 36 (research.md O24): affinity cells read the reach sphere -------------

// AC 36: a 4 × 4 m animated panel with `_falloff_range 3` holds a
// bounce-delta entry for every affinity cell with a valid probe within 3 m
// of its polygon, and that probe's sub-block is baked, not clipped. One such
// cell lies wholly beyond 3.5 m of the centroid, outside the box a centroid
// ball of the reach and the 0.5 m padding would keep.
#[test]
fn a_reach_3_panel_holds_a_delta_entry_for_every_cell_within_3_m_of_it() {
    // Interior about 12.0 × 2.4 × 6.0 m: engine x from map y, z from map x.
    let fixture = sealed_room_fixture([236.0, 472.0, 96.0]);
    let (lo, hi) = geometry_bounds(&fixture);
    let spacing = crate::sh_bake::DEFAULT_PROBE_SPACING;
    let cell_m = f64::from(spacing) * f64::from(AFFINITY_FACTOR);
    // The first cell along x ends 3.6 m short of the centroid.
    let centroid_x = lo.x + cell_m + 3.6;
    let centre_z = (lo.z + hi.z) * 0.5;
    assert!(centroid_x + 2.0 < hi.x, "the panel fits the room");
    let static_light = flush_panel(centroid_x, centre_z, hi.y, 1.0, Some(3.0));
    let lights = vec![animated(&static_light)];
    let emitter = lights[0].area.as_ref().expect("area light");
    let config = ShConfig {
        probe_spacing: spacing,
    };
    let delta = delta_section(&fixture, &lights, &config);
    let deltas = delta_tiles(&delta);

    let [nx, ny, nz] = delta.affinity_dims;
    let mut checked = 0;
    let mut beyond_centroid_box = 0;
    for cell in 0..nx * ny * nz {
        let corner = [cell % nx, (cell / nx) % ny, cell / (nx * ny)];
        let cell_min = lo + DVec3::from(corner.map(f64::from)) * cell_m;
        let entries = delta.affinity_offsets[cell as usize] as usize
            ..delta.affinity_offsets[cell as usize + 1] as usize;
        let entry = entries.into_iter().find(|&e| delta.affinity_lights[e] == 0);
        for local in 0..PROBES_PER_CELL as u32 {
            let offset = [
                local % AFFINITY_FACTOR,
                (local / AFFINITY_FACTOR) % AFFINITY_FACTOR,
                local / (AFFINITY_FACTOR * AFFINITY_FACTOR),
            ];
            let probe = cell_min + DVec3::from(offset.map(f64::from)) * f64::from(spacing);
            let near = emitter.nearest_distance(probe) < 3.0;
            if !near || !probe_is_valid_pub(&fixture.tree, &fixture.exterior_leaves, probe) {
                continue;
            }
            let entry = entry.unwrap_or_else(|| {
                panic!("cell {cell} holds a probe {probe} within 3 m of the panel but no entry")
            });
            let tile = &deltas[entry * PROBES_PER_CELL + local as usize];
            assert!(
                tile_sum(tile, [1.0; 3]).iter().all(|&v| v > 0.0),
                "probe {probe} within 3 m of the panel bakes no bounce"
            );
            checked += 1;
            // The cell's box, every point of it more than 3.5 m from the
            // centroid along x.
            let cell_max_x = cell_min.x + cell_m;
            if cell_max_x < emitter.centroid.x - 3.5 {
                beyond_centroid_box += 1;
                assert!(probe.distance(emitter.centroid) > 3.5);
            }
        }
    }
    assert!(checked > 0, "no probe within 3 m of the panel");
    assert!(
        beyond_centroid_box > 0,
        "no probe within 3 m of the panel lies in a cell beyond 3.5 m of its centroid"
    );
}
