use std::sync::Arc;

use glam::{DVec3, Vec3};

use super::posed_solid::PosedSolid;
use super::posed_solid::box_brush;
use super::{JointVisibility, PosedOccluder};
use crate::bvh_build::build_bvh;
use crate::geometry::{FaceIndexRange, GeometryResult};
use crate::lightmap_bake::{DEFAULT_AREA_SAMPLE_COUNT, RAY_EPSILON, area_sample_target};
use crate::map_data::{
    BrushVolume, FalloffModel, KinematicMoveMode, LightType, MapKinematicMover,
    MapKinematicWaypoint, MapLight, ShadowType,
};
use crate::mover_light_gate::{MoverPoses, Terminus};
use crate::sh_bake::{RaytracingCtx, soft_visibility_seed};
use crate::sh_bake::{bake_probe_direct_rgb_posed, probe_direct_joint_visibility};
use crate::texel_footprint::ProbeEarlyOut;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

/// A mover holding `brushes` at its first waypoint, which sits at the
/// origin; its last waypoint is 100 m up, out of every test's way.
fn occluder(brushes: Vec<BrushVolume>) -> PosedOccluder {
    PosedOccluder::new(Arc::new(solid(brushes)))
}

/// [`occluder`]'s mover posed at its first waypoint, as a solid.
pub(super) fn solid(brushes: Vec<BrushVolume>) -> PosedSolid {
    let mover = MapKinematicMover {
        mover_id: 0,
        name: "mover".to_string(),
        tags: Vec::new(),
        origin: DVec3::ZERO,
        authored_origin: None,
        path: "first".to_string(),
        speed: 1.0,
        wait_ms: 0.0,
        spin_axis: [0.0; 3],
        spin_speed_deg_s: 0.0,
        spin_accel_deg_s2: 0.0,
        carry_yaw: false,
        block_policy: "displace".to_string(),
        crush_damage: 0.0,
        crush_interval_ms: 0.0,
        auto_close_ms: None,
        open_event: None,
        close_event: None,
        blocked_event: None,
        crush_event: None,
        open_sound: None,
        close_sound: None,
        blocked_sound: None,
        crush_sound: None,
        move_mode: KinematicMoveMode::Once,
        start_on_spawn: false,
        brush_volumes: brushes,
        crush_damage_type: None,
        light_ramp_start: 0.0,
        light_ramp_end: 1.0,
        light_bounces: None,
        light_poses: None,
    };
    let waypoints = [
        ("first", "last", DVec3::ZERO),
        ("last", "", DVec3::Y * 100.0),
    ]
    .map(|(name, next, origin)| MapKinematicWaypoint {
        name: name.to_string(),
        next: next.to_string(),
        origin,
    });
    let poses = MoverPoses::resolve(std::slice::from_ref(&mover), &waypoints);
    let pose = poses.mover(0).expect("posed").pose(Terminus::First);
    PosedSolid::new(&mover, pose, &TextureNamesSection { names: Vec::new() })
}

fn soft_light(origin: DVec3, light_size: f32) -> MapLight {
    MapLight {
        origin,
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: 50.0,
        light_size,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: None,
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: vec![],
        shadow_type: ShadowType::StaticLightMap,
    }
}

fn geometry_of(triangles: &[[[f32; 3]; 3]]) -> GeometryResult {
    let vertex = |position| {
        Vertex::new(
            position,
            [0.0; 2],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0; 2],
            0,
        )
    };
    GeometryResult {
        geometry: GeometrySection {
            vertices: triangles.iter().flatten().map(|&p| vertex(p)).collect(),
            indices: (0..triangles.len() as u32 * 3).collect(),
            faces: triangles
                .iter()
                .map(|_| FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                })
                .collect(),
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: (0..triangles.len() as u32)
            .map(|face| FaceIndexRange {
                index_offset: face * 3,
                index_count: 3,
            })
            .collect(),
    }
}

/// A probe at the origin under a 0.6 m emitter 3 m up. A static triangle at
/// y 2 hides the emitter's −x side; the posed slab at y 1.5 hides its +z
/// side. Both cut through the penumbra, so the sample set escalates and each
/// trace blocks a different part of it.
struct Scene {
    geometry: GeometryResult,
    light: MapLight,
    occluder: PosedOccluder,
}

const PROBE: Vec3 = Vec3::ZERO;
const PROBE_INDEX: u64 = 7;
const GLOBAL_LIGHT_INDEX: u64 = 3;

fn scene(slab_min: DVec3, slab_max: DVec3) -> Scene {
    Scene {
        geometry: geometry_of(&[[[-0.1, 2.0, -50.0], [-0.1, 2.0, 50.0], [-50.0, 2.0, 0.0]]]),
        light: soft_light(DVec3::new(0.0, 3.0, 0.0), 0.6),
        occluder: occluder(vec![box_brush(slab_min, slab_max)]),
    }
}

fn split_scene() -> Scene {
    scene(DVec3::new(-5.0, 1.5, 0.05), DVec3::new(5.0, 1.6, 5.0))
}

/// L0 red of the probe's direct-SH bake for the scene's one light, with
/// and without the pose.
fn baked_l0(scene: &Scene, posed: bool) -> f32 {
    let (bvh, primitives, _) = build_bvh(&scene.geometry).expect("bvh");
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &scene.geometry,
    };
    bake_probe_direct_rgb_posed(
        &ctx,
        PROBE,
        &[&scene.light],
        &[GLOBAL_LIGHT_INDEX],
        &[ProbeEarlyOut::Agreed],
        PROBE_INDEX,
        posed.then_some(&scene.occluder),
    )[0]
}

fn joint(scene: &Scene) -> JointVisibility {
    let (bvh, primitives, _) = build_bvh(&scene.geometry).expect("bvh");
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &scene.geometry,
    };
    let l = (scene.light.origin.as_vec3() - PROBE).normalize();
    probe_direct_joint_visibility(
        &ctx,
        PROBE,
        l,
        &scene.light,
        soft_visibility_seed(PROBE_INDEX, 0, GLOBAL_LIGHT_INDEX),
        ProbeEarlyOut::Agreed,
        &scene.occluder,
    )
}

/// Each trace counted over every sample of the full lattice, by hand.
fn brute_force_counts(scene: &Scene) -> (u32, u32) {
    let (bvh, primitives, _) = build_bvh(&scene.geometry).expect("bvh");
    let l = (scene.light.origin.as_vec3() - PROBE).normalize();
    let origin = PROBE + l * RAY_EPSILON;
    let seed = soft_visibility_seed(PROBE_INDEX, 0, GLOBAL_LIGHT_INDEX);
    let (mut static_clear, mut posed_clear) = (0, 0);
    for i in 0..DEFAULT_AREA_SAMPLE_COUNT {
        let target = area_sample_target(PROBE, &scene.light, seed, i, DEFAULT_AREA_SAMPLE_COUNT);
        if crate::ray_traversal::segment_clear(&bvh, &primitives, &scene.geometry, origin, target) {
            static_clear += 1;
            if !scene.occluder.blocks(origin, target) {
                posed_clear += 1;
            }
        }
    }
    (static_clear, posed_clear)
}

// Q is the posed clear count over the joint set's static clear
// count, same seed, and the posed bake scales the light's term by its byte.
#[test]
fn posed_direct_bake_scales_a_light_by_its_joint_sample_surviving_fraction() {
    let scene = split_scene();
    let (static_clear, posed_clear) = brute_force_counts(&scene);
    assert!(
        0 < posed_clear && posed_clear < static_clear && static_clear < DEFAULT_AREA_SAMPLE_COUNT,
        "fixture must block part of the emitter under each trace: static {static_clear}, posed {posed_clear}"
    );
    let joint = joint(&scene);
    assert_eq!(
        joint,
        JointVisibility {
            samples: DEFAULT_AREA_SAMPLE_COUNT,
            static_clear,
            posed_clear,
        }
    );
    let expected_q = posed_clear as f32 / static_clear as f32;
    let baked_q = baked_l0(&scene, true) / baked_l0(&scene, false);
    assert!(
        (baked_q - expected_q).abs() <= 1.0 / 510.0 + 1.0e-6,
        "baked q {baked_q} against joint q {expected_q}"
    );
    assert!(joint.meets_mover());
}

// A light whose samples never meet the posed mover stores no
// entry, and the posed bake leaves its term bit for bit.
#[test]
fn a_light_whose_samples_miss_the_pose_keeps_its_term_and_meets_no_mover() {
    let scene = scene(DVec3::new(10.0, 1.5, 10.0), DVec3::new(11.0, 1.6, 11.0));
    let joint = joint(&scene);
    assert!(!joint.meets_mover());
    assert_eq!(joint.surviving_byte(), 255);
    assert_eq!(
        baked_l0(&scene, true).to_bits(),
        baked_l0(&scene, false).to_bits()
    );
}

// A mover that covers the whole emitter stores q = 0 even where the four
// probe samples agree, and a hard emitter's one ray gives q of 0 or 1.
#[test]
fn a_sealing_pose_stores_zero_and_a_hard_light_stores_zero_or_one() {
    let mut sealed = scene(DVec3::new(-5.0, 1.5, -5.0), DVec3::new(5.0, 1.6, 5.0));
    sealed.geometry = geometry_of(&[]);
    assert_eq!(joint(&sealed).surviving_byte(), 0);
    assert_eq!(baked_l0(&sealed, true), 0.0);

    let mut hard = split_scene();
    hard.light.light_size = 0.0;
    hard.geometry = geometry_of(&[]);
    let hard_joint = joint(&hard);
    assert_eq!(hard_joint.samples, 1);
    // The hard ray runs straight up the y axis, past the slab's z ≥ 0.05 edge.
    assert_eq!(hard_joint.surviving_byte(), 255);
}

// The per-ray inside-brush rule: every segment from a probe inside one of
// the mover's brushes starts in solid, so even a light no other brush hides
// is blocked. The stored state is the enclosing-terminus rule's (the probe
// reads the base there: `direct_tests` and `mover_bounce_stage_tests`); this rule
// still decides the probe's posed rays and so its dark depth.
#[test]
fn a_segment_from_inside_a_posed_brush_is_blocked_toward_every_light() {
    let containing = box_brush(DVec3::splat(-0.5), DVec3::splat(0.5));
    let beside = box_brush(DVec3::new(2.0, -1.0, -1.0), DVec3::new(2.2, 1.0, 1.0));
    let up = soft_light(DVec3::new(0.0, 3.0, 0.0), 0.0);
    let east = soft_light(DVec3::new(4.0, 0.0, 0.0), 0.0);
    let occluder = occluder(vec![containing, beside]);
    let blocked = |light: &MapLight| {
        let from = PROBE + (light.origin.as_vec3() - PROBE).normalize() * RAY_EPSILON;
        occluder.blocks(from, light.origin.as_vec3())
    };
    assert!(blocked(&up), "no other brush hides the light above");
    assert!(blocked(&east));
}

/// The unit box's six faces, two triangles each, built from its corners
/// rather than from brush sides: the box as a static brush the tracer meets.
fn static_unit_box() -> GeometryResult {
    let corner = |x: u8, y: u8, z: u8| [f32::from(x), f32::from(y), f32::from(z)];
    let quads = [
        [
            corner(1, 0, 0),
            corner(1, 1, 0),
            corner(1, 1, 1),
            corner(1, 0, 1),
        ],
        [
            corner(0, 0, 0),
            corner(0, 0, 1),
            corner(0, 1, 1),
            corner(0, 1, 0),
        ],
        [
            corner(0, 1, 0),
            corner(0, 1, 1),
            corner(1, 1, 1),
            corner(1, 1, 0),
        ],
        [
            corner(0, 0, 0),
            corner(1, 0, 0),
            corner(1, 0, 1),
            corner(0, 0, 1),
        ],
        [
            corner(0, 0, 1),
            corner(1, 0, 1),
            corner(1, 1, 1),
            corner(0, 1, 1),
        ],
        [
            corner(0, 0, 0),
            corner(0, 1, 0),
            corner(1, 1, 0),
            corner(1, 0, 0),
        ],
    ];
    let triangles: Vec<[[f32; 3]; 3]> = quads
        .iter()
        .flat_map(|[a, b, c, d]| [[*a, *b, *c], [*a, *c, *d]])
        .collect();
    geometry_of(&triangles)
}

// The one posed-occluder predicate: a posed brush meets shadow segments and
// gather rays exactly as the static tracer meets the same brush placed
// statically, with τ = 0.5 mm and its grazing-ray rule. A segment or ray
// passing 0.3 mm outside a face's edge is blocked, 0.7 mm outside is clear,
// and one running parallel to a face 0.3 mm off it is blocked only where it
// crosses another face's plane within τ. Growing the posed planes by τ would
// block the parallel segment that crosses no such plane.
#[test]
fn a_posed_brush_meets_grazing_segments_and_rays_as_the_static_tracer_does() {
    let posed = solid(vec![box_brush(DVec3::ZERO, DVec3::ONE)]);
    let geometry = static_unit_box();
    let (bvh, primitives, _) = build_bvh(&geometry).expect("bvh");
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
    };
    let segment = |from: Vec3, to: Vec3| {
        let static_blocked =
            !crate::ray_traversal::segment_clear(&bvh, &primitives, &geometry, from, to);
        let posed_blocked = posed.segment_blocked(from, to);
        assert_eq!(
            posed_blocked, static_blocked,
            "segment {from} → {to}: posed {posed_blocked}, static {static_blocked}"
        );
        posed_blocked
    };
    let ray = |origin: Vec3, dir: Vec3| {
        let static_hit = crate::sh_bake::gather_hit(&ctx, origin, dir).map(|hit| hit.distance);
        let posed_hit = posed
            .nearest_hit(origin, dir, f32::INFINITY)
            .map(|hit| hit.distance);
        match (static_hit, posed_hit) {
            (None, None) => None,
            (Some(a), Some(b)) => {
                assert!(
                    (a - b).abs() <= 1.0e-6,
                    "ray {origin} along {dir}: posed hit {b}, static {a}"
                );
                Some(b)
            }
            _ => panic!("ray {origin} along {dir}: posed {posed_hit:?}, static {static_hit:?}"),
        }
    };
    const GRAZE: f32 = 3.0e-4;
    const MISS: f32 = 7.0e-4;

    // Down past the +x face, crossing the top and bottom planes just
    // outside their edges.
    let down = |x: f32| (Vec3::new(x, 2.0, 0.5), Vec3::new(x, -1.0, 0.5));
    let (from, to) = down(1.0 + GRAZE);
    assert!(segment(from, to), "0.3 mm outside the edge is within τ");
    assert!(ray(from, Vec3::NEG_Y).is_some());
    let (from, to) = down(1.0 + MISS);
    assert!(!segment(from, to), "0.7 mm outside the edge is past τ");
    assert!(ray(from, Vec3::NEG_Y).is_none());

    // Obliquely past the corner edge x = 1, z = 1, crossing each side's
    // plane 0.3 mm outside the other side.
    let corner = Vec3::new(1.0 + GRAZE * 0.5, 0.5, 1.0 + GRAZE * 0.5);
    let across = Vec3::new(1.0, 0.0, -1.0).normalize();
    assert!(segment(corner - across * 2.0, corner + across * 2.0));

    // Parallel to the top face, 0.3 mm above it: inside the face's
    // footprint it crosses no plane and is clear; across the whole box it
    // crosses the ±x side planes within τ of their top edges.
    let above = 1.0 + GRAZE;
    assert!(!segment(
        Vec3::new(0.2, above, 0.5),
        Vec3::new(0.8, above, 0.5)
    ));
    assert!(segment(
        Vec3::new(-1.0, above, 0.5),
        Vec3::new(2.0, above, 0.5)
    ));
    let hit = ray(Vec3::new(-1.0, above, 0.5), Vec3::X).expect("the −x side within τ");
    assert!((hit - (1.0 - RAY_EPSILON)).abs() <= 1.0e-5, "hit at {hit}");
    assert!(ray(Vec3::new(0.2, above, 0.5), Vec3::Y).is_none());
}

// One inside-brush rule for every posed trace: a gather ray or shadow
// segment starts in solid when its lifted origin lies inside a posed brush,
// with no contact band. A probe within 0.1 mm of a posed face, on either
// side of it, reads the same verdict in the bounce gather, the depth-only
// trace and the direct half's shadow segments: inside (distance 0, blocked)
// along −x into the brush, outside (no posed hit, clear) along +x away
// from it.
#[test]
fn a_probe_a_hair_off_a_posed_face_reads_one_inside_verdict_in_every_posed_trace() {
    use postretro_level_format::probe_depth_tiles::bordered_tile;

    use super::bounce::PosedRadiance;
    use super::gather::{PosedGather, trace_posed_depth, valid_probes};
    use super::posed_scene::PosedScene;
    use crate::bake_control::BakeControl;
    use crate::sh_bake::{ProbeGridLayout, TileRayTable, probe_gather_directions};
    use crate::surface_radiance::{PatchRadiance, PatchSet};

    const FAR: f32 = 100.0;
    let solid = Arc::new(solid(vec![box_brush(DVec3::ZERO, DVec3::ONE)]));
    let scene = PosedScene::new(Arc::clone(&solid), 0).expect("posed patches");
    let occluder = PosedOccluder::new(Arc::clone(&solid));
    // A static floor far below, so the static tracer has a world to miss.
    let geometry = geometry_of(&[[
        [-50.0, -20.0, -50.0],
        [-50.0, -20.0, 50.0],
        [50.0, -20.0, 0.0],
    ]]);
    let (bvh, primitives, _) = build_bvh(&geometry).expect("bvh");
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
    };
    let patches = PatchSet::try_build(&geometry).expect("static patches");
    let radiance = PosedRadiance {
        world: PatchRadiance::zeroed(patches.len()),
        mover: PatchRadiance::zeroed(scene.patches.len()),
    };
    let gather = PosedGather::new(&patches, &radiance, &scene);
    let directions = probe_gather_directions();
    let rays = TileRayTable::new(&directions);
    for offset in [5.0e-5_f32, -5.0e-5] {
        let probe = Vec3::new(1.0 + offset, 0.5, 0.5);
        for (dir, inside) in [(Vec3::NEG_X, true), (Vec3::X, false)] {
            let (_, distance) = gather.sample(&ctx, probe, dir, FAR);
            assert_eq!(
                distance == 0.0,
                inside,
                "gather ray from x = 1 {offset:+} along {dir}: distance {distance}"
            );
            assert_eq!(
                occluder.blocks(probe, probe + dir * 10.0),
                inside,
                "direct segment from x = 1 {offset:+} along {dir}"
            );
        }
        // The depth-only trace bins the gather's own distances, ray for ray.
        let distances: Vec<f32> = directions
            .iter()
            .map(|&dir| gather.sample(&ctx, probe, dir, FAR).1)
            .collect();
        assert!(
            distances.contains(&0.0) && distances.iter().any(|&d| d > 0.0),
            "x = 1 {offset:+}: only rays whose lifted origin is inside read 0"
        );
        let layout = ProbeGridLayout {
            world_min: DVec3::ZERO,
            dims: [1, 1, 1],
            cell_size: [1.0; 3],
            far_sentinel: FAR,
            probe_positions: vec![probe.as_dvec3()],
            validity: vec![1],
        };
        assert_eq!(
            trace_posed_depth(
                &ctx,
                &layout,
                &scene,
                &valid_probes(&layout),
                &BakeControl::unrestricted(),
            ),
            vec![(0, bordered_tile(&rays.interior(&distances)))],
            "x = 1 {offset:+}: the depth-only trace reads the gather's verdicts"
        );
    }
}

// Enclosure takes the gather's lift as an inset (owner rule, an enclosing
// terminus reads the base). A probe 0.5 mm inside a posed face is not
// enclosed: its rays keep the per-ray rule, those whose lift leaves the face
// reading past it, so its depth is not the dark tile and the pose stores its
// bounce state as for any changed probe. A probe 2 mm inside is enclosed:
// every lifted gather ray starts in solid, so its depth is the dark tile,
// and the pose stores no bounce state for it.
#[test]
fn a_probe_within_the_lift_of_a_face_keeps_the_per_ray_rule_and_one_past_it_is_enclosed() {
    use std::collections::HashSet;

    use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
    use postretro_level_format::octahedral::DEFAULT_IRRADIANCE_TILE_DIMENSION;
    use postretro_level_format::probe_depth_tiles::bordered_tile;
    use postretro_level_format::sh_volume::OctahedralShVolumeSection;

    use super::bounce::PosedRadiance;
    use super::gather::{PosedGather, gather_posed_probes, trace_posed_depth, valid_probes};
    use super::posed_scene::PosedScene;
    use crate::bake_control::BakeControl;
    use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
    use crate::partition::{Aabb, BspLeaf, BspTree};
    use crate::sh_bake::{ProbeGridLayout, ShBakeCtx, TileRayTable, probe_gather_directions};
    use crate::surface_radiance::{PatchRadiance, PatchSet};

    const FAR: f32 = 100.0;
    let solid = Arc::new(solid(vec![box_brush(DVec3::ZERO, DVec3::ONE)]));
    let scene = PosedScene::new(Arc::clone(&solid), 0).expect("posed patches");
    let occluder = PosedOccluder::new(Arc::clone(&solid));
    let geometry = geometry_of(&[[
        [-50.0, -20.0, -50.0],
        [-50.0, -20.0, 50.0],
        [50.0, -20.0, 0.0],
    ]]);
    let (bvh, primitives, _) = build_bvh(&geometry).expect("bvh");
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
    };
    let tree = BspTree {
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
    };
    let exterior = HashSet::new();
    let static_lights = StaticBakedLights::from_lights(&[]);
    let animated_lights = AnimatedBakedLights::from_lights(&[]);
    let sh_ctx = ShBakeCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
        tree: &tree,
        exterior_leaves: &exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: 0,
    };
    let patches = PatchSet::try_build(&geometry).expect("static patches");
    let radiance = PosedRadiance {
        world: PatchRadiance::zeroed(patches.len()),
        mover: PatchRadiance::zeroed(scene.patches.len()),
    };
    let gather = PosedGather::new(&patches, &radiance, &scene);
    let directions = probe_gather_directions();
    let rays = TileRayTable::new(&directions);
    let dark = bordered_tile(&rays.interior(&[0.0; 256]));
    // Id 34 as one uncompressed tile of 1.0: the posed gather, black here,
    // leaves its bar at every texel.
    let dim = DEFAULT_IRRADIANCE_TILE_DIMENSION;
    let one = 0x3c00u16.to_le_bytes();
    let base = OctahedralShVolumeSection {
        grid_dimensions: [1, 1, 1],
        tile_dimension: dim,
        atlas_dimensions: [dim, dim],
        layer_count: 1,
        tiles_per_layer: 1,
        atlas_tiles_per_row: 1,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        compact_atlas: one.repeat(4 * (dim * dim) as usize),
        ..OctahedralShVolumeSection::placeholder()
    };
    for (depth_m, enclosed) in [(5.0e-4_f32, false), (2.0e-3, true)] {
        // `depth_m` inside the box's +x face, deep inside every other.
        let probe = Vec3::new(1.0 - depth_m, 0.5, 0.5);
        assert_eq!(solid.encloses(probe), enclosed, "{depth_m} m inside");
        assert_eq!(scene.encloses(probe), enclosed, "{depth_m} m: the scene");
        assert_eq!(
            occluder.encloses(probe),
            enclosed,
            "{depth_m} m: the occluder"
        );
        assert!(
            solid.contains(probe),
            "{depth_m} m: inside by the no-band test"
        );

        let distances: Vec<f32> = directions
            .iter()
            .map(|&dir| gather.sample(&ctx, probe, dir, FAR).1)
            .collect();
        assert_eq!(
            distances.iter().all(|&d| d == 0.0),
            enclosed,
            "{depth_m} m: every lifted ray starts in solid exactly when enclosed"
        );
        assert!(
            !occluder.blocks(probe, probe + Vec3::X * 10.0) || enclosed,
            "{depth_m} m: a segment out through the near face escapes"
        );
        let layout = ProbeGridLayout {
            world_min: DVec3::ZERO,
            dims: [1, 1, 1],
            cell_size: [1.0; 3],
            far_sentinel: FAR,
            probe_positions: vec![probe.as_dvec3()],
            validity: vec![1],
        };
        let traced = trace_posed_depth(
            &ctx,
            &layout,
            &scene,
            &valid_probes(&layout),
            &BakeControl::unrestricted(),
        );
        assert_eq!(traced.len(), 1, "{depth_m} m: the pose meets the probe");
        assert_eq!(
            traced[0].1 == dark,
            enclosed,
            "{depth_m} m: the depth is the dark tile exactly when enclosed"
        );

        let posed = gather_posed_probes(
            &sh_ctx,
            &layout,
            &gather,
            &base,
            true,
            false,
            &BakeControl::unrestricted(),
        );
        let [posed] = posed.as_slice() else {
            panic!("{depth_m} m: the probe differs from id 34");
        };
        assert_eq!(posed.enclosed, enclosed, "{depth_m} m");
        assert!(posed.difference > posed.bar, "{depth_m} m: past the bar");
        assert_eq!(
            posed.tile.is_some(),
            !enclosed,
            "{depth_m} m: the state is stored exactly when not enclosed"
        );
        assert_eq!(
            posed.depth.as_ref(),
            Some(&traced[0].1),
            "{depth_m} m: the gather bins the depth-only trace's tile"
        );
    }
}
