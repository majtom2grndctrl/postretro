// The direct half's production bake: q against the joint sample set,
// the enclosing-terminus rule, suns and spinners, bake-only lights in
// namespace 2, the own-tile flag against ids 41 and 45 as emitted, term
// records (one tile per cell and light, masked to the probes movers change),
// the per-light memo and worker-count determinism.

use std::collections::HashSet;
use std::sync::Arc;

use glam::{DVec3, Vec3};
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::DEFAULT_DELTA_PROBE_F16_STRIDE;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::mover_probe_channel::{
    DirectRecordView, MoverLightNamespace, MoverProbeChannelSection,
};
use postretro_level_format::texture_names::TextureNamesSection;

use super::blocks::BounceBlocks;
use super::cost::direct_entry_bound;
use super::direct::{
    ChannelLight, DirectEntry, DirectScene, LightEntries, MoverDirectBake, ProbeCells, bake_direct,
    light_movers,
};
use super::direct_memo::{DirectMemo, MemoPrefix, decode, encode};
use super::lights::{ChannelLightSources, channel_lights};
use super::posed_solid::{PosedMovers, box_brush};
use super::section::{EmittedDirectEntries, MoverRecords, channel_section};
use super::visibility::{PosedOccluder, joint_soft_visibility};
use crate::affinity_grid::{AffinityReachInputs, AffinityReachPolicy};
use crate::bake_control::BakeControl;
use crate::bake_counters::counted;
use crate::bvh_build::{BvhPrimitive, build_bvh};
use crate::cache::StageCache;
use crate::geometry::{FaceIndexRange, GeometryResult};
use crate::lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT;
use crate::map_data::{
    BrushVolume, FalloffModel, KinematicMoveMode, LightType, MapKinematicMover,
    MapKinematicWaypoint, MapLight, ShadowType,
};
use crate::mover_light_gate::{MoverPoses, Terminus};
use crate::partition::{Aabb, BspLeaf, BspTree};
use crate::sh_bake::{
    ProbeGridLayout, RaytracingCtx, ShBakeCtx, ShConfig, bake_probe_direct_rgb_posed,
    incident_radiance_at_point, probe_grid_layout, segment_clear, soft_visibility_seed, vec3_from,
};
use crate::texel_footprint::ProbeEarlyOut;

/// Mover `id`, holding `brushes` at its first waypoint, the origin; its path
/// is [`waypoints`]'.
fn mover(id: u32, brushes: Vec<BrushVolume>, spin: bool) -> MapKinematicMover {
    MapKinematicMover {
        mover_id: id,
        name: format!("mover{id}"),
        tags: Vec::new(),
        origin: DVec3::ZERO,
        authored_origin: None,
        path: format!("first{id}"),
        speed: 1.0,
        wait_ms: 0.0,
        spin_axis: if spin { [0.0, 1.0, 0.0] } else { [0.0; 3] },
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
        light_ramp_start: 0.25,
        light_ramp_end: 0.75,
        light_bounces: None,
        light_poses: None,
    }
}

fn waypoints(id: u32, last: DVec3) -> [MapKinematicWaypoint; 2] {
    [
        MapKinematicWaypoint {
            name: format!("first{id}"),
            next: format!("last{id}"),
            origin: DVec3::ZERO,
        },
        MapKinematicWaypoint {
            name: format!("last{id}"),
            next: String::new(),
            origin: last,
        },
    ]
}

fn point_light(origin: DVec3, range: f32, size: f32) -> MapLight {
    MapLight {
        origin,
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0, 0.5, 0.25],
        falloff_model: FalloffModel::Linear,
        falloff_range: range,
        light_size: size,
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

fn sun() -> MapLight {
    MapLight {
        light_type: LightType::Directional,
        cone_direction: Some([0.0, -1.0, 0.0]),
        angular_diameter: 0.5,
        ..point_light(DVec3::ZERO, 0.0, 0.0)
    }
}

/// An 8 m box of probes, 1 m apart, around one static triangle at y 2 that
/// hides the −x side of the world from above. Every probe is valid.
fn geometry() -> GeometryResult {
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
    let mut positions: Vec<[f32; 3]> =
        vec![[-0.1, 2.0, -50.0], [-0.1, 2.0, 50.0], [-50.0, 2.0, 0.0]];
    // Corners that span the grid but join no triangle.
    for corner in 0..8 {
        positions.push([
            if corner & 1 == 0 { -4.0 } else { 4.0 },
            if corner & 2 == 0 { -4.0 } else { 4.0 },
            if corner & 4 == 0 { -4.0 } else { 4.0 },
        ]);
    }
    GeometryResult {
        geometry: GeometrySection {
            vertices: positions.into_iter().map(vertex).collect(),
            indices: vec![0, 1, 2],
            faces: vec![FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            }],
        },
        texture_names: TextureNamesSection {
            names: vec!["base".into()],
        },
        face_index_ranges: vec![FaceIndexRange {
            index_offset: 0,
            index_count: 3,
        }],
    }
}

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

/// The slab under the light at y 1.5–1.6, covering z ≥ 0.05, plus a block
/// around the probe at (2, 0, 2); the last waypoint parks both 100 m up.
fn slab_mover() -> MapKinematicMover {
    mover(
        0,
        vec![
            box_brush(DVec3::new(-5.0, 1.5, 0.05), DVec3::new(5.0, 1.6, 5.0)),
            box_brush(DVec3::new(1.5, -0.5, 1.5), DVec3::new(2.5, 0.5, 2.5)),
        ],
        false,
    )
}

const LAST: DVec3 = DVec3::new(0.0, 100.0, 0.0);

struct World {
    geometry: GeometryResult,
    bvh: bvh::bvh::Bvh<f32, 3>,
    primitives: Vec<BvhPrimitive>,
    tree: BspTree,
    exterior: HashSet<usize>,
    layout: ProbeGridLayout,
    cells: ProbeCells,
}

fn world() -> World {
    world_of(geometry())
}

fn world_of(geometry: GeometryResult) -> World {
    let (bvh, primitives, _) = build_bvh(&geometry).expect("bvh");
    let tree = open_tree();
    let exterior = HashSet::new();
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&[]);
    let animated_lights = crate::light_namespaces::AnimatedBakedLights::from_lights(&[]);
    let layout = probe_grid_layout(
        &ShBakeCtx {
            bounce_materials: None,
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geometry,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 0,
        },
        &ShConfig { probe_spacing: 1.0 },
    );
    let cells = ProbeCells::of_layout(&layout);
    World {
        geometry,
        bvh,
        primitives,
        tree,
        exterior,
        layout,
        cells,
    }
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

    fn scene<'a>(&'a self, vertices: &'a [[f32; 3]]) -> DirectScene<'a> {
        DirectScene {
            ctx: self.ctx(),
            layout: &self.layout,
            cells: &self.cells,
            reach: AffinityReachInputs {
                geometry_vertices: vertices,
                tree: &self.tree,
                exterior_leaves: &self.exterior,
                portals: &[],
                probe_spacing: 1.0,
            },
        }
    }

    fn vertices(&self) -> Vec<[f32; 3]> {
        self.geometry
            .geometry
            .vertices
            .iter()
            .map(|vertex| vertex.position)
            .collect()
    }

    fn probe_at(&self, point: [f64; 3]) -> usize {
        self.layout
            .probe_positions
            .iter()
            .position(|position| position.distance(DVec3::from(point)) < 1.0e-6)
            .expect("a probe at that point")
    }

    fn bake(&self, poses: &PosedMovers, lights: &[ChannelLight<'_>]) -> MoverDirectBake {
        let vertices = self.vertices();
        bake_direct(
            poses,
            lights,
            &self.scene(&vertices),
            None,
            &BakeControl::unrestricted(),
        )
        .expect("direct bake")
    }
}

fn static_light(light: &MapLight, index: u32) -> ChannelLight<'_> {
    ChannelLight {
        namespace: MoverLightNamespace::StaticLight,
        light_index: index,
        baked: light.clone(),
        authored: light,
        source_index: index as usize,
        seed_axis: u64::from(index) + 3,
        early_out: ProbeEarlyOut::Agreed,
        policy: AffinityReachPolicy::DIRECT,
    }
}

fn poses_of(movers: &[MapKinematicMover]) -> PosedMovers {
    let waypoints: Vec<MapKinematicWaypoint> = movers
        .iter()
        .flat_map(|mover| {
            waypoints(
                mover.mover_id,
                if mover.mover_id == 0 {
                    LAST
                } else {
                    DVec3::X * 40.0
                },
            )
        })
        .collect();
    posed(movers, &waypoints)
}

/// `movers` posed along `waypoints` as solids.
fn posed(movers: &[MapKinematicMover], waypoints: &[MapKinematicWaypoint]) -> PosedMovers {
    let poses = MoverPoses::resolve(movers, waypoints);
    PosedMovers::resolve(&poses, movers, &TextureNamesSection { names: Vec::new() })
        .expect("every analysed mover is a map mover")
}

/// The probe's entry payload for `mover`: `(q₀, q₁)` and the probe's own tile.
fn probe_payload<'e>(
    world: &World,
    light: &'e LightEntries,
    mover_id: u32,
    probe: usize,
) -> Option<([u8; 2], &'e [u16])> {
    light.entries.iter().find_map(|entry| {
        if entry.mover_id != mover_id {
            return None;
        }
        let slot = world
            .cells
            .valid_probes(entry.cell as usize)
            .iter()
            .position(|&candidate| candidate == probe)?;
        Some((
            [entry.visibility[2 * slot], entry.visibility[2 * slot + 1]],
            &entry.tiles[slot * DEFAULT_DELTA_PROBE_F16_STRIDE
                ..(slot + 1) * DEFAULT_DELTA_PROBE_F16_STRIDE],
        ))
    })
}

// Every stored q is the posed clear count over the joint
// sample set's static clear count, same seed, and its byte scales the
// light's term in the posed direct bake.
#[test]
fn production_q_is_the_joint_sample_surviving_fraction_at_every_gated_probe() {
    let world = world();
    let light = point_light(DVec3::new(0.0, 3.5, 0.0), 10.0, 0.6);
    let channel = static_light(&light, 4);
    let movers = [slab_mover()];
    let poses = poses_of(&movers);
    let bake = world.bake(&poses, std::slice::from_ref(&channel));
    assert_eq!(bake.lights.len(), 1, "the light under the slab is gated");
    let gated = &bake.lights[0];
    assert_eq!(gated.light_index, 4);

    let first = PosedOccluder::new(Arc::clone(
        &poses.movers()[0].solids[Terminus::First.index()],
    ));
    let ctx = world.ctx();
    let (mut checked, mut partial, mut enclosed) = (0, 0, 0);
    for entry in &gated.entries {
        for (slot, probe) in world
            .cells
            .valid_probes(entry.cell as usize)
            .into_iter()
            .enumerate()
        {
            let point = vec3_from(world.layout.probe_positions[probe]);
            let stored = entry.visibility[2 * slot];
            let Some((radiance, l)) = incident_radiance_at_point(&light, point) else {
                assert_eq!(stored, 255);
                continue;
            };
            if radiance == Vec3::ZERO {
                assert_eq!(stored, 255);
                continue;
            }
            if first.encloses(point) {
                // The enclosing terminus reads the mover-absent term.
                assert_eq!(stored, 255, "probe {probe} is enclosed");
                enclosed += 1;
                continue;
            }
            let joint = joint_soft_visibility(
                point,
                l,
                &light,
                soft_visibility_seed(probe as u64, 0, channel.seed_axis),
                DEFAULT_AREA_SAMPLE_COUNT,
                ProbeEarlyOut::Agreed,
                |from, to| segment_clear(&ctx, from, to),
                |from, to| first.blocks(from, to),
            );
            let expected = if joint.meets_mover() {
                joint.surviving_byte()
            } else {
                255
            };
            assert_eq!(stored, expected, "probe {probe}");
            assert_eq!(
                entry.visibility[2 * slot + 1],
                255,
                "the last pose is 100 m up"
            );
            checked += 1;
            if joint.meets_mover() && joint.posed_clear > 0 {
                partial += 1;
                // The posed direct bake scales the term by the same byte.
                let bake_term = |posed: Option<&PosedOccluder>| {
                    bake_probe_direct_rgb_posed(
                        &ctx,
                        point,
                        &[&light],
                        &[channel.seed_axis],
                        &[ProbeEarlyOut::Agreed],
                        probe as u64,
                        posed,
                    )[0]
                };
                let q = bake_term(Some(&first)) / bake_term(None);
                assert!(
                    (q - f32::from(stored) / 255.0).abs() <= 1.0 / 510.0 + 1.0e-5,
                    "probe {probe}: baked q {q} against stored {stored}"
                );
            }
        }
    }
    assert!(
        checked > 0 && partial > 0 && enclosed == 1,
        "{checked} probes, {partial} in penumbra, {enclosed} enclosed"
    );
}

// A light the mover is a candidate for, whose samples never
// meet it, stores no entry.
#[test]
fn a_light_whose_samples_never_meet_the_pose_stores_no_entry() {
    let world = world();
    let light = point_light(DVec3::new(0.0, -3.0, -3.0), 4.6, 0.1);
    let poses = poses_of(&[slab_mover()]);
    assert!(
        !light_movers(&light, &poses).is_empty(),
        "the light's reach touches the slab's bounds"
    );
    let bake = world.bake(&poses, &[static_light(&light, 0)]);
    assert!(bake.lights.is_empty(), "{:?}", bake.lights);
}

// Enclosing terminus: a valid probe inside a posed
// brush stores q = 255 for that pose and reads the light's mover-absent
// term, though every segment from it starts in solid. With q = 255 at both
// termini it leaves the record's probe mask, which stays tight: exactly the
// probes some mover gates.
#[test]
fn an_enclosed_probe_stores_q_255_and_leaves_the_tight_mask() {
    let world = world();
    let light = point_light(DVec3::new(0.0, 3.5, 0.0), 10.0, 0.6);
    let movers = [slab_mover()];
    let poses = poses_of(&movers);
    let bake = world.bake(&poses, &[static_light(&light, 0)]);
    let inside = world.probe_at([2.0, 0.0, 2.0]);
    let point = vec3_from(world.layout.probe_positions[inside]);
    let first = PosedOccluder::new(Arc::clone(
        &poses.movers()[0].solids[Terminus::First.index()],
    ));
    assert!(first.encloses(point), "the block encloses the probe");
    assert!(
        first.blocks(point, light.origin.as_vec3()),
        "the per-ray rule still blocks its segments"
    );
    let ([q0, q1], _) =
        probe_payload(&world, &bake.lights[0], 0, inside).expect("the slab gates the probe's cell");
    assert_eq!(
        (q0, q1),
        (255, 255),
        "both termini read the mover-absent term"
    );

    let section = channel_section(
        &bake,
        &BounceBlocks::default(),
        &world.cells,
        &MoverRecords::new(&movers, &MoverPoses::default()),
        &super::section::MoverPayloadBars::default(),
        |_, _, _| false,
        |_| true,
    )
    .unwrap()
    .expect("the slab gates the light");
    let mut masked = 0;
    for view in section.direct_record_views() {
        for probe in world.cells.valid_probes(view.cell as usize) {
            let (pair, _) = probe_payload(&world, &bake.lights[0], 0, probe)
                .expect("the entry covers every valid probe of its cell");
            let gated = pair.iter().any(|&q| q < 255);
            assert_eq!(
                masked_slot(&world, &view, probe).is_some(),
                gated,
                "probe {probe}: masked exactly where q < 255"
            );
            masked += usize::from(gated);
        }
    }
    assert!(masked > 0, "the slab gates some probe");
    assert!(
        section
            .direct_record_views()
            .all(|view| masked_slot(&world, &view, inside).is_none()),
        "the enclosed probe stores nothing"
    );
    assert_eq!(
        MoverProbeChannelSection::from_bytes(&section.to_bytes()).expect("decodes"),
        section
    );
}

// Direct half: a mid pose that encloses a probe
// stores no value state there. Its q is 255, never read: the section marks
// that (mover, probe, mid pose) across, once for the (cell, mover), and
// compose blends across it between the termini's q. The termini keep their
// own: here the box shadows the probe at the first pose and has sunk below
// it at the last. Every mark is an enclosed mid pose of a probe the
// mover's record masks, and every such pair is marked.
#[test]
fn a_probe_a_mid_pose_encloses_stores_q_255_and_is_marked_across() {
    let world = world();
    let light = point_light(DVec3::new(0.0, 3.5, 0.0), 10.0, 0.6);
    let mut sinker = mover(
        0,
        vec![box_brush(
            DVec3::new(1.0, 0.5, 1.0),
            DVec3::new(3.0, 1.5, 3.0),
        )],
        false,
    );
    sinker.light_poses = Some(3);
    let movers = [sinker];
    let mut mover_poses = MoverPoses::resolve(&movers, &waypoints(0, DVec3::new(0.0, -2.0, 0.0)));
    mover_poses
        .place_poses(&movers, 1.0, &ProbeGridLayout::no_probes())
        .expect("authored poses place");
    let poses = PosedMovers::resolve(
        &mover_poses,
        &movers,
        &TextureNamesSection { names: Vec::new() },
    )
    .expect("the sinker poses");
    let solids = &poses.movers()[0].solids;
    assert_eq!(solids.len(), 3, "light_poses 3");
    let inside = world.probe_at([2.0, 0.0, 2.0]);
    let point = vec3_from(world.layout.probe_positions[inside]);
    assert_eq!(
        solids
            .iter()
            .map(|solid| solid.encloses(point))
            .collect::<Vec<_>>(),
        [false, true, false],
        "only the mid pose encloses the probe"
    );

    let bake = world.bake(&poses, &[static_light(&light, 0)]);
    assert!(bake.enclosed.encloses(0, inside as u32, 1));
    let entry = bake.lights[0]
        .entries
        .iter()
        .find(|entry| {
            world
                .cells
                .valid_probes(entry.cell as usize)
                .contains(&inside)
        })
        .expect("the sinker gates the probe's cell");
    let slot = world
        .cells
        .valid_probes(entry.cell as usize)
        .iter()
        .position(|&probe| probe == inside)
        .unwrap();
    let q = entry.poses_at(slot);
    assert!(
        q[0] < 255,
        "the box shadows the probe at the first pose: {q:?}"
    );
    assert_eq!((q[1], q[2]), (255, 255), "enclosed, then sunk below it");

    let section = channel_section(
        &bake,
        &BounceBlocks::default(),
        &world.cells,
        &MoverRecords::new(&movers, &mover_poses),
        &super::section::MoverPayloadBars::default(),
        |_, _, _| false,
        |_| true,
    )
    .unwrap()
    .expect("the sinker gates the light");
    assert_eq!(
        MoverProbeChannelSection::from_bytes(&section.to_bytes()).expect("decodes"),
        section
    );
    let mut marked = 0;
    for view in section.direct_record_views() {
        let position = section.listed_position(view.cell as usize).expect("listed");
        let across = section.across_entry(position, 0);
        assert_eq!(view.movers().next().unwrap().across, across);
        for (local, probe) in world.cells.valid_locals(view.cell as usize) {
            let point = vec3_from(world.layout.probe_positions[probe]);
            let expected = view.record.probe_mask & (1 << local) != 0 && solids[1].encloses(point);
            assert_eq!(
                across.is_some_and(|across| across.is_across(1, local as u32)),
                expected,
                "probe {probe}: marked exactly where masked and enclosed at the mid pose"
            );
            marked += usize::from(expected);
            if probe == inside {
                assert!(expected, "the enclosed probe is masked and marked");
            }
        }
    }
    assert!(marked > 0);
}

// A probe a hair off a posed face, outside it, is not enclosed: its q is
// still the joint ratio, the samples that cross into the brush blocked and
// the rest clear. (The lift rule itself, one verdict per ray in every posed
// trace, is `a_probe_a_hair_off_a_posed_face_reads_one_inside_verdict_…`.)
#[test]
fn a_probe_beside_a_face_is_not_enclosed_and_keeps_the_per_ray_rule() {
    let world = world();
    // The light straight above the probe at (2, 0, 1); the block's −z
    // face sits 0.05 mm past the probe, so the probe is outside it.
    let light = point_light(DVec3::new(2.0, 3.5, 1.0), 10.0, 0.6);
    let beside = mover(
        0,
        vec![box_brush(
            DVec3::new(1.5, -0.5, 1.000_05),
            DVec3::new(2.5, 0.5, 2.0),
        )],
        false,
    );
    let poses = poses_of(&[beside]);
    let probe = world.probe_at([2.0, 0.0, 1.0]);
    let point = vec3_from(world.layout.probe_positions[probe]);
    let first = PosedOccluder::new(Arc::clone(
        &poses.movers()[0].solids[Terminus::First.index()],
    ));
    assert!(!first.encloses(point), "the probe is beside the face");
    let ctx = world.ctx();
    let (_, l) = incident_radiance_at_point(&light, point).expect("lit");
    let joint = joint_soft_visibility(
        point,
        l,
        &light,
        soft_visibility_seed(probe as u64, 0, 3),
        DEFAULT_AREA_SAMPLE_COUNT,
        ProbeEarlyOut::Agreed,
        |from, to| segment_clear(&ctx, from, to),
        |from, to| first.blocks(from, to),
    );
    assert!(
        joint.meets_mover() && joint.posed_clear > 0,
        "some samples' lifts cross the face, some do not: {joint:?}"
    );
    let bake = world.bake(&poses, &[static_light(&light, 0)]);
    let ([q0, q1], _) = probe_payload(&world, &bake.lights[0], 0, probe).expect("gated");
    assert_eq!((q0, q1), (joint.surviving_byte(), 255));
    assert!(q0 < 255);
}

// Enclosure takes the gather's lift as an inset: a probe 0.5 mm inside a
// posed face is not enclosed, so it traces the joint set and stores its
// per-ray q; one 2 mm inside is enclosed and stores q = 255, though every
// segment from it is blocked as well. The light sits beyond the box, so
// every segment crosses it either way.
#[test]
fn a_probe_within_the_lift_of_a_face_stores_its_per_ray_q_and_one_past_it_stores_255() {
    let world = world();
    let light = point_light(DVec3::new(2.0, 0.0, 4.5), 10.0, 0.6);
    let probe = world.probe_at([2.0, 0.0, 1.0]);
    let point = vec3_from(world.layout.probe_positions[probe]);
    let ctx = world.ctx();
    let (_, l) = incident_radiance_at_point(&light, point).expect("lit");
    for (depth_m, enclosed) in [(5.0e-4, false), (2.0e-3, true)] {
        // The box's −z face sits `depth_m` short of the probe.
        let block = mover(
            0,
            vec![box_brush(
                DVec3::new(1.5, -0.5, 1.0 - depth_m),
                DVec3::new(2.5, 0.5, 1.5),
            )],
            false,
        );
        let poses = poses_of(&[block]);
        let first = PosedOccluder::new(Arc::clone(
            &poses.movers()[0].solids[Terminus::First.index()],
        ));
        assert_eq!(first.encloses(point), enclosed, "{depth_m} m inside");
        let joint = joint_soft_visibility(
            point,
            l,
            &light,
            soft_visibility_seed(probe as u64, 0, 3),
            DEFAULT_AREA_SAMPLE_COUNT,
            ProbeEarlyOut::Agreed,
            |from, to| segment_clear(&ctx, from, to),
            |from, to| first.blocks(from, to),
        );
        assert!(
            joint.meets_mover() && joint.surviving_byte() < 255,
            "{depth_m} m: the box blocks the light's samples: {joint:?}"
        );
        let bake = world.bake(&poses, &[static_light(&light, 0)]);
        let ([q0, q1], _) = probe_payload(&world, &bake.lights[0], 0, probe).expect("gated");
        let expected = if enclosed {
            255
        } else {
            joint.surviving_byte()
        };
        assert_eq!((q0, q1), (expected, 255), "{depth_m} m");
    }
}

// A sun the posed brushes block stores q < 1, and a spinner,
// which has no parked pose, stores no entry.
#[test]
fn a_blocked_sun_stores_q_below_one_and_a_spinner_stores_no_entry() {
    let world = world();
    let sun = sun();
    let poses = poses_of(&[slab_mover()]);
    let bake = world.bake(&poses, &[static_light(&sun, 0)]);
    let under = world.probe_at([1.0, 0.0, 1.0]);
    let ([first, _], _) = probe_payload(&world, &bake.lights[0], 0, under).expect("gated");
    assert!(first < 255, "the slab hides the sun: q₀ byte {first}");

    let spinner = mover(
        3,
        vec![box_brush(
            DVec3::new(-5.0, 1.5, -5.0),
            DVec3::new(5.0, 1.6, 5.0),
        )],
        true,
    );
    let poses = poses_of(&[spinner]);
    assert!(poses.is_empty());
    let bake = world.bake(&poses, &[static_light(&sun, 0)]);
    assert!(bake.lights.is_empty());
}

// The carried tile is the light's own term as id 45 bakes it: unit radiance,
// its seed axis, at the same probe.
#[test]
fn an_animated_lights_own_tile_equals_its_id_45_tile() {
    let world = world();
    let mut authored = point_light(DVec3::new(0.0, 3.5, 0.0), 10.0, 0.6);
    authored.animation = Some(crate::map_data::LightAnimation {
        period: 1.0,
        phase: 0.0,
        brightness: Some(vec![0.0, 1.0]),
        color: None,
        direction: None,
        start_active: true,
    });
    authored.intensity = 3.0;
    let lights = [authored.clone()];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let animated_lights = crate::light_namespaces::AnimatedBakedLights::from_lights(&lights);
    let sh_ctx = ShBakeCtx {
        bounce_materials: None,
        bvh: &world.bvh,
        primitives: &world.primitives,
        geometry: &world.geometry,
        tree: &world.tree,
        exterior_leaves: &world.exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: 1,
    };
    let mutable =
        crate::delta_drop_policy::ScriptMutableDescriptorSlots::empty(animated_lights.len());
    let id45 = crate::animated_direct_sh_bake::bake_animated_direct_sh_delta_volumes(
        &crate::animated_direct_sh_bake::AnimatedDirectShBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &[],
            animated_lights: &animated_lights,
            mutable_descriptors: &mutable,
            embedded_emitters: &crate::texel_footprint::NO_EMBEDDED_EMITTERS,
        },
        &ShConfig { probe_spacing: 1.0 },
    )
    .expect("id 45");

    let mut baked = authored.clone();
    baked.intensity = 1.0;
    baked.color = [1.0; 3];
    let channel = ChannelLight {
        namespace: MoverLightNamespace::AnimatedBaked,
        light_index: 0,
        baked,
        authored: &authored,
        source_index: 0,
        seed_axis: 0,
        early_out: ProbeEarlyOut::Agreed,
        policy: AffinityReachPolicy::DIRECT,
    };
    let bake = world.bake(&poses_of(&[slab_mover()]), &[channel]);
    let probe = world.probe_at([1.0, 0.0, 1.0]);
    let (_, tile) = probe_payload(&world, &bake.lights[0], 0, probe).expect("gated");

    // Id 45's dense sub-block for the probe's cell, at the probe's local slot.
    let cell = world
        .cells
        .valid_masks
        .iter()
        .enumerate()
        .find(|(cell, _)| world.cells.valid_probes(*cell).contains(&probe))
        .map(|(cell, _)| cell)
        .unwrap();
    let entry = id45.affinity_offsets[cell] as usize;
    assert_eq!(id45.affinity_lights[entry], 0);
    let local = world
        .cells
        .valid_probes(cell)
        .iter()
        .position(|&candidate| candidate == probe)
        .unwrap();
    // Every probe of this grid is valid, so the dense local is the slot.
    let start = (entry * 64 + local) * DEFAULT_DELTA_PROBE_F16_STRIDE;
    assert_eq!(
        tile,
        &id45.delta_subblocks[start..start + DEFAULT_DELTA_PROBE_F16_STRIDE]
    );
}

/// Two static lightmap lights: a runtime light in the grid's far corner
/// that lights nothing near the slab, then a `_bake_only` light above it,
/// static-baked index 1.
fn bake_only_lights() -> [MapLight; 2] {
    let mut bake_only = point_light(DVec3::new(0.0, 3.5, 0.0), 10.0, 0.6);
    bake_only.bake_only = true;
    [
        point_light(DVec3::new(-3.5, -3.5, -3.5), 1.0, 0.1),
        bake_only,
    ]
}

/// `lights` as id 35 and the channel see them.
struct LightWorld<'w> {
    static_lights: crate::light_namespaces::StaticBakedLights<'w>,
    animated_lights: crate::light_namespaces::AnimatedBakedLights<'w>,
    alpha_lights: crate::light_namespaces::AlphaLightsNs<'w>,
    mutable: crate::delta_drop_policy::ScriptMutableDescriptorSlots,
}

impl<'w> LightWorld<'w> {
    fn new(lights: &'w [MapLight]) -> Self {
        Self {
            static_lights: crate::light_namespaces::StaticBakedLights::from_lights(lights),
            animated_lights: crate::light_namespaces::AnimatedBakedLights::from_lights(lights),
            alpha_lights: crate::light_namespaces::AlphaLightsNs::from_lights(lights),
            mutable: crate::delta_drop_policy::ScriptMutableDescriptorSlots::empty(
                crate::light_namespaces::AnimatedBakedLights::from_lights(lights).len(),
            ),
        }
    }

    fn sh_ctx<'a>(&'a self, world: &'a World) -> ShBakeCtx<'a> {
        ShBakeCtx {
            bounce_materials: None,
            bvh: &world.bvh,
            primitives: &world.primitives,
            geometry: &world.geometry,
            tree: &world.tree,
            exterior_leaves: &world.exterior,
            static_lights: &self.static_lights,
            animated_lights: &self.animated_lights,
            total_light_count: self.static_lights.len() + self.animated_lights.len(),
        }
    }

    /// The production gated set: [`channel_lights`].
    fn channel_lights<'a>(&'a self, sh_ctx: &'a ShBakeCtx<'a>) -> Vec<ChannelLight<'a>> {
        channel_lights(&ChannelLightSources {
            sh_ctx,
            alpha_lights: &self.alpha_lights,
            animated_lights: &self.animated_lights,
            static_embedded: &crate::texel_footprint::NO_EMBEDDED_EMITTERS,
            animated_embedded: &crate::texel_footprint::NO_EMBEDDED_EMITTERS,
            mutable_descriptors: &self.mutable,
        })
    }
}

// Lights in scope: a static lightmap light with an AlphaLights record keeps
// its AlphaLights name, and a `_bake_only` one, which has none, takes its
// static-baked index in namespace 2, so each light has exactly one name.
#[test]
fn the_gated_set_names_bake_only_lights_by_static_baked_index_and_no_light_twice() {
    let world = world();
    let lights = bake_only_lights();
    let light_world = LightWorld::new(&lights);
    let sh_ctx = light_world.sh_ctx(&world);
    let names: Vec<(MoverLightNamespace, u32, u64)> = light_world
        .channel_lights(&sh_ctx)
        .iter()
        .map(|light| (light.namespace, light.light_index, light.seed_axis))
        .collect();
    assert_eq!(
        names,
        [
            (MoverLightNamespace::StaticLight, 0, 0),
            (MoverLightNamespace::BakeOnlyStaticLight, 1, 1),
        ]
    );
}

// Lights in scope: a bake-only light the posed slab blocks stores
// q < 1 under it, and its entry carries its own tile even where the caller
// would flag none, ranked to index 0 of the section's bake-only roster.
#[test]
fn a_bake_only_light_blocked_by_a_posed_mover_stores_q_below_one_with_its_own_tile() {
    let world = world();
    let lights = bake_only_lights();
    let light_world = LightWorld::new(&lights);
    let sh_ctx = light_world.sh_ctx(&world);
    let channel = light_world.channel_lights(&sh_ctx);
    let movers = [slab_mover()];
    let bake = world.bake(&poses_of(&movers), &channel);
    assert_eq!(
        bake.lights
            .iter()
            .map(|light| (light.namespace, light.light_index))
            .collect::<Vec<_>>(),
        [(MoverLightNamespace::BakeOnlyStaticLight, 1)],
        "only the bake-only light reaches the slab"
    );

    let section = channel_section(
        &bake,
        &BounceBlocks::default(),
        &world.cells,
        &MoverRecords::new(&movers, &MoverPoses::default()),
        &super::section::MoverPayloadBars::default(),
        |_, _, _| false,
        |_| true,
    )
    .unwrap()
    .expect("the gated bake-only light makes a section");
    assert!(section.direct_records.iter().all(|record| {
        record.namespace == MoverLightNamespace::BakeOnlyStaticLight
            && record.light_index == 0
            && record.carries_own_tile
    }));
    assert_eq!(section.bake_only_light_count(), 1);

    let under = world.probe_at([1.0, 0.0, 1.0]);
    let ([first, last], tile) =
        probe_payload(&world, &bake.lights[0], 0, under).expect("the probe under the slab");
    assert!(first < 255, "the slab hides the light: q₀ byte {first}");
    assert_eq!(last, 255, "the last pose is 100 m up");
    let (view, slot) = section
        .direct_record_views()
        .find_map(|view| Some((view, masked_slot(&world, &view, under)?)))
        .expect("the probe is masked in its cell's record");
    assert_eq!(view.movers().next().unwrap().probe_q(slot), [first, last]);
    assert_eq!(view.own_tile(slot).expect("its own tile"), tile);
    assert_eq!(
        MoverProbeChannelSection::from_bytes(&section.to_bytes()).expect("decodes"),
        section
    );
}

/// The slot dense `probe` takes in `view`'s payload, if the record masks it.
fn masked_slot(world: &World, view: &DirectRecordView<'_>, probe: usize) -> Option<usize> {
    let valid_slot = world
        .cells
        .valid_probes(view.cell as usize)
        .iter()
        .position(|&candidate| candidate == probe)?;
    let mut bits = view.valid_probe_mask;
    for _ in 0..valid_slot {
        bits &= bits - 1;
    }
    let local = bits.trailing_zeros();
    let mask = view.record.probe_mask;
    (mask & (1 << local) != 0).then(|| (mask & ((1u64 << local) - 1)).count_ones() as usize)
}

/// The slab mover and a second mover holding the same brushes at its first
/// waypoint, so both gate the light under the slab at the same probes.
fn twin_slab_movers() -> [MapKinematicMover; 2] {
    let slab = slab_mover();
    let twin = mover(1, slab.brush_volumes.clone(), false);
    [slab, twin]
}

// Term records: two movers gating one light at one cell share one record,
// which stores the light's tile once, at its masked probes, and each mover's
// q pairs under it.
#[test]
fn two_movers_gating_one_light_at_one_cell_store_its_tile_once() {
    let world = world();
    let light = point_light(DVec3::new(0.0, 3.5, 0.0), 10.0, 0.6);
    let movers = twin_slab_movers();
    let bake = world.bake(&poses_of(&movers), &[static_light(&light, 4)]);
    let gated = &bake.lights[0];
    let section = channel_section(
        &bake,
        &BounceBlocks::default(),
        &world.cells,
        &MoverRecords::new(&movers, &MoverPoses::default()),
        &super::section::MoverPayloadBars::default(),
        |_, _, _| true,
        |_| true,
    )
    .unwrap()
    .expect("the slabs gate the light");

    let shared: Vec<_> = section
        .direct_record_views()
        .filter(|view| view.mover_ids == [0, 1])
        .collect();
    assert!(!shared.is_empty(), "both movers gate the light at one cell");
    for view in &shared {
        let masked = view.record.masked_probe_count();
        assert_eq!(
            view.own_tiles.map(|tiles| tiles.len()),
            Some(masked * DEFAULT_DELTA_PROBE_F16_STRIDE),
            "one tile per masked probe, not one per mover"
        );
        for probe in world.cells.valid_probes(view.cell as usize) {
            let Some(slot) = masked_slot(&world, view, probe) else {
                continue;
            };
            for mover in view.movers() {
                let (pair, tile) = probe_payload(&world, gated, mover.mover_id, probe)
                    .expect("each mover's entry covers the probe");
                assert_eq!(
                    mover.probe_q(slot),
                    pair,
                    "mover {} probe {probe}",
                    mover.mover_id
                );
                assert_eq!(
                    view.own_tile(slot).map(|tile| tile.to_vec()),
                    Some(tile.to_vec()),
                    "probe {probe}"
                );
            }
        }
    }
    let counts = section.direct_counts();
    assert_eq!(
        section.own_tiles.len(),
        counts.own_tile_probes * DEFAULT_DELTA_PROBE_F16_STRIDE
    );
    assert!(
        counts.mover_entries > counts.records,
        "some record holds two movers: {counts:?}"
    );
    assert_eq!(
        MoverProbeChannelSection::from_bytes(&section.to_bytes()).expect("decodes"),
        section
    );
}

// Term records: a valid probe where every mover keeps q = 255 at both
// termini is left out of the record's mask, so it stores neither q nor a
// tile; every masked probe has some mover below 255.
#[test]
fn the_mask_leaves_out_a_probe_every_mover_leaves_at_255() {
    let world = world();
    let light = point_light(DVec3::new(0.0, 3.5, 0.0), 10.0, 0.6);
    let movers = twin_slab_movers();
    let bake = world.bake(&poses_of(&movers), &[static_light(&light, 4)]);
    let gated = &bake.lights[0];
    let section = channel_section(
        &bake,
        &BounceBlocks::default(),
        &world.cells,
        &MoverRecords::new(&movers, &MoverPoses::default()),
        &super::section::MoverPayloadBars::default(),
        |_, _, _| true,
        |_| true,
    )
    .unwrap()
    .expect("the slabs gate the light");

    let mut excluded = 0;
    for view in section.direct_record_views() {
        for probe in world.cells.valid_probes(view.cell as usize) {
            let changed = view.mover_ids.iter().any(|&mover_id| {
                probe_payload(&world, gated, mover_id, probe)
                    .is_some_and(|(pair, _)| pair.iter().any(|&q| q < 255))
            });
            let slot = masked_slot(&world, &view, probe);
            assert_eq!(slot.is_some(), changed, "probe {probe}");
            if let Some(slot) = slot {
                assert!(view.movers().any(|mover| mover.probe_q(slot) != [255, 255]));
            } else {
                excluded += 1;
            }
        }
    }
    assert!(
        excluded > 0,
        "some valid probe of a gated cell is unchanged"
    );
}

// A bake-only light's carried tile is its own term in id 35: at every probe
// its entries cover, it equals id 35's tile in a bake where no other light
// reaches that probe, texel for texel (id 35's lossless form).
#[test]
fn a_bake_only_lights_own_tile_equals_its_own_id_35_term() {
    let world = world();
    let lights = bake_only_lights();
    let light_world = LightWorld::new(&lights);
    let sh_ctx = light_world.sh_ctx(&world);
    let channel = light_world.channel_lights(&sh_ctx);
    let bake = world.bake(&poses_of(&[slab_mover()]), &channel);
    let id35 = crate::direct_sh_bake::bake_direct_sh_volume(
        &crate::direct_sh_bake::DirectBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &[],
            embedded_emitters: &crate::texel_footprint::NO_EMBEDDED_EMITTERS,
        },
        &ShConfig { probe_spacing: 1.0 },
    );
    assert_eq!(id35.grid_dimensions, world.layout.dims);
    let dimension = id35.tile_dimension as usize;
    let width = id35.atlas_dimensions[0] as usize;
    let layer_texels = width * id35.atlas_dimensions[1] as usize;
    let id35_tile = |probe: usize| -> Vec<u16> {
        let [layer, tile_x, tile_y] =
            postretro_level_format::octahedral::irradiance_array_tile_location(
                probe,
                id35.tiles_per_layer,
                id35.atlas_tiles_per_row,
            )
            .map(|coordinate| coordinate as usize);
        let mut tile = Vec::with_capacity(DEFAULT_DELTA_PROBE_F16_STRIDE);
        for y in 0..dimension {
            for x in 0..dimension {
                let texel = layer * layer_texels
                    + (tile_y * dimension + y) * width
                    + tile_x * dimension
                    + x;
                let rgba = &id35.atlas[texel * 8..texel * 8 + 8];
                tile.extend((0..3).map(|c| u16::from_le_bytes([rgba[2 * c], rgba[2 * c + 1]])));
            }
        }
        tile
    };

    let [runtime_light, _] = &lights;
    let (mut compared, mut lit) = (0, 0);
    for entry in &bake.lights[0].entries {
        for (slot, probe) in world
            .cells
            .valid_probes(entry.cell as usize)
            .into_iter()
            .enumerate()
        {
            let point = vec3_from(world.layout.probe_positions[probe]);
            assert!(
                incident_radiance_at_point(runtime_light, point)
                    .is_none_or(|(radiance, _)| radiance == Vec3::ZERO),
                "probe {probe}: the runtime light adds nothing to id 35 here"
            );
            let own = &entry.tiles[slot * DEFAULT_DELTA_PROBE_F16_STRIDE
                ..(slot + 1) * DEFAULT_DELTA_PROBE_F16_STRIDE];
            assert_eq!(own, id35_tile(probe), "probe {probe}");
            compared += 1;
            lit += usize::from(own.iter().any(|&half| half != 0));
        }
    }
    assert!(compared > 0 && lit > 0, "{compared} probes, {lit} lit");
}

fn id41(levels: Vec<u8>, offsets: Vec<u32>, lights: Vec<u32>) -> DirectShDeltaVolumesSection {
    DirectShDeltaVolumesSection {
        affinity_factor: 4,
        affinity_dims: [2, 1, 1],
        tile_dimension: 6,
        tile_border: 1,
        valid_probe_masks: vec![u64::MAX; 2],
        cell_levels: levels,
        affinity_offsets: offsets,
        affinity_lights: lights,
        delta_subblocks: Vec::new(),
    }
}

fn entry(cell: u32, mover_id: u32) -> DirectEntry {
    DirectEntry {
        cell,
        mover_id,
        pose_count: 2,
        visibility: vec![7; 2],
        tiles: vec![cell as u16 + 1; DEFAULT_DELTA_PROBE_F16_STRIDE],
    }
}

/// A two-cell grid whose cells hold one valid probe each.
fn two_cells() -> ProbeCells {
    ProbeCells {
        dims: [8, 4, 4],
        affinity_dims: [2, 1, 1],
        valid_masks: vec![1, 1],
    }
}

// Own-tile flag: an entry carries its light's tile unless ids 41 or 45, as
// emitted, hold the term per probe at its cell: a static light with no id-41
// entry there, or whose cell id 41 stores coarser, carries it.
#[test]
fn the_own_tile_flag_follows_ids_41_and_45_as_emitted() {
    let selection = EntityShadowLightsSection {
        light_indices: vec![2, 9],
    };
    // Slot 1 (AlphaLights 9) has entries at both cells; cell 1 is coarsened.
    let direct = id41(vec![0, 1], vec![0, 1, 2], vec![1, 1]);
    let animated = AnimatedDirectShDeltaVolumesSection {
        affinity_factor: 4,
        affinity_dims: [2, 1, 1],
        tile_dimension: 6,
        tile_border: 1,
        animation_descriptor_indices: vec![0, 1],
        valid_probe_masks: vec![u64::MAX; 2],
        cell_levels: vec![0, 0],
        affinity_offsets: vec![0, 0, 1],
        affinity_lights: vec![1],
        delta_subblocks: Vec::new(),
    };
    let emitted = EmittedDirectEntries {
        selection: Some(&selection),
        direct: Some(&direct),
        animated: Some(&animated),
    };
    use MoverLightNamespace::{AnimatedBaked, StaticLight};
    assert!(emitted.holds_per_probe(0, StaticLight, 9));
    assert!(!emitted.holds_per_probe(1, StaticLight, 9), "coarsened");
    assert!(
        !emitted.holds_per_probe(0, StaticLight, 2),
        "no entry at the cell"
    );
    assert!(!emitted.holds_per_probe(0, StaticLight, 5), "not selected");
    assert!(emitted.holds_per_probe(1, AnimatedBaked, 1));
    assert!(!emitted.holds_per_probe(0, AnimatedBaked, 1));
    let other_light = AnimatedDirectShDeltaVolumesSection {
        affinity_lights: vec![0],
        ..animated.clone()
    };
    assert!(!emitted_with(&selection, &direct, &other_light).holds_per_probe(1, AnimatedBaked, 1));
    let none = EmittedDirectEntries {
        selection: None,
        direct: None,
        animated: None,
    };
    assert!(!none.holds_per_probe(0, StaticLight, 9));

    let bake = MoverDirectBake {
        lights: vec![
            LightEntries {
                namespace: StaticLight,
                light_index: 9,
                entries: vec![entry(0, 4), entry(1, 4)],
            },
            LightEntries {
                namespace: AnimatedBaked,
                light_index: 1,
                entries: vec![entry(1, 2)],
            },
        ],
        ..MoverDirectBake::default()
    };
    let ramps = MoverRecords::new(
        &[mover(4, Vec::new(), false), mover(2, Vec::new(), false)],
        &MoverPoses::default(),
    );
    let section = channel_section(
        &bake,
        &BounceBlocks::default(),
        &two_cells(),
        &ramps,
        &super::section::MoverPayloadBars::default(),
        |cell, ns, light| !emitted.holds_per_probe(cell, ns, light),
        |ns| emitted.composes_gated(ns),
    )
    .unwrap()
    .expect("entries make a section");
    let flags: Vec<(MoverLightNamespace, bool, &[u32])> = section
        .direct_record_views()
        .map(|view| {
            (
                view.record.namespace,
                view.record.carries_own_tile,
                view.mover_ids,
            )
        })
        .collect();
    // Cell 0: (static). Cell 1: (static) then (animated).
    assert_eq!(
        flags,
        vec![
            (StaticLight, false, &[4][..]),
            (StaticLight, true, &[4][..]),
            (AnimatedBaked, false, &[2][..]),
        ]
    );
    assert_eq!(section.direct_offsets, vec![0, 1, 3]);
    assert_eq!(section.own_tiles, vec![2; DEFAULT_DELTA_PROBE_F16_STRIDE]);
    assert_eq!(
        section
            .mover_records
            .iter()
            .map(|record| (
                record.mover_id,
                record.ramp_start,
                record.ramp_end,
                record.knots.clone()
            ))
            .collect::<Vec<_>>(),
        vec![
            (2, 0.25, 0.75, vec![0.0, 1.0]),
            (4, 0.25, 0.75, vec![0.0, 1.0])
        ]
    );
    let decoded = MoverProbeChannelSection::from_bytes(&section.to_bytes()).expect("decodes");
    assert_eq!(decoded, section);
}

// No id 45, no namespace-1 record: the renderer composes animated-baked
// records ungated when id 45 is absent, so the compiler writes none. Static
// and bake-only records stay, and a bake whose only entries are animated
// writes no section.
#[test]
fn with_no_id_45_no_animated_baked_record_is_written() {
    use MoverLightNamespace::{AnimatedBaked, BakeOnlyStaticLight, StaticLight};
    let without_id45 = EmittedDirectEntries {
        selection: None,
        direct: None,
        animated: None,
    };
    assert!(!without_id45.composes_gated(AnimatedBaked));
    assert!(without_id45.composes_gated(StaticLight));
    assert!(without_id45.composes_gated(BakeOnlyStaticLight));
    let lights = |namespaces: &[MoverLightNamespace]| MoverDirectBake {
        lights: namespaces
            .iter()
            .map(|&namespace| LightEntries {
                namespace,
                light_index: 1,
                entries: vec![entry(1, 2)],
            })
            .collect(),
        ..MoverDirectBake::default()
    };
    let section = |bake: &MoverDirectBake, emitted: &EmittedDirectEntries<'_>| {
        channel_section(
            bake,
            &BounceBlocks::default(),
            &two_cells(),
            &MoverRecords::new(&[mover(2, Vec::new(), false)], &MoverPoses::default()),
            &super::section::MoverPayloadBars::default(),
            |cell, ns, light| !emitted.holds_per_probe(cell, ns, light),
            |ns| emitted.composes_gated(ns),
        )
        .unwrap()
    };
    let namespaces = |section: &MoverProbeChannelSection| -> Vec<MoverLightNamespace> {
        section
            .direct_record_views()
            .map(|view| view.record.namespace)
            .collect()
    };

    let both = lights(&[StaticLight, AnimatedBaked]);
    let kept = section(&both, &without_id45).expect("the static record stays");
    assert_eq!(namespaces(&kept), vec![StaticLight]);
    assert!(
        section(&lights(&[AnimatedBaked]), &without_id45).is_none(),
        "animated-only entries write no section without id 45"
    );

    let animated = AnimatedDirectShDeltaVolumesSection {
        affinity_factor: 4,
        affinity_dims: [2, 1, 1],
        tile_dimension: 6,
        tile_border: 1,
        animation_descriptor_indices: vec![0, 1],
        valid_probe_masks: vec![u64::MAX; 2],
        cell_levels: vec![0, 0],
        affinity_offsets: vec![0, 0, 1],
        affinity_lights: vec![1],
        delta_subblocks: Vec::new(),
    };
    let with_id45 = EmittedDirectEntries {
        selection: None,
        direct: None,
        animated: Some(&animated),
    };
    let kept = section(&both, &with_id45).expect("both records stay");
    assert_eq!(namespaces(&kept), vec![StaticLight, AnimatedBaked]);
}

fn emitted_with<'a>(
    selection: &'a EntityShadowLightsSection,
    direct: &'a DirectShDeltaVolumesSection,
    animated: &'a AnimatedDirectShDeltaVolumesSection,
) -> EmittedDirectEntries<'a> {
    EmittedDirectEntries {
        selection: Some(selection),
        direct: Some(direct),
        animated: Some(animated),
    }
}

// No entry, no section: id 57 is never written empty.
#[test]
fn a_bake_with_no_entry_makes_no_section() {
    let section = channel_section(
        &MoverDirectBake::default(),
        &BounceBlocks::default(),
        &two_cells(),
        &MoverRecords::new(&[], &MoverPoses::default()),
        &super::section::MoverPayloadBars::default(),
        |_, _, _| true,
        |_| true,
    )
    .unwrap();
    assert!(section.is_none());
}

#[test]
fn memo_payloads_round_trip_and_reject_malformed_ones() {
    let cells = two_cells();
    let world = world();
    let light = point_light(DVec3::new(0.0, 3.5, 0.0), 10.0, 0.6);
    let poses = poses_of(&[slab_mover()]);
    let movers = light_movers(&light, &poses);
    assert_eq!(movers.len(), 1);
    let entries = vec![entry(0, 0), entry(1, 0)];
    let bytes = encode(&entries);
    assert_eq!(decode(&bytes, &cells, &movers), Some(entries.clone()));
    assert_eq!(decode(&encode(&[]), &cells, &movers), Some(Vec::new()));
    assert!(
        decode(&bytes[..bytes.len() - 1], &cells, &movers).is_none(),
        "short"
    );
    assert!(
        decode(&[bytes.clone(), vec![0]].concat(), &cells, &movers).is_none(),
        "long"
    );
    let swapped = vec![entry(1, 0), entry(0, 0)];
    assert!(
        decode(&encode(&swapped), &cells, &movers).is_none(),
        "out of order"
    );
    assert!(
        decode(&encode(&[entry(0, 5)]), &cells, &movers).is_none(),
        "unknown mover"
    );
    assert!(
        decode(&encode(&[entry(2, 0)]), &cells, &movers).is_none(),
        "past the grid"
    );
    let three_poses = DirectEntry {
        pose_count: 3,
        visibility: vec![7; 3],
        ..entry(0, 0)
    };
    assert!(
        decode(&encode(&[three_poses]), &cells, &movers).is_none(),
        "a pose count unlike the mover's"
    );
    drop(world);
}

/// A slab that crosses between the light at (2, 3.5, 2) and the probes
/// under it only half way along its 20 m path: it starts 10 m west and ends
/// 10 m east, out of the light's 4 m reach at both termini. Three poses
/// (`light_poses`), so its middle pose sits right under the light.
fn crossing_slab(ramp: (f32, f32)) -> (MapKinematicMover, PosedMovers, MoverPoses) {
    let mut slab = mover(
        0,
        vec![box_brush(
            DVec3::new(-8.5, 1.5, 1.0),
            DVec3::new(-7.5, 1.6, 3.0),
        )],
        false,
    );
    slab.light_poses = Some(3);
    (slab.light_ramp_start, slab.light_ramp_end) = ramp;
    let waypoints = waypoints(0, DVec3::new(20.0, 0.0, 0.0));
    let mut poses = MoverPoses::resolve(std::slice::from_ref(&slab), &waypoints);
    poses
        .place_poses(
            std::slice::from_ref(&slab),
            1.0,
            &ProbeGridLayout::no_probes(),
        )
        .expect("authored poses place");
    assert_eq!(poses.analysed[0].knots, vec![0.0, 0.5, 1.0]);
    let posed = PosedMovers::resolve(
        &poses,
        std::slice::from_ref(&slab),
        &TextureNamesSection { names: Vec::new() },
    )
    .expect("the slab poses");
    (slab, posed, poses)
}

// A light only a mid pose blocks is gated: its reach misses both termini
// and meets the middle pose, so the channel names it for the mover, and the
// probe under the slab keeps q below 255 at that pose and 255 at both
// termini. The gate's report counts it for the mover as gated only at a mid
// pose. The term record masks it, and the mover's record carries the three
// knots.
#[test]
fn a_light_only_a_mid_pose_blocks_is_gated_at_that_pose_alone() {
    let world = world();
    let light = point_light(DVec3::new(2.0, 3.5, 2.0), 4.0, 0.1);
    let channel = static_light(&light, 0);
    let (slab, posed, poses) = crossing_slab((0.0, 1.0));
    let movers = light_movers(&light, &posed);
    assert_eq!(movers.len(), 1, "a mid pose's reach gates the light");
    assert_eq!(
        movers[0]
            .poses
            .iter()
            .map(Option::is_some)
            .collect::<Vec<_>>(),
        [false, true, false],
        "only the middle pose lies in the light's reach"
    );
    let bake = world.bake(&posed, std::slice::from_ref(&channel));
    assert_eq!(bake.lights.len(), 1, "the light is gated");
    let probe = world.probe_at([2.0, 0.0, 2.0]);
    let entry = bake.lights[0]
        .entries
        .iter()
        .find(|entry| {
            world
                .cells
                .valid_probes(entry.cell as usize)
                .contains(&probe)
        })
        .expect("the probe's cell has an entry");
    let slot = world
        .cells
        .valid_probes(entry.cell as usize)
        .iter()
        .position(|&candidate| candidate == probe)
        .expect("the probe is valid");
    let q = entry.poses_at(slot);
    assert_eq!(q.len(), 3);
    assert!(q[1] < 255, "the middle pose blocks the light: {q:?}");
    assert_eq!([q[0], q[2]], [255, 255], "neither terminus blocks it");
    let reported = super::mid_pose_only_lights(&bake, std::slice::from_ref(&channel));
    assert_eq!(
        reported.movers().collect::<Vec<_>>(),
        [(slab.mover_id, &[0usize][..])],
        "the report counts the light for the slab alone"
    );

    let section = channel_section(
        &bake,
        &BounceBlocks::default(),
        &world.cells,
        &MoverRecords::new(std::slice::from_ref(&slab), &poses),
        &super::section::MoverPayloadBars::default(),
        |_, _, _| true,
        |_| true,
    )
    .unwrap()
    .expect("the gated light makes a section");
    assert_eq!(section.mover_records[0].knots, vec![0.0, 0.5, 1.0]);
    let decoded = MoverProbeChannelSection::from_bytes(&section.to_bytes()).expect("decodes");
    assert_eq!(decoded, section);
}

// A ramp of (0.6, 0.9) maps the only blocking pose's knot (t 0.5) onto
// t′ 0, so compose never reads it. The bake still bakes it (no pose is
// skipped), and the section id 57 writes from it loads: its record mask is
// the probes some pose gates, and no mover entry stores 255 at every masked
// probe, which the loader would refuse.
#[test]
fn a_ramped_movers_squeezed_out_blocking_pose_still_bakes_and_its_section_loads() {
    let world = world();
    let light = point_light(DVec3::new(2.0, 3.5, 2.0), 4.0, 0.1);
    let channel = static_light(&light, 0);
    let (slab, posed, poses) = crossing_slab((0.6, 0.9));
    let bake = world.bake(&posed, std::slice::from_ref(&channel));
    let gated: Vec<u8> = bake.lights[0]
        .entries
        .iter()
        .flat_map(|entry| (0..entry.visibility.len() / 3).map(move |slot| entry.poses_at(slot)[1]))
        .collect();
    assert!(
        gated.iter().any(|&q| q < 255),
        "the squeezed-out pose still bakes its q"
    );
    let section = channel_section(
        &bake,
        &BounceBlocks::default(),
        &world.cells,
        &MoverRecords::new(std::slice::from_ref(&slab), &poses),
        &super::section::MoverPayloadBars::default(),
        |_, _, _| true,
        |_| true,
    )
    .unwrap()
    .expect("a section");
    let record = &section.mover_records[0];
    assert_eq!(
        (record.ramp_start, record.ramp_end, record.knots.clone()),
        (0.6, 0.9, vec![0.0, 0.5, 1.0]),
        "knots are path fractions, not ramped"
    );
    MoverProbeChannelSection::from_bytes(&section.to_bytes()).expect("id 57 loads");
}

// Cache: a warm bake from the memo traces no ray and equals cold; the key
// ignores what never occludes (texture names) and movers the light cannot
// reach, and follows a pose the light reaches.
#[test]
fn the_memo_serves_a_warm_bake_without_tracing_and_keys_on_occlusion_only() {
    let world = world();
    let light = point_light(DVec3::new(0.0, 3.5, 0.0), 10.0, 0.6);
    let far = point_light(DVec3::new(-3.5, -3.5, -3.5), 1.0, 0.1);
    let lights = [static_light(&light, 0), static_light(&far, 1)];
    let poses = poses_of(&[slab_mover()]);
    let dir = tempfile::tempdir().expect("cache dir");
    let cache = StageCache::new(dir.path()).expect("cache");
    let vertices = world.vertices();
    let scene = world.scene(&vertices);
    let control = BakeControl::unrestricted();

    let cold = world.bake(&poses, &lights);
    let (filled, filling) = counted(2, || {
        bake_direct(&poses, &lights, &scene, Some(&cache), &control).unwrap()
    });
    assert!(filling.shadow_rays > 0);
    let (warm, serving) = counted(2, || {
        bake_direct(&poses, &lights, &scene, Some(&cache), &control).unwrap()
    });
    assert_eq!(serving.shadow_rays, 0, "a memo hit traces nothing");
    for bake in [&filled, &warm] {
        assert_eq!(format!("{:?}", bake.lights), format!("{:?}", cold.lights));
    }

    let memo = DirectMemo::new(&cache, MemoPrefix::new(&scene));
    let key = |memo: &DirectMemo<'_>, light: &ChannelLight<'_>, poses: &PosedMovers| {
        memo.key(light, &light_movers(&light.baked, poses))
            .as_filename()
    };
    let base = key(&memo, &lights[0], &poses);

    let mut retextured = world_with(&world, |geometry| {
        geometry.texture_names.names = vec!["other".into()];
    });
    let retextured_scene = retextured.0.scene(&retextured.1);
    let retextured_memo = DirectMemo::new(&cache, MemoPrefix::new(&retextured_scene));
    assert_eq!(
        key(&retextured_memo, &lights[0], &poses),
        base,
        "retexture hits"
    );
    retextured.0.geometry.geometry.vertices[0].position[0] = -0.2;
    let moved_scene = retextured.0.scene(&retextured.1);
    let moved_memo = DirectMemo::new(&cache, MemoPrefix::new(&moved_scene));
    assert_ne!(
        key(&moved_memo, &lights[0], &poses),
        base,
        "geometry misses"
    );

    // A second mover far from both lights leaves both keys alone; moving the
    // slab's last waypoint re-keys the light reaching it and not the other.
    let far_mover = mover(
        7,
        vec![box_brush(DVec3::splat(30.0), DVec3::splat(31.0))],
        false,
    );
    let with_far = poses_of(&[slab_mover(), far_mover]);
    assert_eq!(key(&memo, &lights[0], &with_far), base);
    let mut moved_waypoints = waypoints(0, DVec3::new(0.0, 3.0, 0.0)).to_vec();
    moved_waypoints.extend(waypoints(7, DVec3::X * 40.0));
    let moved = posed(&[slab_mover()], &moved_waypoints);
    assert_ne!(key(&memo, &lights[0], &moved), base);
    assert_eq!(
        key(&memo, &lights[1], &moved),
        key(&memo, &lights[1], &poses),
        "the far light reaches neither slab pose"
    );
}

/// A copy of `world` with its geometry edited, and that copy's vertices.
fn world_with(world: &World, edit: impl FnOnce(&mut GeometryResult)) -> (World, Vec<[f32; 3]>) {
    let mut geometry = world.geometry.clone();
    edit(&mut geometry);
    let copy = world_of(geometry);
    let vertices = copy.vertices();
    (copy, vertices)
}

// Direct half: the bake does not depend on the worker count.
#[test]
fn the_direct_bake_is_identical_across_worker_counts() {
    let world = world();
    let light = point_light(DVec3::new(0.0, 3.5, 0.0), 10.0, 0.6);
    let sun = sun();
    let lights = [static_light(&light, 0), static_light(&sun, 1)];
    let poses = poses_of(&[slab_mover()]);
    let (one, _) = counted(1, || world.bake(&poses, &lights));
    let (four, _) = counted(4, || world.bake(&poses, &lights));
    assert!(one.entry_count() > 0);
    assert_eq!(format!("{:?}", one.lights), format!("{:?}", four.lights));
}

/// Bytes a bake's entries hold, by capacity, as the SH gate prices them.
fn held_entry_bytes(bake: &MoverDirectBake) -> u64 {
    bake.lights
        .iter()
        .flat_map(|light| &light.entries)
        .map(|entry| {
            (entry.visibility.capacity() + entry.tiles.capacity() * 2) as u64
                + size_of::<DirectEntry>() as u64
        })
        .sum()
}

// SH gate: the direct half's bound, priced with no ray from
// the bake's own prefilter, covers what the bake holds for a lit point light
// and a sun, and is tighter than charging every cell in the light's reach a
// full cell's payload for every mover it may meet (what it charged before,
// which refused the full hallway at 39.5 GB).
#[test]
fn the_direct_entry_bound_covers_the_bake_within_the_lights_reach() {
    let world = world();
    let poses = poses_of(&[slab_mover()]);
    let full_cell =
        (64 * (2 + DEFAULT_DELTA_PROBE_F16_STRIDE * 2) + size_of::<DirectEntry>()) as u64;
    for light in [
        point_light(DVec3::new(0.0, 3.5, 0.0), 10.0, 0.6),
        point_light(DVec3::new(0.0, -3.0, -3.0), 4.6, 0.1),
        sun(),
    ] {
        let bake = world.bake(&poses, &[static_light(&light, 0)]);
        let held = held_entry_bytes(&bake);
        let bound = direct_entry_bound(&[&light], &poses, &world.layout);
        assert!(
            bound >= held,
            "{:?} light: bound {bound} B under the {held} B the bake holds",
            light.light_type
        );
        // Every valid probe in reach, every mover it may meet, a full cell.
        let reached_cells: HashSet<usize> = (0..world.layout.total_probes())
            .filter(|&probe| world.layout.validity[probe] != 0)
            .filter(|&probe| {
                light.light_type == LightType::Directional
                    || world.layout.probe_positions[probe].distance(light.origin)
                        <= f64::from(light.falloff_range) + 1.0
            })
            .map(|probe| {
                let [nx, ny, _] = world.layout.dims.map(|dim| dim as usize);
                let [ax, ay, _] = world.cells.affinity_dims.map(|dim| dim as usize);
                let (x, y, z) = (probe % nx, probe / nx % ny, probe / (nx * ny));
                x / 4 + y / 4 * ax + z / 4 * ax * ay
            })
            .collect();
        let whole_reach =
            light_movers(&light, &poses).len() as u64 * reached_cells.len() as u64 * full_cell;
        assert!(
            bound < whole_reach,
            "{:?} light: bound {bound} B is no tighter than the whole reach's {whole_reach} B",
            light.light_type
        );
    }
}
