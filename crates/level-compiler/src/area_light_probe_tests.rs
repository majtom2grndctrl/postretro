// Area lights at probe points and patches: direct SH projects over the polygons (analytic L0–L2,
// control-variate shadows), billboard scatter stores the term facing Φ̂, patches take the clipped
// term, the mover probe channel's own tile and q integrate over the polygons, and every consumer
// applies the reach window and reaches past the centroid's ball.
// See: context/lib/build_pipeline.md §Compiler pipeline (Area lights)

use std::collections::HashSet;
use std::f64::consts::PI;

use bvh::bvh::Bvh;
use glam::{DVec3, Vec3};
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

use crate::area_light::reach_window;
use crate::area_light::test_lights::{area_light, ceiling_panel, horizontal_rect, pillar_sides};
use crate::billboard_direct_scatter_bake::visible_radiance_sum;
use crate::bvh_build::{BvhPrimitive, build_bvh};
use crate::direct_sh_bake::{DirectBakeInputs, build_reach_index};
use crate::geometry::{FaceIndexRange, GeometryResult};
use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT;
use crate::map_data::MapLight;
use crate::mover_probe_channel::joint_soft_visibility;
use crate::partition::{Aabb, BspLeaf, BspTree};
use crate::sh_bake::{
    RaytracingCtx, ShBakeCtx, ShConfig, area_sh_projection, bake_probe_direct_rgb,
    bake_probe_direct_rgb_posed, evaluate_sh_rgb, incident_radiance_at_point, probe_grid_layout,
    reflected_radiance, segment_clear, soft_visibility_seed, vec3_from,
};
use crate::surface_radiance::{Patch, PatchReachIndex, Side, shade_patch_side};
use crate::texel_footprint::{EmbeddedEmitters, ProbeEarlyOut};

/// The cosine-lobe factors direct SH convolves each band with.
const LOBE: [f64; 9] = [
    PI,
    2.0 * PI / 3.0,
    2.0 * PI / 3.0,
    2.0 * PI / 3.0,
    PI / 4.0,
    PI / 4.0,
    PI / 4.0,
    PI / 4.0,
    PI / 4.0,
];

/// Seed axis and probe index the direct bakes take here; any values do.
const SEED_AXIS: u64 = 5;
const PROBE_INDEX: u64 = 11;

// --- scenes ---------------------------------------------------------------

/// Triangles that only occlude: one face each.
#[derive(Default)]
struct Scene {
    triangles: Vec<[Vec3; 3]>,
}

impl Scene {
    fn quad(&mut self, c: [Vec3; 4]) -> &mut Self {
        self.triangles.push([c[0], c[1], c[2]]);
        self.triangles.push([c[0], c[2], c[3]]);
        self
    }

    /// A horizontal quad over `x` × `z` at height `y`.
    fn horizontal(&mut self, x: [f32; 2], z: [f32; 2], y: f32) -> &mut Self {
        self.quad([
            Vec3::new(x[0], y, z[0]),
            Vec3::new(x[1], y, z[0]),
            Vec3::new(x[1], y, z[1]),
            Vec3::new(x[0], y, z[1]),
        ])
    }

    /// The six faces of an axis-aligned box.
    fn aabb(&mut self, min: Vec3, max: Vec3) -> &mut Self {
        let corner = |i: u32| {
            Vec3::new(
                if i & 1 == 0 { min.x } else { max.x },
                if i & 2 == 0 { min.y } else { max.y },
                if i & 4 == 0 { min.z } else { max.z },
            )
        };
        for [a, b, c, d] in [
            [0, 1, 3, 2],
            [4, 6, 7, 5],
            [0, 4, 5, 1],
            [2, 3, 7, 6],
            [0, 2, 6, 4],
            [1, 5, 7, 3],
        ] {
            self.quad([corner(a), corner(b), corner(c), corner(d)]);
        }
        self
    }

    fn build(&self) -> World {
        let vertex = |p: Vec3| {
            Vertex::new(
                p.to_array(),
                [0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            )
        };
        let geometry = GeometryResult {
            geometry: GeometrySection {
                vertices: self
                    .triangles
                    .iter()
                    .flatten()
                    .map(|&p| vertex(p))
                    .collect(),
                indices: (0..self.triangles.len() as u32 * 3).collect(),
                faces: self
                    .triangles
                    .iter()
                    .map(|_| FaceMeta {
                        leaf_index: 0,
                        texture_index: 0,
                    })
                    .collect(),
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: (0..self.triangles.len() as u32)
                .map(|i| FaceIndexRange {
                    index_offset: i * 3,
                    index_count: 3,
                })
                .collect(),
        };
        let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH");
        World {
            geometry,
            bvh,
            primitives,
        }
    }
}

struct World {
    geometry: GeometryResult,
    bvh: Bvh<f32, 3>,
    primitives: Vec<BvhPrimitive>,
}

impl World {
    fn ctx(&self) -> RaytracingCtx<'_> {
        RaytracingCtx {
            bounce_materials: None,
            bvh: &self.bvh,
            primitives: &self.primitives,
            geometry: &self.geometry,
        }
    }

    fn clear(&self, from: Vec3, to: Vec3) -> bool {
        segment_clear(&self.ctx(), from, to)
    }

    /// Direct SH at `probe` from `light` alone: what id 35 bakes and the
    /// mover probe channel packs as the light's own tile.
    fn direct(&self, probe: Vec3, light: &MapLight) -> [f32; 27] {
        bake_probe_direct_rgb(
            &self.ctx(),
            probe,
            &[light],
            &[SEED_AXIS],
            &[ProbeEarlyOut::Agreed],
            PROBE_INDEX,
        )
    }

    /// The mover probe channel's own tile T at `probe` (`direct::own_tile`
    /// packs exactly this), and its q with a pose that blocks nothing.
    fn mover_own_term(&self, probe: Vec3, light: &MapLight) -> ([f32; 27], u32, u32) {
        let tile = bake_probe_direct_rgb_posed(
            &self.ctx(),
            probe,
            &[light],
            &[SEED_AXIS],
            &[ProbeEarlyOut::Agreed],
            PROBE_INDEX,
            None,
        );
        let (_, lift) = incident_radiance_at_point(light, probe).expect("lit");
        let joint = joint_soft_visibility(
            probe,
            lift,
            light,
            soft_visibility_seed(PROBE_INDEX, 0, SEED_AXIS),
            DEFAULT_AREA_SAMPLE_COUNT,
            ProbeEarlyOut::Agreed,
            |from, to| self.clear(from, to),
            |_, _| false,
        );
        (tile, joint.samples, joint.static_clear)
    }

    /// Billboard scatter's stored radiance at `probe` from `light` alone.
    fn scatter(&self, probe: Vec3, light: &MapLight) -> Vec3 {
        visible_radiance_sum(
            probe,
            [(light, 0, ProbeEarlyOut::Agreed)],
            PROBE_INDEX,
            |from, to| self.clear(from, to),
        )
    }

    /// The patch layer's outgoing radiance on the front of a patch at
    /// `centroid` facing `normal`, or `None` where it records nothing.
    fn patch(&self, centroid: Vec3, normal: Vec3, light: &MapLight) -> Option<Vec3> {
        shade_patch_side(
            &self.ctx(),
            &patch_at(centroid, normal),
            Side::Front,
            light,
            SEED_AXIS,
            ProbeEarlyOut::Agreed,
            |from, to| self.clear(from, to),
        )
    }
}

fn patch_at(centroid: Vec3, normal: Vec3) -> Patch {
    Patch {
        face: 0,
        cell: [0, 0],
        centroid,
        normal,
        area: 0.25,
        texture_index: 0,
    }
}

/// A 4 × 4 m panel flush in a ceiling 1 m above the origin.
fn flush_4x4_panel(reach: f32) -> (World, MapLight) {
    let world = Scene::default()
        .horizontal([-6.0, 6.0], [-6.0, 6.0], 1.0)
        .build();
    let light = area_light(
        vec![ceiling_panel([-2.0, 2.0], [-2.0, 2.0], 1.0)],
        1.0,
        Some(reach),
    );
    (world, light)
}

/// A light wrapping a 0.5 m pillar from 0.5 to 1.5 m up, its faces flush
/// on the pillar's solid box, centroid inside it.
fn pillar_light() -> (World, MapLight) {
    let world = Scene::default()
        .aabb(Vec3::new(-0.25, 0.0, -0.25), Vec3::new(0.25, 2.0, 0.25))
        .horizontal([-4.0, 4.0], [-4.0, 4.0], 0.0)
        .build();
    let light = area_light(pillar_sides(0.0, 0.0, 0.25, [0.5, 1.5]), 1.0, Some(10.0));
    (world, light)
}

// --- the analytic terms each consumer must reach ---------------------------

/// The windowed analytic direct SH of `light` at `probe`, after the lobe,
/// coloured: [band][channel].
fn analytic_sh(light: &MapLight, probe: Vec3) -> [[f64; 3]; 9] {
    let emitter = light.area.as_ref().expect("area light");
    let p = probe.as_dvec3();
    let window = reach_window(emitter.nearest_distance(p), f64::from(light.falloff_range));
    let projection = area_sh_projection(emitter, p);
    std::array::from_fn(|band| {
        let value = projection[band] * LOBE[band] * window * f64::from(light.intensity);
        light.color.map(|c| value * f64::from(c))
    })
}

/// The windowed polygon term at a receiver `(p, n)` per unit colour.
fn windowed_term(light: &MapLight, p: Vec3, n: Vec3) -> f64 {
    let emitter = light.area.as_ref().expect("area light");
    emitter
        .windowed_irradiance(p.as_dvec3(), n.as_dvec3(), f64::from(light.falloff_range))
        .0
        * f64::from(light.intensity)
}

/// The unwindowed polygon term at a receiver `(p, n)` per unit colour.
fn unwindowed_term(light: &MapLight, p: Vec3, n: Vec3) -> f64 {
    let emitter = light.area.as_ref().expect("area light");
    emitter.irradiance(p.as_dvec3(), n.as_dvec3()).0 * f64::from(light.intensity)
}

/// Φ̂ at `p`: the scatter receiver's facing.
fn net_direction(light: &MapLight, p: Vec3) -> Vec3 {
    let emitter = light.area.as_ref().expect("area light");
    emitter
        .irradiance_vector(p.as_dvec3())
        .normalize()
        .as_vec3()
}

/// What a patch facing `n` at `p` reflects from the unoccluded windowed term.
fn patch_reference(light: &MapLight, p: Vec3, n: Vec3) -> Vec3 {
    reflected_radiance(None, 0, Vec3::splat(windowed_term(light, p, n) as f32))
}

fn assert_rel(actual: f64, expected: f64, tolerance: f64, what: &str) {
    let relative = (actual - expected).abs() / expected.abs().max(f64::MIN_POSITIVE);
    assert!(
        relative <= tolerance,
        "{what}: {actual} vs {expected} (relative {relative:e}, tolerance {tolerance:e})"
    );
}

// --- AC 7, 8: direct SH over the polygon ------------------------------------

// AC 7: unoccluded, the probe's L0 is the solid angle and its L1 the
// irradiance vector, analytically: L·Ω·Y₀₀ and L·√(3/4π)·(−Φy, Φz, −Φx),
// each through the cosine lobe, per colour channel.
#[test]
fn unoccluded_probe_coefficients_match_the_analytic_l0_and_l1() {
    let (world, mut light) = flush_4x4_panel(50.0);
    light.intensity = 2.0;
    light.color = [1.0, 0.5, 0.25];
    let probe = Vec3::new(0.6, -0.3, -0.9);
    let acc = world.direct(probe, &light);

    let emitter = light.area.as_ref().unwrap();
    let p = probe.as_dvec3();
    let omega = emitter.solid_angle(p);
    let phi = emitter.irradiance_vector(p);
    let window = reach_window(emitter.nearest_distance(p), 50.0);
    let y00 = 0.5 / PI.sqrt();
    let y1 = (3.0 / (4.0 * PI)).sqrt();
    let scale = f64::from(light.intensity) * window;
    let expected = [
        PI * y00 * omega,
        2.0 * PI / 3.0 * y1 * -phi.y,
        2.0 * PI / 3.0 * y1 * phi.z,
        2.0 * PI / 3.0 * y1 * -phi.x,
    ];
    for (band, value) in expected.into_iter().enumerate() {
        for (channel, colour) in light.color.iter().enumerate() {
            assert_rel(
                f64::from(acc[band * 3 + channel]),
                value * scale * f64::from(*colour),
                1.0e-5,
                &format!("band {band} channel {channel}"),
            );
        }
    }
    // L2 is the second moment's projection, through the same path.
    let analytic = analytic_sh(&light, probe);
    for (band, channels) in analytic.iter().enumerate().skip(4) {
        for (channel, value) in channels.iter().enumerate() {
            let actual = f64::from(acc[band * 3 + channel]);
            assert!(
                (actual - value).abs() <= 1.0e-5 * value.abs().max(1.0e-3),
                "L2 band {band} channel {channel}: {actual} vs {value}"
            );
        }
    }
}

// AC 8: 4 × 4 m panel, probe 1 m below its centre: projected over the
// polygon, the side-facing reconstruction is within 5% of the exact 0.781
// and the down-facing one within 0.05 of 0. The single-direction projection
// read 0.230 and 0.154 there.
#[test]
fn a_4x4_panel_reconstructs_side_and_down_facing_irradiance() {
    let (world, light) = flush_4x4_panel(1.0e3);
    let acc = world.direct(Vec3::ZERO, &light);
    for side in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z] {
        let value = f64::from(evaluate_sh_rgb(&acc, side).x);
        assert_rel(value, 0.781, 0.05, &format!("side {side}"));
    }
    let down = f64::from(evaluate_sh_rgb(&acc, Vec3::NEG_Y).x);
    assert!(down.abs() <= 0.05, "down-facing {down}");
    let up = f64::from(evaluate_sh_rgb(&acc, Vec3::Y).x);
    assert_rel(up, 2.611, 0.02, "up-facing");
}

// Shadows never scale each coefficient by its own ratio: with the panel's
// −x half shadowed, L0 is the analytic value times the clear share of the
// solid-angle samples, and L1 swings toward the lit +x half, which a
// scalar ratio would leave centred. Fully shadowed writes nothing.
#[test]
fn a_shadowed_probe_takes_the_control_variate_not_a_ratio() {
    let light = area_light(
        vec![ceiling_panel([-2.0, 2.0], [-2.0, 2.0], 1.0)],
        1.0,
        Some(1.0e3),
    );
    let mut half = Scene::default();
    half.horizontal([-6.0, 6.0], [-6.0, 6.0], 1.0)
        .horizontal([-3.0, 0.0], [-3.0, 3.0], 0.5);
    let world = half.build();
    let acc = world.direct(Vec3::ZERO, &light);
    let analytic = analytic_sh(&light, Vec3::ZERO);
    let ratio = f64::from(acc[0]) / analytic[0][0];
    assert!(ratio > 0.3 && ratio < 0.7, "half the solid angle: {ratio}");
    // The share is a whole number of the 32 samples.
    let clear = ratio * f64::from(DEFAULT_AREA_SAMPLE_COUNT);
    assert!(
        (clear - clear.round()).abs() < 1.0e-3,
        "L0 is the clear share of the samples: {clear}"
    );
    let east = evaluate_sh_rgb(&acc, Vec3::X).x;
    let west = evaluate_sh_rgb(&acc, Vec3::NEG_X).x;
    assert!(
        east > 1.5 * west,
        "the lit half leads: east {east}, west {west}"
    );

    let mut covered = Scene::default();
    covered
        .horizontal([-6.0, 6.0], [-6.0, 6.0], 1.0)
        .horizontal([-3.0, 3.0], [-3.0, 3.0], 0.5);
    assert_eq!(covered.build().direct(Vec3::ZERO, &light), [0.0; 27]);
}

// --- AC 11: billboard scatter and the mover probe channel -------------------

// AC 11 (restated): 1 m under a 4 × 4 m panel, billboard scatter stores the
// polygon term facing the panel, 2.611, the floor's value, not the centroid
// point value L·A/d² (16); the mover probe channel's own tile reconstructs
// side-facing light within 5% of 0.781 and q reads every sample clear. Far
// off, scatter tends to a point light's I/d².
#[test]
fn scatter_and_the_mover_channel_integrate_a_4x4_panel_over_the_polygon() {
    let (world, light) = flush_4x4_panel(1.0e3);
    let scatter = f64::from(world.scatter(Vec3::ZERO, &light).x);
    assert_rel(scatter, 2.611, 0.01, "scatter 1 m under the panel");
    assert!(scatter < 3.0, "not the centroid point value 16: {scatter}");

    let (tile, samples, clear) = world.mover_own_term(Vec3::ZERO, &light);
    let side = f64::from(evaluate_sh_rgb(&tile, Vec3::X).x);
    assert_rel(side, 0.781, 0.05, "mover channel side-facing");
    assert!(
        f64::from(evaluate_sh_rgb(&tile, Vec3::NEG_Y).x) <= 0.05,
        "down-facing"
    );
    assert_eq!(clear, samples, "q: every sample clear");

    // 40 m under a 1 m² panel the scatter term is I/d², I = L·A.
    let small = area_light(
        vec![ceiling_panel([-0.5, 0.5], [-0.5, 0.5], 1.0)],
        1.0,
        Some(1.0e4),
    );
    let far = Vec3::new(0.0, -39.0, 0.0);
    let point_value = 1.0 / 40.0f64.powi(2);
    let far_world = Scene::default()
        .horizontal([-1.0, 1.0], [-1.0, 1.0], 1.0)
        .build();
    assert_rel(
        f64::from(far_world.scatter(far, &small).x),
        point_value,
        1.0e-3,
        "far limit",
    );
}

// Regression: a probe at the symmetric centre of opposing faces of one light
// has Φ = 0 exactly while Ω > 0, and the light vanished from direct SH,
// scatter and the mover channel, which all normalised Φ. Midway between a
// 2 × 2 m floor panel and its mirror in the ceiling, direct SH stores the
// analytic L0 = Y₀₀·Ω·L·window through the lobe and no L1; scatter stores
// one panel's term for a card facing it; the mover channel's own tile
// matches direct SH and q reads every sample clear.
#[test]
fn a_probe_between_mirrored_faces_of_one_light_keeps_its_light() {
    let world = Scene::default()
        .horizontal([-4.0, 4.0], [-4.0, 4.0], 0.0)
        .horizontal([-4.0, 4.0], [-4.0, 4.0], 2.0)
        .build();
    let light = area_light(
        vec![
            ceiling_panel([-1.0, 1.0], [-1.0, 1.0], 2.0),
            horizontal_rect([-1.0, 1.0], [-1.0, 1.0], 0.0, DVec3::Y),
        ],
        1.0,
        Some(50.0),
    );
    let probe = Vec3::new(0.0, 1.0, 0.0);
    let emitter = light.area.as_ref().unwrap();
    let p = probe.as_dvec3();
    assert_eq!(emitter.irradiance_vector(p), DVec3::ZERO, "Φ cancels");
    assert!(emitter.solid_angle(p) > 0.0);
    // Toward whichever panel subtends more, to the last bit; the ceiling on
    // an exact tie.
    let facing = emitter
        .probe_direction(p)
        .expect("both panels face the probe");
    assert!(facing == DVec3::Y || facing == DVec3::NEG_Y, "{facing}");

    let acc = world.direct(probe, &light);
    let analytic = analytic_sh(&light, probe);
    let y00 = 0.5 / PI.sqrt();
    let window = reach_window(emitter.nearest_distance(p), 50.0);
    let l0 = PI * y00 * emitter.solid_angle(p) * f64::from(light.intensity) * window;
    assert!(l0 > 0.0);
    assert_rel(analytic[0][0], l0, 1.0e-12, "analytic L0");
    assert_rel(f64::from(acc[0]), l0, 1.0e-5, "direct SH L0");
    for value in &acc[3..12] {
        assert!(value.abs() <= 1.0e-5 * l0 as f32, "L1 cancels: {acc:?}");
    }

    let scatter = f64::from(world.scatter(probe, &light).x);
    assert!(scatter > 0.0);
    assert_rel(
        scatter,
        windowed_term(&light, probe, facing.as_vec3()),
        1.0e-5,
        "scatter facing one panel",
    );

    let (tile, samples, clear) = world.mover_own_term(probe, &light);
    assert_eq!(tile, acc, "the mover channel's own tile is direct SH");
    assert!(samples > 0);
    assert_eq!(clear, samples, "q: every sample clear");
}

// --- AC 14: the window in every probe consumer ------------------------------

// AC 14 (probe consumers): on the axis of a 1 m² `light 300` panel at half
// its 16 m reach, the direct-SH probe's L0, the surface-radiance patch,
// billboard scatter and the mover probe channel each read 0.879 of their
// unwindowed term within 1e-3; at reach each reads 0.
#[test]
fn every_probe_consumer_reads_the_window_at_half_reach_and_nothing_at_reach() {
    let window_at_half = (1.0f64 - 1.0 / 16.0).powi(2);
    let panel_at = |height: f32| {
        let world = Scene::default()
            .horizontal([-2.0, 2.0], [-2.0, 2.0], height)
            .build();
        let light = area_light(
            vec![ceiling_panel([-0.5, 0.5], [-0.5, 0.5], f64::from(height))],
            1.0,
            None,
        );
        assert_eq!(light.falloff_range, 16.0);
        (world, light)
    };

    let (world, light) = panel_at(8.0);
    let probe = Vec3::ZERO;
    let unwindowed_l0 = {
        let emitter = light.area.as_ref().unwrap();
        area_sh_projection(emitter, probe.as_dvec3())[0] * PI
    };
    let direct = f64::from(world.direct(probe, &light)[0]) / unwindowed_l0;
    let facing = net_direction(&light, probe);
    let scatter =
        f64::from(world.scatter(probe, &light).x) / unwindowed_term(&light, probe, facing);
    let patch = f64::from(world.patch(probe, Vec3::Y, &light).expect("lit").x)
        / f64::from(reflected_radiance(None, 0, Vec3::ONE).x)
        / unwindowed_term(&light, probe, Vec3::Y);
    let (tile, _, _) = world.mover_own_term(probe, &light);
    let mover = f64::from(tile[0]) / unwindowed_l0;
    for (what, ratio) in [
        ("direct SH L0", direct),
        ("scatter", scatter),
        ("patch", patch),
        ("mover channel", mover),
    ] {
        assert!(
            (ratio - window_at_half).abs() < 1.0e-3,
            "{what}: {ratio} vs {window_at_half}"
        );
    }

    let (world, light) = panel_at(16.0);
    assert_eq!(world.direct(probe, &light), [0.0; 27], "direct SH at reach");
    assert_eq!(world.scatter(probe, &light), Vec3::ZERO, "scatter at reach");
    assert_eq!(world.patch(probe, Vec3::Y, &light), None, "patch at reach");
    assert_eq!(
        bake_probe_direct_rgb_posed(
            &world.ctx(),
            probe,
            &[&light],
            &[SEED_AXIS],
            &[ProbeEarlyOut::Agreed],
            PROBE_INDEX,
            None,
        ),
        [0.0; 27],
        "mover channel at reach"
    );
}

// --- AC 15: a receiver in reach of a corner, not the centroid ---------------

/// An open, one-leaf tree over the whole test world.
fn open_tree() -> BspTree {
    BspTree {
        nodes: Vec::new(),
        leaves: vec![BspLeaf {
            face_indices: Vec::new(),
            bounds: Aabb {
                min: DVec3::splat(-100.0),
                max: DVec3::splat(100.0),
            },
            is_solid: false,
            defining_planes: Vec::new(),
        }],
    }
}

// AC 15 (direct SH and patch, O21): a 20 × 20 m panel reaching 2 m lights a
// wall patch and a probe within reach of its corner, but 15 m from its
// centroid. The patch reach index holds the patch, the probe's affinity cell
// lists the light, and each is lit at the windowed polygon term within 1e-5
// relative.
#[test]
fn receivers_in_reach_of_a_corner_but_not_the_centroid_take_the_windowed_term() {
    let reach = 2.0;
    let light = area_light(
        vec![ceiling_panel([-10.0, 10.0], [-10.0, 10.0], 3.0)],
        1.0,
        Some(reach),
    );
    let mut scene = Scene::default();
    scene
        .horizontal([-12.0, 12.0], [-12.0, 12.0], 3.0)
        .horizontal([-12.0, 12.0], [-12.0, 12.0], 0.0);
    let world = scene.build();
    let centroid = light.origin;

    // A wall patch facing back toward the panel, past its corner.
    let wall = Vec3::new(10.8, 2.0, 10.8);
    let normal = Vec3::new(-1.0, 0.0, -1.0).normalize();
    let emitter = light.area.as_ref().unwrap();
    assert!(emitter.nearest_distance(wall.as_dvec3()) < f64::from(reach));
    assert!(wall.as_dvec3().distance(centroid) > f64::from(reach) + 12.0);
    let patches = [
        patch_at(Vec3::new(0.0, 0.0, 0.0), Vec3::Y),
        patch_at(wall, normal),
        patch_at(Vec3::new(-11.0, 0.0, 4.0), Vec3::Y),
    ];
    let in_reach = PatchReachIndex::new(&patches).in_reach(&light);
    assert!(
        in_reach.contains(&1),
        "the corner patch is in reach: {in_reach:?}"
    );
    let lit = world.patch(wall, normal, &light).expect("lit");
    let expected = patch_reference(&light, wall, normal);
    assert!(expected.x > 0.0);
    assert_rel(
        f64::from(lit.x),
        f64::from(expected.x),
        1.0e-5,
        "corner patch",
    );

    // The probe nearest the corner that lies in reach of it.
    let static_lights = StaticBakedLights::from_lights(std::slice::from_ref(&light));
    let animated_lights = AnimatedBakedLights::from_lights(&[]);
    let tree = open_tree();
    let exterior = HashSet::new();
    let sh_ctx = ShBakeCtx {
        bounce_materials: None,
        bvh: &world.bvh,
        primitives: &world.primitives,
        geometry: &world.geometry,
        tree: &tree,
        exterior_leaves: &exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: 1,
    };
    let config = ShConfig { probe_spacing: 1.0 };
    let layout = probe_grid_layout(&sh_ctx, &config);
    let probe = (0..layout.total_probes())
        .filter(|&i| layout.validity[i] != 0)
        .find(|&i| {
            let p = layout.probe_positions[i];
            emitter.nearest_distance(p) < 0.9 * f64::from(reach)
                && emitter.polygons[0].signed_distance(p) > 0.1
                && p.distance(centroid) > f64::from(reach) + 12.0
        })
        .expect("a probe in reach of the corner only");
    let embedded = EmbeddedEmitters::default();
    let inputs = DirectBakeInputs {
        sh_ctx: &sh_ctx,
        portals: &[],
        embedded_emitters: &embedded,
    };
    let index = build_reach_index(&inputs, &[&light], config.probe_spacing);
    let [nx, ny, _] = layout.dims.map(|d| d as usize);
    let (px, py, pz) = (probe % nx, (probe / nx) % ny, probe / (nx * ny));
    let cell = index.cell_for_probe(px as u32, py as u32, pz as u32);
    assert_eq!(
        index.cell_lights[cell],
        vec![0],
        "the probe's cell lists it"
    );
    let position = vec3_from(layout.probe_positions[probe]);
    let l0 = world.direct(position, &light)[0];
    assert!(l0 > 0.0);
    assert_rel(
        f64::from(l0),
        analytic_sh(&light, position)[0][0],
        1.0e-5,
        "corner probe L0",
    );
}

// --- AC 19: flush panel and pillar, nothing between -------------------------

// AC 19: under a flush ceiling panel and beside a light wrapping all four
// sides of a pillar, with nothing between, the direct-SH probe's L0, the
// surface-radiance patch, billboard scatter and the mover probe channel each
// match their unoccluded polygon term within 1%.
#[test]
fn flush_panel_and_pillar_probe_consumers_match_the_unoccluded_term() {
    let flush = {
        let world = Scene::default()
            .horizontal([-4.0, 4.0], [-4.0, 4.0], 2.0)
            .horizontal([-4.0, 4.0], [-4.0, 4.0], 0.0)
            .build();
        let light = area_light(
            vec![ceiling_panel([-0.5, 0.5], [-0.5, 0.5], 2.0)],
            1.0,
            Some(10.0),
        );
        (
            world,
            light,
            Vec3::new(0.3, 1.0, 0.2),
            Vec3::new(0.6, 0.0, -0.4),
            Vec3::Y,
        )
    };
    let (pillar_world, pillar) = pillar_light();
    let beside = (
        pillar_world,
        pillar,
        Vec3::new(1.0, 1.0, 0.3),
        Vec3::new(1.2, 1.0, 0.0),
        Vec3::NEG_X,
    );
    for (name, (world, light, probe, patch, normal)) in [("flush", flush), ("pillar", beside)] {
        let l0 = f64::from(world.direct(probe, &light)[0]);
        assert_rel(
            l0,
            analytic_sh(&light, probe)[0][0],
            0.01,
            &format!("{name} direct SH L0"),
        );

        let facing = net_direction(&light, probe);
        assert_rel(
            f64::from(world.scatter(probe, &light).x),
            windowed_term(&light, probe, facing),
            0.01,
            &format!("{name} scatter"),
        );

        let lit = world.patch(patch, normal, &light).expect("lit");
        assert_rel(
            f64::from(lit.x),
            f64::from(patch_reference(&light, patch, normal).x),
            0.01,
            &format!("{name} patch"),
        );

        let (tile, samples, clear) = world.mover_own_term(probe, &light);
        assert_rel(
            f64::from(tile[0]),
            analytic_sh(&light, probe)[0][0],
            0.01,
            &format!("{name} mover channel"),
        );
        assert_eq!(clear, samples, "{name}: q reads every sample clear");
    }
}
