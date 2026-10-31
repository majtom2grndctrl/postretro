// Later-bounce tests: bounce-lit rooms, per-bounce energy, and the fill's weighting of id 34.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::{DVec3, Vec3};

use super::bounce::{BOUNCE_RAYS_PER_PATCH, LATER_BOUNCE_COUNT, add_later_bounces, later_bounces};
use super::gather::{PatchGather, PatchLookup};
use super::patches::{PatchSet, Side};
use super::sum::PatchRadiance;
use super::test_scene::{Scene, point, room_lights};
use super::tests::polygon_geometry;
use crate::bake_control::BakeControl;
use crate::geometry::GeometryResult;
use crate::map_data::MapLight;
use crate::sh_bake::bounce_parity_tests::{bc6h_error, interior_rms, rgba16f_tiles};
use crate::sh_bake::{
    RadianceSource, ShConfig, bake_sh_volume_with_source, probe_grid_layout, static_light_refs,
};
use crate::surface_materials::stats::{BounceMaterials, MaterialStats};
use crate::texel_footprint::NO_EMBEDDED_EMITTERS;

/// A wall at `x` over `y` and `z`, facing +x when `positive`.
fn x_quad(x: f32, y: [f32; 2], z: [f32; 2], positive: bool) -> Vec<Vec3> {
    let ([y0, y1], [z0, z1]) = (y, z);
    oriented(
        [[x, y0, z0], [x, y1, z0], [x, y1, z1], [x, y0, z1]],
        positive,
    )
}

/// A floor or ceiling at `y` over `x` and `z`, facing +y when `positive`.
fn y_quad(y: f32, x: [f32; 2], z: [f32; 2], positive: bool) -> Vec<Vec3> {
    let ([x0, x1], [z0, z1]) = (x, z);
    oriented(
        [[x0, y, z0], [x0, y, z1], [x1, y, z1], [x1, y, z0]],
        positive,
    )
}

/// A wall at `z` over `x` and `y`, facing +z when `positive`.
fn z_quad(z: f32, x: [f32; 2], y: [f32; 2], positive: bool) -> Vec<Vec3> {
    let ([x0, x1], [y0, y1]) = (x, y);
    oriented(
        [[x0, y0, z], [x1, y0, z], [x1, y1, z], [x0, y1, z]],
        positive,
    )
}

/// The quad as wound (front on the positive axis), or reversed.
fn oriented(corners: [[f32; 3]; 4], positive: bool) -> Vec<Vec3> {
    let mut quad: Vec<Vec3> = corners.into_iter().map(Vec3::from).collect();
    if !positive {
        quad.reverse();
    }
    quad
}

/// The six inward faces of the box `min..max`.
pub(super) fn inward_box(min: [f32; 3], max: [f32; 3]) -> Vec<Vec<Vec3>> {
    let (x, y, z) = ([min[0], max[0]], [min[1], max[1]], [min[2], max[2]]);
    vec![
        y_quad(min[1], x, z, true),
        y_quad(max[1], x, z, false),
        x_quad(min[0], y, z, true),
        x_quad(max[0], y, z, false),
        z_quad(min[2], x, y, true),
        z_quad(max[2], x, y, false),
    ]
}

const HEIGHT: [f32; 2] = [0.0, 3.0];

/// A 12 × 3 × 4 m hall split by two 0.5 m partitions into room A
/// (x 0..5.5), a corridor (x 6..8) and room B (x 8.5..12). The first
/// partition leaves a gap at its north end (z 3..4), the second at its
/// south end (z 0..1), so room B sees the corridor but nothing of room A.
/// Faces split where the partitions meet them, as brush sides do.
pub(super) fn two_rooms() -> GeometryResult {
    let mut faces = Vec::new();
    for (x, z) in [
        ([0.0, 5.5], [0.0, 4.0]),
        ([5.5, 6.0], [3.0, 4.0]),
        ([6.0, 8.0], [0.0, 4.0]),
        ([8.0, 8.5], [0.0, 1.0]),
        ([8.5, 12.0], [0.0, 4.0]),
    ] {
        faces.push(y_quad(HEIGHT[0], x, z, true));
        faces.push(y_quad(HEIGHT[1], x, z, false));
    }
    faces.extend([
        x_quad(0.0, HEIGHT, [0.0, 4.0], true),
        x_quad(12.0, HEIGHT, [0.0, 4.0], false),
        z_quad(0.0, [0.0, 5.5], HEIGHT, true),
        z_quad(0.0, [6.0, 12.0], HEIGHT, true),
        z_quad(4.0, [0.0, 8.0], HEIGHT, false),
        z_quad(4.0, [8.5, 12.0], HEIGHT, false),
        // The first partition, x 5.5..6, z 0..3.
        x_quad(5.5, HEIGHT, [0.0, 3.0], false),
        x_quad(6.0, HEIGHT, [0.0, 3.0], true),
        z_quad(3.0, [5.5, 6.0], HEIGHT, true),
        // The second partition, x 8..8.5, z 1..4.
        x_quad(8.0, HEIGHT, [1.0, 4.0], false),
        x_quad(8.5, HEIGHT, [1.0, 4.0], true),
        z_quad(1.0, [8.0, 8.5], HEIGHT, false),
    ]);
    polygon_geometry(&faces)
}

/// Room A's one light, whose range stops short of the first partition.
pub(super) fn room_a_light() -> MapLight {
    point([3.0, 1.5, 3.2], 0.1, 2.5)
}

/// The compact slot of the probe standing at `target` (1 m spacing).
fn slot_at(scene: &Scene, lights: &[MapLight], target: [f64; 3]) -> usize {
    scene.with_ctx(lights, |ctx| {
        let layout = probe_grid_layout(ctx, &ShConfig { probe_spacing: 1.0 });
        let target = DVec3::from(target);
        let index = (0..layout.total_probes())
            .find(|&i| layout.probe_positions[i].distance(target) < 1.0e-6)
            .expect("a probe stands at the target");
        assert_ne!(layout.validity[index], 0, "the probe is valid");
        layout.validity[..index].iter().filter(|&&v| v != 0).count()
    })
}

fn tile_total(tile: &[[f32; 3]]) -> f64 {
    tile.iter().flatten().map(|&v| f64::from(v)).sum()
}

/// Bounce one with no build cache: the stage's layers, summed.
fn bounce_one(scene: &Scene, lights: &[MapLight]) -> (PatchSet, PatchRadiance) {
    let patches = PatchSet::build(&scene.geometry);
    let radiance = scene.with_ctx(lights, |ctx| {
        PatchRadiance::single_bounce(
            &ctx.ray_ctx(),
            &patches,
            &static_light_refs(ctx),
            &NO_EMBEDDED_EMITTERS,
            &BakeControl::unrestricted(),
        )
    });
    (patches, radiance)
}

/// Each later bounce of `first`, in order.
fn each_later_bounce(
    scene: &Scene,
    lights: &[MapLight],
    patches: &PatchSet,
    first: &PatchRadiance,
) -> Vec<Vec<[f32; 3]>> {
    scene.with_ctx(lights, |ctx| {
        later_bounces(&ctx.ray_ctx(), patches, first, &BakeControl::unrestricted())
    })
}

/// Area-weighted energy of bounce one, both sides, summed over channels.
fn first_energy(patches: &PatchSet, radiance: &PatchRadiance) -> f64 {
    (0..patches.len() as u32)
        .flat_map(|patch| Side::BOTH.map(|side| (patch, side)))
        .map(|(patch, side)| {
            f64::from(patches.patches()[patch as usize].area)
                * f64::from(radiance.get(patch, side).element_sum())
        })
        .sum()
}

/// Area-weighted energy of one later bounce (front sides), summed over
/// channels.
fn later_energy(patches: &PatchSet, front: &[[f32; 3]]) -> f64 {
    patches
        .patches()
        .iter()
        .zip(front)
        .map(|(patch, value)| {
            f64::from(patch.area) * value.iter().map(|&v| f64::from(v)).sum::<f64>()
        })
        .sum()
}

/// Each bounce's area-weighted energy, bounce one first.
fn bounce_energies(scene: &Scene, lights: &[MapLight]) -> Vec<f64> {
    let (patches, first) = bounce_one(scene, lights);
    let mut energies = vec![first_energy(&patches, &first)];
    for front in each_later_bounce(scene, lights, &patches, &first) {
        energies.push(later_energy(&patches, &front));
    }
    energies
}

fn ratios(energies: &[f64]) -> Vec<f64> {
    energies.windows(2).map(|pair| pair[1] / pair[0]).collect()
}

// AC 6: the corridor takes no direct light and sees room A only through
// the gap, so its probe's single bounce comes from room A's faces alone.
// Later bounces light the corridor's own faces too, and the probe reads
// brighter.
#[test]
fn a_room_lit_only_by_bounce_reads_brighter_with_later_bounces() {
    let scene = Scene::of(two_rooms());
    let lights = [room_a_light()];
    let (patches, first) = bounce_one(&scene, &lights);
    for (index, patch) in patches.patches().iter().enumerate() {
        if patch.centroid.x > 5.5 {
            for side in Side::BOTH {
                assert_eq!(
                    first.get(index as u32, side),
                    Vec3::ZERO,
                    "no direct light past room A: {patch:?}"
                );
            }
        }
    }
    let slot = slot_at(&scene, &lights, [7.0, 1.0, 3.0]);
    let single = tile_total(&rgba16f_tiles(&scene.bake_at(&lights, 0.0, None, 4).0)[slot]);
    let multi = tile_total(&rgba16f_tiles(&scene.bake_at(&lights, 1.0, None, 4).0)[slot]);
    eprintln!("corridor probe: fill 1 / fill 0 = {:.3}", multi / single);
    assert!(
        single > 0.0,
        "the corridor probe sees room A through the gap"
    );
    assert!(multi > 1.05 * single, "{multi} vs single bounce {single}");
}

// AC 6: room B's probe sees the corridor through the second gap and
// nothing of room A, so every face its rays hit lies beyond every light's
// reach: it reads black at fill 0 and later bounces alone at fill 1.
#[test]
fn a_probe_seeing_only_faces_beyond_every_lights_reach_reads_later_bounces_alone() {
    let scene = Scene::of(two_rooms());
    let lights = [room_a_light()];
    let slot = slot_at(&scene, &lights, [10.0, 1.0, 3.0]);
    let single = &rgba16f_tiles(&scene.bake_at(&lights, 0.0, None, 4).0)[slot];
    let multi = &rgba16f_tiles(&scene.bake_at(&lights, 1.0, None, 4).0)[slot];
    assert!(
        single.iter().flatten().all(|&v| v == 0.0),
        "room B's probe reads black at fill 0"
    );
    assert!(
        tile_total(multi) > 0.0,
        "room B's probe reads later bounces"
    );
    assert!(multi.iter().flatten().all(|v| v.is_finite()));
}

/// A 20 × 20 m floor open to the sky, with a low kerb standing on it.
pub(super) fn open_floor() -> GeometryResult {
    let (x, z, y) = ([-4.0, -2.0], [-1.0, -0.5], [0.0, 0.5]);
    polygon_geometry(&[
        y_quad(0.0, [-10.0, 10.0], [-10.0, 10.0], true),
        y_quad(y[1], x, z, true),
        x_quad(x[0], y, z, false),
        x_quad(x[1], y, z, true),
        z_quad(z[0], x, y, false),
        z_quad(z[1], x, y, true),
    ])
}

// AC 6: in open space the floor sees only sky and a low kerb, so the
// later bounces barely move a probe there: less than the BC6H encode error
// the parity rows bound each probe by (the bar the parity harness takes).
#[test]
fn a_probe_in_direct_lit_open_space_moves_less_than_its_bc6h_bar() {
    let scene = Scene::of(open_floor());
    let lights = [point([0.0, 4.0, 0.0], 0.2, 12.0)];
    let (patches, first) = bounce_one(&scene, &lights);
    let later = each_later_bounce(&scene, &lights, &patches, &first);
    assert!(
        later_energy(&patches, &later[0]) > 0.0,
        "the kerb and the floor exchange later bounces"
    );
    let slot = slot_at(&scene, &lights, [-3.0, 1.0, -3.0]);
    let single = scene.bake_at(&lights, 0.0, None, 4).0;
    let multi = scene.bake_at(&lights, 1.0, None, 4).0;
    let (single_tile, multi_tile) = (&rgba16f_tiles(&single)[slot], &rgba16f_tiles(&multi)[slot]);
    let bar = bc6h_error(&single)[slot];
    let moved = interior_rms(multi_tile, single_tile);
    eprintln!("open-space probe: moved {moved:e} RMS against a BC6H bar of {bar:e}");
    assert!(tile_total(single_tile) > 0.0, "the probe is lit");
    assert!(bar > 0.0);
    assert!(moved < bar, "moved {moved} against the bar {bar}");
}

// AC 7, research P8: below albedo one (grey 0.45 everywhere) each bounce
// carries less area-weighted energy than the one before.
#[test]
fn each_added_bounce_adds_less_area_weighted_energy_below_albedo_one() {
    let scene = Scene::new();
    let energies = bounce_energies(&scene, &room_lights());
    let ratios = ratios(&energies);
    eprintln!("grey room per-bounce energy ratios E(k+1)/E(k): {ratios:.4?}");
    assert_eq!(energies.len(), 1 + LATER_BOUNCE_COUNT as usize);
    assert!(energies[0] > 0.0);
    for (bounce, ratio) in ratios.iter().enumerate() {
        assert!(
            *ratio > 0.0 && *ratio < 1.0,
            "bounce {} over bounce {}: {ratio}",
            bounce + 2,
            bounce + 1
        );
    }
}

/// Patch discretisation's allowance on a lossless enclosure: each later
/// bounce's energy may exceed the previous one's by this fraction. Patches
/// read their neighbours at bilinear blends of 0.5 m cells and gather over
/// 64 fixed cosine rays, so a bounce that should conserve energy exactly
/// lands near it, not on it (0.2% over at most on the white room below).
const LOSSLESS_ENERGY_TOLERANCE: f64 = 0.01;

// AC 7, research P8: a closed room of albedo-1 faces neither loses energy
// through a gap nor gains it beyond the discretisation tolerance, and its
// fill-1 id 34 stays finite.
#[test]
fn a_closed_albedo_one_room_bakes_finite_and_never_gains_energy() {
    let white = BounceMaterials::new(vec![MaterialStats::with_snapped_mean([255; 3])], 1.0)
        .expect("white materials");
    assert_eq!(white.resolved_albedo(0), Some(Vec3::ONE));
    let scene = Scene::of(polygon_geometry(&inward_box(
        [0.0, 0.0, 0.0],
        [4.0, 3.0, 5.0],
    )))
    .with_materials(white);
    let lights = [point([1.3, 2.1, 1.7], 0.2, 6.0)];
    let energies = bounce_energies(&scene, &lights);
    let ratios = ratios(&energies);
    eprintln!("closed white room per-bounce energy ratios E(k+1)/E(k): {ratios:.4?}");
    for (bounce, ratio) in ratios.iter().enumerate() {
        assert!(
            ratio.is_finite() && *ratio <= 1.0 + LOSSLESS_ENERGY_TOLERANCE,
            "bounce {} over bounce {}: {ratio}",
            bounce + 2,
            bounce + 1
        );
    }
    let section = scene.bake_at(&lights, 1.0, None, 4).0;
    let tiles = rgba16f_tiles(&section);
    assert!(tiles.iter().flatten().flatten().all(|v| v.is_finite()));
    assert!(tiles.iter().any(|tile| tile_total(tile) > 0.0));
}

// AC 8: at fill 0 the stage traces no later bounce and bakes the bytes the
// single-bounce gather bakes from bounce one alone.
#[test]
fn fill_zero_bakes_the_single_bounce_bytes_and_traces_no_later_bounce() {
    let scene = Scene::new();
    let lights = room_lights();
    let (fill_zero, counts) = scene.bake_at(&lights, 0.0, None, 4);
    assert_eq!(counts.bounce_rays, 0, "{counts:?}");
    let (patches, first) = bounce_one(&scene, &lights);
    let single = scene.with_ctx(&lights, |ctx| {
        let gather = PatchGather::new(&patches, &first, PatchLookup::Bilinear);
        bake_sh_volume_with_source(
            ctx,
            &ShConfig { probe_spacing: 1.0 },
            &BakeControl::unrestricted(),
            RadianceSource::Patches(&gather),
        )
    });
    assert_eq!(fill_zero.compact_atlas, single.compact_atlas);
    assert_eq!(fill_zero.probes, single.probes);
}

// AC 8: at fill 1 each patch's front side ships bounce one plus every later
// bounce unscaled, its back side bounce one alone, and id 34 is the gather
// over exactly that.
#[test]
fn fill_one_bakes_bounce_one_plus_the_unscaled_later_bounces() {
    let scene = Scene::new();
    let lights = room_lights();
    let (patches, first) = bounce_one(&scene, &lights);
    let bounces = each_later_bounce(&scene, &lights, &patches, &first);
    let mut shipped = first.clone();
    scene.with_ctx(&lights, |ctx| {
        add_later_bounces(
            &ctx.ray_ctx(),
            &patches,
            &mut shipped,
            1.0,
            &BakeControl::unrestricted(),
        );
    });
    for patch in 0..patches.len() as u32 {
        let later: Vec3 = bounces.iter().fold(Vec3::ZERO, |sum, bounce| {
            sum + Vec3::from(bounce[patch as usize])
        });
        assert_eq!(
            shipped.get(patch, Side::Front),
            first.get(patch, Side::Front) + later,
            "patch {patch}"
        );
        assert_eq!(shipped.get(patch, Side::Back), first.get(patch, Side::Back));
    }
    assert_ne!(shipped, first, "later bounces add light");

    let (fill_one, counts) = scene.bake_at(&lights, 1.0, None, 4);
    assert_eq!(
        counts.bounce_rays,
        patches.len() as u64 * u64::from(BOUNCE_RAYS_PER_PATCH * LATER_BOUNCE_COUNT),
        "every patch gathers every bounce"
    );
    let summed = scene.with_ctx(&lights, |ctx| {
        let gather = PatchGather::new(&patches, &shipped, PatchLookup::Bilinear);
        bake_sh_volume_with_source(
            ctx,
            &ShConfig { probe_spacing: 1.0 },
            &BakeControl::unrestricted(),
            RadianceSource::Patches(&gather),
        )
    });
    assert_eq!(fill_one.compact_atlas, summed.compact_atlas);
    assert_eq!(fill_one.probes, summed.probes);
}

// AC 8: between fills, every probe's pre-encode (RGBA16F) irradiance reads
// between its fill-0 and fill-1 values, per texel and channel, within one
// f16 step of the rounding each end took.
#[test]
fn a_fill_between_reads_between_fill_zero_and_one_per_channel() {
    let scene = Scene::new();
    let lights = room_lights();
    let tiles = |fill| rgba16f_tiles(&scene.bake_at(&lights, fill, None, 4).0);
    let (zero, half, one) = (tiles(0.0), tiles(0.5), tiles(1.0));
    let mut strictly_between = 0usize;
    for (slot, ((z, h), o)) in zero.iter().zip(&half).zip(&one).enumerate() {
        for ((z, h), o) in z
            .iter()
            .flatten()
            .zip(h.iter().flatten())
            .zip(o.iter().flatten())
        {
            let (lo, hi) = (z.min(*o), z.max(*o));
            let step = lo.abs().max(hi.abs()) / 1024.0 + 1.0e-7;
            assert!(
                *h >= lo - step && *h <= hi + step,
                "slot {slot}: fill 0.5 reads {h} outside [{lo}, {hi}]"
            );
            strictly_between += usize::from(*h > lo && *h < hi);
        }
    }
    assert!(strictly_between > 0, "the fill moves some probe");
}

// AC 9's fixture premise (`tests/sh_base_warm_cold.rs`'s corridor, in
// metres): a probe 38 m down a 48.8 m corridor, farther from the west end's
// one light than its 4 m range plus the retired grouped bake's 16 m reach
// cutoff, reads that light's bounce: the later bounces carry it down the
// corridor to the patches the probe gathers. (Bounce one alone reaches it
// through almost no ray: the lit end subtends a sliver of its sphere.) A
// warm rebuild serves that probe the cold bytes.
#[test]
fn a_probe_reads_patches_lit_beyond_the_retired_16_m_group_reach() {
    let unit = 0.0254;
    let scene = Scene::of(polygon_geometry(&inward_box(
        [0.0, 0.0, 0.0],
        [1920.0 * unit, 128.0 * unit, 128.0 * unit],
    )));
    let lights = [point([f64::from(64.0 * unit), 1.6, 1.6], 0.1, 160.0 * unit)];
    let probe = [40.0, 1.0, 1.0];
    // Past the light's range and the 16 m cutoff, with a whole 4-probe
    // group (16 m at the CLI suite's 4 m spacing) of slack besides.
    let reach = f64::from(lights[0].falloff_range);
    assert!(DVec3::from(probe).distance(lights[0].origin) > reach + 16.0 + 16.0);
    let slot = slot_at(&scene, &lights, probe);
    let (cold, _) = scene.bake(&lights, None, 4);
    assert!(
        tile_total(&rgba16f_tiles(&cold)[slot]) > 0.0,
        "the far probe gathers light from the west end"
    );
    let dir = tempfile::tempdir().unwrap();
    let cache = crate::cache::StageCache::new(dir.path()).unwrap();
    scene.bake(&lights, Some(&cache), 4);
    assert_eq!(scene.bake(&lights, Some(&cache), 4).0, cold);
}
