// Stage-level reflection-probe tests: a probe captured end to end over a synthetic room.
// See: context/lib/resource_management.md §5

use std::collections::{HashMap, HashSet};
use std::path::Path;

use glam::Vec3;
use postretro_level_format::alpha_lights::{
    AlphaFalloffModel, AlphaLightRecord, AlphaLightType, AlphaLightsSection, AlphaShadowType,
};
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::animated_light_weight_maps::{
    AnimatedBlock, AnimatedLightWeightMapsSection, ChunkAtlasRect, TexelLight, TexelLightEntry,
};
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::delta_sh_volumes::{AFFINITY_FACTOR, DeltaShVolumesSection};
use postretro_level_format::lightmap::{
    IRRADIANCE_FORMAT_RGBA16F, LightmapBlock, LightmapMode, LightmapSection, f32_to_f16_bits,
};
use postretro_level_format::octahedral::irradiance_tile_texel_coord;
use postretro_level_format::prm::{
    PrmFile, PrmFormat, PrmHeader, PrmSlot, PrmSlots, STAGE_VERSION, cache_filename_for_key,
};
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_TILE_BORDER, REFLECTION_PROBE_TILE_DIMENSION, ReflectionProbeBinding,
    ReflectionProbeLightShare, ReflectionProbesSection,
};
use postretro_level_format::sh_reconstruct::Level;
use postretro_level_format::sh_volume::{AnimationDescriptor, OctahedralShVolumeSection};

use super::reflection_probes_test_fixtures::{
    GeometryBuilder, dense_sh_volume, fluid_volume, rect, up_rect, volumes,
};
use super::*;
use crate::bc6h::decode_bc6h_rgb_to_f32_rgba;
use crate::bvh_build::build_bvh;

/// No BSP: no capture point is in solid or outside the hull.
fn empty_tree() -> BspTree {
    BspTree {
        nodes: Vec::new(),
        leaves: Vec::new(),
    }
}

/// A 4×4 diffuse `.prm` of one sRGB colour, at `key`.
fn write_solid_prm(dir: &Path, key: [u8; 32], rgba: [u8; 4]) {
    let mut payload = Vec::new();
    for texels in [16, 4, 1] {
        for _ in 0..texels {
            payload.extend_from_slice(&rgba);
        }
    }
    let prm = PrmFile {
        header: PrmHeader {
            stage_version: STAGE_VERSION,
            slot_mask: PrmSlots::DIFFUSE,
            bundle_hash: [0; 32],
            total_body_bytes: 0,
            layer_count: 1,
        },
        slots: [
            Some(PrmSlot {
                format: PrmFormat::Rgba8UnormSrgb,
                width: 4,
                height: 4,
                level_count: 3,
                payload,
            }),
            None,
            None,
            None,
        ],
    };
    std::fs::write(
        dir.join(format!("{}.prm", cache_filename_for_key(&key))),
        prm.to_bytes().unwrap(),
    )
    .unwrap();
}

/// A uniform `width × height` RGBA16F lightmap block.
pub(super) fn uniform_block(width: u16, height: u16, irradiance: f32) -> LightmapBlock {
    let texel: Vec<u8> = [irradiance, irradiance, irradiance, 1.0]
        .iter()
        .flat_map(|v| f32_to_f16_bits(*v).to_le_bytes())
        .collect();
    LightmapBlock {
        cell_id: 0,
        width,
        height,
        irradiance: texel.repeat(usize::from(width) * usize::from(height)),
        direction: Vec::new(),
    }
}

/// The decoded tile texel a direction lands on.
pub(super) fn tile_texel(section: &ReflectionProbesSection, layer: usize, direction: Vec3) -> Vec3 {
    let dimension = section.tile_dimension;
    let decoded = decode_bc6h_rgb_to_f32_rgba(section.tile(layer), dimension, dimension).unwrap();
    let [x, y] = irradiance_tile_texel_coord(
        direction.normalize().to_array(),
        REFLECTION_PROBE_TILE_DIMENSION,
        REFLECTION_PROBE_TILE_BORDER,
    );
    let at = ((y as u32 * dimension + x as u32) * 4) as usize;
    Vec3::new(decoded[at], decoded[at + 1], decoded[at + 2])
}

/// Floor and ceiling face indices in [`lit_floor_room`]: each face owns four
/// consecutive vertices.
pub(super) const FLOOR: usize = 0;
pub(super) const CEILING: usize = 1;

/// The stage inputs beyond the room's geometry, lightmap and materials.
pub(super) struct Extras {
    pub sh: OctahedralShVolumeSection,
    pub delta: Option<DeltaShVolumesSection>,
    pub weight_maps: Option<AnimatedLightWeightMapsSection>,
    pub animated_direct: Option<AnimatedDirectShDeltaVolumesSection>,
    pub alpha_lights: AlphaLightsSection,
    pub chunk_list: ChunkLightListSection,
    pub authored: Vec<MapReflectionVolume>,
}

impl Default for Extras {
    fn default() -> Self {
        Self {
            sh: OctahedralShVolumeSection::placeholder(),
            delta: None,
            weight_maps: None,
            animated_direct: None,
            alpha_lights: AlphaLightsSection::default(),
            chunk_list: ChunkLightListSection::placeholder(),
            authored: Vec::new(),
        }
    }
}

/// A 6 × 3 × 6 m room whose floor (lightmap block 1, irradiance 1) is
/// textured `floor_rgba`; the ceiling holds lightmap block 2 at irradiance 0,
/// and every other face is unlit grey.
pub(super) struct LitFloorRoom {
    pub geometry: crate::geometry::GeometryResult,
    pub lightmap: LightmapSection,
    pub keys: HashMap<String, [u8; 32]>,
    pub prm_dir: tempfile::TempDir,
    pub extras: Extras,
}

pub(super) fn lit_floor_room(floor_rgba: [u8; 4]) -> LitFloorRoom {
    lit_floor_room_with(floor_rgba, |_| {})
}

/// [`lit_floor_room`] with more faces pushed after the room's six.
pub(super) fn lit_floor_room_with(
    floor_rgba: [u8; 4],
    more: impl FnOnce(&mut GeometryBuilder),
) -> LitFloorRoom {
    let (lo, hi) = (Vec3::new(-3.0, 0.0, -3.0), Vec3::new(3.0, 3.0, 3.0));
    let mut builder = GeometryBuilder::default();
    let corner_uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    builder.push_face(Vec3::Y, &rect(lo, hi, 1, lo.y), 0, lo, 1, corner_uvs);
    builder.push_face(Vec3::NEG_Y, &rect(lo, hi, 1, hi.y), 1, lo, 2, corner_uvs);
    for (normal, axis, value) in [
        (Vec3::X, 0, lo.x),
        (Vec3::NEG_X, 0, hi.x),
        (Vec3::Z, 2, lo.z),
        (Vec3::NEG_Z, 2, hi.z),
    ] {
        builder.push_face(normal, &rect(lo, hi, axis, value), 1, lo, 0, [[0.0; 2]; 4]);
    }
    more(&mut builder);
    let geometry = builder.finish(vec!["floor".to_string(), "wall".to_string()]);
    let prm_dir = tempfile::tempdir().unwrap();
    write_solid_prm(prm_dir.path(), [1; 32], floor_rgba);
    write_solid_prm(prm_dir.path(), [2; 32], [128, 128, 128, 255]);
    let keys = HashMap::from([
        ("floor".to_string(), [1u8; 32]),
        ("wall".to_string(), [2u8; 32]),
    ]);
    LitFloorRoom {
        geometry,
        lightmap: LightmapSection {
            direction_texel_scale: 1,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            mode: LightmapMode::Shadowed,
            blocks: vec![uniform_block(8, 8, 1.0), uniform_block(8, 8, 0.0)],
        },
        keys,
        prm_dir,
        extras: Extras::default(),
    }
}

/// The pool most tests reflect: a 2 m square at 0.5 m, so the capture point
/// sits at (0, 0.502, 0).
pub(super) fn pool() -> EnvironmentVolumesSection {
    volumes(vec![fluid_volume(5, &[up_rect(-1.0, -1.0, 1.0, 1.0, 0.5)])])
}

/// Run `f` over the stage inputs of `room` and `environment`.
pub(super) fn with_inputs<R>(
    room: &LitFloorRoom,
    environment: &EnvironmentVolumesSection,
    f: impl FnOnce(&ReflectionProbeInputs<'_>) -> R,
) -> R {
    let (bvh, primitives, _) = build_bvh(&room.geometry).unwrap();
    let tree = empty_tree();
    let exterior = HashSet::new();
    let extras = &room.extras;
    // No brushes: the synthetic room is faces only, and no probe excludes.
    let face_brushes = vec![None; room.geometry.geometry.faces.len()];
    f(&ReflectionProbeInputs {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &room.geometry,
        environment_volumes: environment,
        reflection_volumes: &extras.authored,
        brush_volumes: &[],
        face_brushes: &face_brushes,
        capture_override: &[],
        tree: &tree,
        exterior_leaves: &exterior,
        lightmap: &room.lightmap,
        sh_volume: &extras.sh,
        delta_sh: extras.delta.as_ref(),
        animated_weight_maps: extras.weight_maps.as_ref(),
        animated_direct_sh: extras.animated_direct.as_ref(),
        alpha_lights: &extras.alpha_lights,
        chunk_light_list: &extras.chunk_list,
        texture_keys: &room.keys,
        prm_root: room.prm_dir.path(),
    })
}

pub(super) fn bake(
    room: &LitFloorRoom,
    environment: &EnvironmentVolumesSection,
) -> Option<ReflectionProbesSection> {
    with_inputs(room, environment, |inputs| {
        bake_or_load_reflection_probes(inputs, None, &BakeControl::unrestricted()).unwrap()
    })
}

/// An animated light's descriptor: `brightness` keys over a 1 s loop at
/// `base_color`.
pub(super) fn descriptor(base_color: [f32; 3], brightness: Vec<f32>) -> AnimationDescriptor {
    AnimationDescriptor {
        period: 1.0,
        phase: 0.0,
        base_color,
        brightness,
        color: Vec::new(),
        direction: Vec::new(),
        start_active: 1,
    }
}

/// Give `faces` of `room` animated direct: face `f` lit by `(light, weight)`
/// pairs, uniform over its 8×8 lightmap block. Each face's block takes its
/// own 8² page, so its bilinear reads clamp at the page edge rather than
/// reaching a neighbour.
pub(super) fn uniform_weight_maps(room: &mut LitFloorRoom, faces: &[(usize, &[(u32, f32)])]) {
    let mut section = AnimatedLightWeightMapsSection {
        page_size: 8,
        compact_layers: faces.len() as u32,
        ..AnimatedLightWeightMapsSection::empty()
    };
    for (k, &(face, lights)) in faces.iter().enumerate() {
        let lightmap_block = room.geometry.geometry.vertices[face * 4].lightmap_block;
        assert!(lightmap_block > 0, "face {face} has no lightmap block");
        for v in &mut room.geometry.geometry.vertices[face * 4..face * 4 + 4] {
            v.animated_block = k as u16 + 1;
        }
        section.blocks.push(AnimatedBlock {
            lightmap_block: u32::from(lightmap_block) - 1,
            block_x: 0,
            block_y: 0,
            compact_x: 0,
            compact_y: 0,
            compact_layer: k as u32,
            width: 8,
            height: 8,
        });
        section.chunk_rects.push(ChunkAtlasRect {
            compact_x: 0,
            compact_y: 0,
            width: 8,
            height: 8,
            texel_offset: section.offset_counts.len() as u32,
            block: k as u32,
        });
        for _ in 0..64 {
            section.offset_counts.push(TexelLightEntry {
                offset: section.texel_lights.len() as u32,
                count: lights.len() as u32,
            });
            for &(light_index, weight) in lights {
                section.texel_lights.push(TexelLight {
                    light_index,
                    weight,
                    direction_oct: [0, 0],
                });
            }
        }
    }
    room.extras.weight_maps = Some(section);
}

/// A packed base SH volume over the room, every probe's tile uniform at
/// `value`, one probe per metre from (−3, 0, −3).
pub(super) fn room_sh(value: f32) -> OctahedralShVolumeSection {
    let mut dense = dense_sh_volume([7, 4, 7], |_, _, _| [value; 3]);
    dense.grid_origin = [-3.0, 0.0, -3.0];
    crate::sh_density::pack_indirect_section(dense, Some(Level::L0))
        .unwrap()
        .0
}

/// Id 27 over `base`'s grid: light `light` of `roster` carries a uniform
/// delta `value` at every valid probe, all L0.
pub(super) fn uniform_delta(
    base: &OctahedralShVolumeSection,
    roster: u32,
    light: u32,
    value: [f32; 3],
) -> DeltaShVolumesSection {
    let factor = u32::from(AFFINITY_FACTOR);
    let grid = base.grid_dimensions;
    let dims = grid.map(|axis| axis.div_ceil(factor));
    let tile = base.tile_dimension as usize;
    let mut masks = Vec::new();
    let mut offsets = vec![0u32];
    let mut lights = Vec::new();
    let mut payload = Vec::new();
    for bz in 0..dims[2] {
        for by in 0..dims[1] {
            for bx in 0..dims[0] {
                let mut mask = 0u64;
                for local in 0..64u32 {
                    let (lx, ly, lz) = (local % 4, (local / 4) % 4, local / 16);
                    let (x, y, z) = (bx * 4 + lx, by * 4 + ly, bz * 4 + lz);
                    if x < grid[0] && y < grid[1] && z < grid[2] {
                        let probe = (x + y * grid[0] + z * grid[0] * grid[1]) as usize;
                        if base.probes[probe].validity != 0 {
                            mask |= 1 << local;
                        }
                    }
                }
                masks.push(mask);
                if mask != 0 {
                    lights.push(light);
                    for _ in 0..mask.count_ones() as usize * tile * tile {
                        payload.extend(value.map(f32_to_f16_bits));
                    }
                }
                offsets.push(lights.len() as u32);
            }
        }
    }
    DeltaShVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: dims,
        tile_dimension: base.tile_dimension,
        tile_border: base.tile_border,
        animation_descriptor_indices: (0..roster).collect(),
        valid_probe_masks: masks,
        cell_levels: vec![0; (dims[0] * dims[1] * dims[2]) as usize],
        affinity_offsets: offsets,
        affinity_lights: lights,
        delta_subblocks: payload,
    }
}

/// Id 45 carrying only `descriptors`, its descriptor table: the stage reads
/// nothing else of it.
pub(super) fn animated_direct_table(descriptors: Vec<u32>) -> AnimatedDirectShDeltaVolumesSection {
    AnimatedDirectShDeltaVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: [0; 3],
        tile_dimension: 0,
        tile_border: 0,
        animation_descriptor_indices: descriptors,
        valid_probe_masks: Vec::new(),
        cell_levels: Vec::new(),
        affinity_offsets: vec![0],
        affinity_lights: Vec::new(),
        delta_subblocks: Vec::new(),
    }
}

/// A runtime light record, point type, linear falloff.
pub(super) fn alpha_light(
    origin: [f64; 3],
    color: [f32; 3],
    range: f32,
    shadow_type: AlphaShadowType,
) -> AlphaLightRecord {
    AlphaLightRecord {
        origin,
        light_type: AlphaLightType::Point,
        intensity: 1.0,
        color,
        falloff_model: AlphaFalloffModel::Linear,
        falloff_range: range,
        cone_angle_inner: 0.0,
        cone_angle_outer: 0.0,
        cone_direction: [0.0; 3],
        is_dynamic: false,
        casts_entity_shadows: false,
        leaf_index: 0,
        shadow_type,
    }
}

// Acceptance row 11: the capture traces the lower hemisphere too.
#[test]
fn probe_over_a_pool_with_a_red_floor_reads_red_straight_down() {
    let room = lit_floor_room([255, 0, 0, 255]);
    let section = bake(&room, &pool()).expect("the pool keeps its probe");

    assert_eq!(section.probes.len(), 1);
    let probe = &section.probes[0];
    assert_eq!(probe.source_entity, 5);
    assert_eq!(probe.lights, [ReflectionProbeLightShare::UNUSED; 4]);
    assert!((probe.capture_point[1] - 0.502).abs() < 1e-5, "{probe:?}");
    assert_eq!(
        section.bindings,
        vec![ReflectionProbeBinding {
            volume: 0,
            face: 0,
            probe: 0
        }]
    );

    // Straight down through the water: the lit red floor.
    let down = tile_texel(&section, 0, Vec3::NEG_Y);
    assert!(down.x > 0.9 && down.y < 0.02 && down.z < 0.02, "{down}");
    // Down at a slant still lands on the floor, in the tile's lower half.
    let slant = tile_texel(&section, 0, Vec3::new(0.5, -1.0, 0.3));
    assert!(slant.x > 0.9 && slant.y < 0.02, "{slant}");
    // Up: the unlit ceiling reflects black.
    let up = tile_texel(&section, 0, Vec3::Y);
    assert!(up.length() < 0.01, "{up}");

    // The emitted section is canonical: it decodes to itself.
    let decoded = ReflectionProbesSection::from_bytes(&section.to_bytes()).unwrap();
    assert_eq!(decoded, section);
}

#[test]
fn a_level_without_up_facing_fluid_emits_no_section() {
    let room = lit_floor_room([255, 0, 0, 255]);
    assert_eq!(bake(&room, &volumes(Vec::new())), None);
    // A fluid face that only faces sideways derives nothing either.
    let side = volumes(vec![fluid_volume(
        1,
        &[vec![
            [1.0, 0.0, -1.0],
            [1.0, 1.0, -1.0],
            [1.0, 1.0, 1.0],
            [1.0, 0.0, 1.0],
        ]],
    )]);
    assert_eq!(bake(&room, &side), None);
}

// Acceptance row 1: a level whose up-facing faces all lose their probes
// emits no section. The pool's derived capture point lies inside another
// fluid entity's brush, so placement keeps nothing and the stage writes no
// id 54, rather than an empty one.
#[test]
fn a_level_whose_faces_all_lose_their_probes_emits_no_section() {
    use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
    let capture = postretro_test_log_capture::LogCapture::start();
    let room = lit_floor_room([255, 0, 0, 255]);
    let plane = |normal: [f32; 3], distance: f32| EnvironmentPlane { normal, distance };
    let mut sludge = fluid_volume(6, &[]);
    sludge.fluid = "sludge".to_string();
    sludge.brushes = vec![EnvironmentBrush {
        aabb_min: [-2.0, 0.0, -2.0],
        aabb_max: [2.0, 1.0, 2.0],
        planes: vec![
            plane([1.0, 0.0, 0.0], 2.0),
            plane([-1.0, 0.0, 0.0], 2.0),
            plane([0.0, 1.0, 0.0], 1.0),
            plane([0.0, -1.0, 0.0], 0.0),
            plane([0.0, 0.0, 1.0], 2.0),
            plane([0.0, 0.0, -1.0], 2.0),
        ],
    }];
    let mut environment = pool();
    environment.volumes.push(sludge);
    assert_eq!(bake(&room, &environment), None);
    capture.assert_logged_once(log::Level::Warn, "derived probe of fluid entity 5");
    capture.assert_logged_once(log::Level::Warn, "is inside fluid entity 6; dropped");
    // The same pool with nothing over its point keeps its probe.
    assert!(bake(&room, &pool()).is_some());
}

/// A room with every capture term: static lightmap, base and delta SH, two
/// animated lights' weight maps (one tied with a third for the K-th entry)
/// and an sdf light.
fn every_term_room() -> LitFloorRoom {
    let mut room = lit_floor_room([200, 60, 20, 255]);
    room.extras.sh = room_sh(0.25);
    room.extras.sh.animation_descriptors = vec![
        descriptor([1.0, 0.5, 0.25], vec![1.0, 0.0, 0.0, 1.0]),
        descriptor([0.25, 0.5, 1.0], Vec::new()),
        descriptor([0.5; 3], vec![0.3, 0.9]),
        descriptor([0.5; 3], vec![0.3, 0.9]),
        descriptor([2.0; 3], Vec::new()),
    ];
    room.extras.delta = Some(uniform_delta(&room.extras.sh, 5, 1, [0.5, 0.25, 0.125]));
    // Light 0 on the floor too, below the horizon: in the tile, not the
    // shares. Every light reaches the ceiling, above it, so all five rank.
    uniform_weight_maps(
        &mut room,
        &[
            (FLOOR, &[(0, 0.5)]),
            (
                CEILING,
                &[(0, 0.5), (1, 0.75), (2, 0.1), (3, 0.1), (4, 0.05)],
            ),
        ],
    );
    room.extras.alpha_lights = AlphaLightsSection {
        lights: vec![alpha_light(
            [2.0, 2.5, 2.0],
            [1.0, 0.8, 0.6],
            6.0,
            AlphaShadowType::Sdf,
        )],
    };
    room
}

// Row 8's premise at the stage and pin P22: every term at once, a tie for
// the K-th entry included, at one worker and at four.
#[test]
fn capture_is_byte_identical_across_worker_counts() {
    let room = every_term_room();
    let environment = volumes(vec![
        fluid_volume(2, &[up_rect(-2.0, -2.0, 0.0, 0.0, 0.5)]),
        fluid_volume(3, &[up_rect(1.0, 1.0, 2.0, 2.0, 1.0)]),
    ]);
    let run = |threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| bake(&room, &environment))
            .unwrap()
    };
    let one = run(1);
    let four = run(4);
    assert_eq!(one.probes.len(), 2);
    // The distance tiles are part of the identity: both are real payloads.
    for layer in 0..2 {
        assert!(one.distance_tile(layer).iter().any(|&b| b != 0));
    }
    assert_eq!(one.distances, four.distances);
    assert_eq!(one.to_bytes(), four.to_bytes());
    assert_eq!(one.to_bytes(), run(1).to_bytes());
    // Lights 2 and 3 tie; the lower roster index takes the fourth entry.
    let stored: Vec<u32> = one.probes[0]
        .lights
        .iter()
        .map(|l| l.roster_index)
        .collect();
    assert!(stored.contains(&2) && !stored.contains(&3), "{stored:?}");
}

/// The stage's cache key for `room` and `environment`, placement run as the
/// stage runs it.
fn stage_key(room: &LitFloorRoom, environment: &EnvironmentVolumesSection) -> String {
    with_inputs(room, environment, |inputs| {
        let tracer = SceneTracer::new(inputs.bvh, inputs.primitives, inputs.geometry);
        let bounds = WorldBounds::from_positions(
            room.geometry.geometry.vertices.iter().map(|v| &v.position),
        )
        .unwrap();
        let classifier = PointClassifier::new(inputs.tree, inputs.exterior_leaves, environment);
        let control = BakeControl::unrestricted();
        let probes = place_probes(
            environment,
            inputs.reflection_volumes,
            &PlacementScene {
                tracer: &tracer,
                bounds: &bounds,
                classifier: &classifier,
                governor: control.governor(),
            },
            MAX_REFLECTION_PROBES,
        )
        .unwrap();
        let brush_faces = BrushFaces::new(inputs.face_brushes, 0);
        let brushes = CaptureBrushes {
            volumes: inputs.brush_volumes,
            face_brushes: inputs.face_brushes,
            faces: &brush_faces,
        };
        let exclusions: Vec<ProbeExclusion<'_>> = probes
            .iter()
            .map(|probe| ProbeExclusion::new(&tracer, brushes, &[], probe))
            .collect();
        cache_key(
            inputs,
            &probes,
            &exclusions,
            &ExclusionConstants::PRODUCTION,
        )
        .unwrap()
        .as_filename()
    })
}

fn probe_box(min: [f32; 3], max: [f32; 3], offset: Option<[f32; 2]>) -> MapReflectionVolume {
    MapReflectionVolume {
        source_entity: 6,
        source_label: "entity 6 'reflection_volume'".to_string(),
        min,
        max,
        capture_offset: offset,
    }
}

// Acceptance row 9: unchanged input hits; each input the capture reads
// misses when it changes.
#[test]
fn cache_key_misses_on_every_input_the_capture_reads_and_hits_otherwise() {
    let mut room = every_term_room();
    let environment = pool();
    let base = stage_key(&room, &environment);
    assert_eq!(base, stage_key(&room, &environment), "unchanged input hits");
    let mut misses = 0;
    let mut expect_miss = |room: &LitFloorRoom, environment: &EnvironmentVolumesSection, what| {
        assert_ne!(base, stage_key(room, environment), "{what}");
        misses += 1;
    };

    // A moved fluid face.
    let moved = volumes(vec![fluid_volume(5, &[up_rect(-1.0, -1.0, 1.0, 1.0, 0.6)])]);
    expect_miss(&room, &moved, "fluid face");

    // A moved wall: the east wall a hair further out.
    let east = 3 * 4;
    let saved = room.geometry.geometry.vertices.clone();
    for v in &mut room.geometry.geometry.vertices[east..east + 4] {
        v.position[0] += 0.01;
    }
    expect_miss(&room, &environment, "wall");
    room.geometry.geometry.vertices = saved;

    let lightmap = room.lightmap.clone();
    room.lightmap.blocks[0] = uniform_block(8, 8, 2.0);
    expect_miss(&room, &environment, "lightmap");
    room.lightmap = lightmap;

    // Albedo and emissive both live in a material's content-addressed
    // `.prm`, so a changed texture is a changed key.
    room.keys.insert("floor".to_string(), [9; 32]);
    expect_miss(&room, &environment, "albedo or emissive texture");
    room.keys.insert("floor".to_string(), [1; 32]);

    let sh = room.extras.sh.clone();
    room.extras.sh = room_sh(0.5);
    room.extras.sh.animation_descriptors = sh.animation_descriptors.clone();
    expect_miss(&room, &environment, "base SH");
    room.extras.sh = sh;

    let delta = room.extras.delta.clone();
    room.extras.delta = Some(uniform_delta(&room.extras.sh, 5, 1, [0.5, 0.25, 0.25]));
    expect_miss(&room, &environment, "delta SH");
    room.extras.delta = delta;

    let maps = room.extras.weight_maps.clone();
    room.extras.weight_maps.as_mut().unwrap().texel_lights[0].weight = 0.4;
    expect_miss(&room, &environment, "animated weight map");
    room.extras.weight_maps = maps;

    // Id 45 enters by its descriptor table alone, which the roster check
    // reads on a miss: a table that changes misses; the rest of id 45, which
    // the capture never reads, keeps the key.
    room.extras.animated_direct = Some(animated_direct_table((0..5).collect()));
    let with_table = stage_key(&room, &environment);
    expect_miss(&room, &environment, "animated direct descriptor table");
    room.extras
        .animated_direct
        .as_mut()
        .unwrap()
        .delta_subblocks = vec![1, 2, 3];
    assert_eq!(
        with_table,
        stage_key(&room, &environment),
        "id 45's payload"
    );
    room.extras
        .animated_direct
        .as_mut()
        .unwrap()
        .animation_descriptor_indices[4] = u32::MAX;
    assert_ne!(
        with_table,
        stage_key(&room, &environment),
        "a non-identity id-45 table"
    );
    room.extras.animated_direct = None;

    room.extras.sh.animation_descriptors[0].brightness[1] = 0.5;
    expect_miss(&room, &environment, "animated light curve");
    room.extras.sh.animation_descriptors[0].brightness[1] = 0.0;

    room.extras.alpha_lights.lights[0].origin[1] = 2.25;
    expect_miss(&room, &environment, "sdf light moved");
    room.extras.alpha_lights.lights[0].origin[1] = 2.5;
    room.extras.alpha_lights.lights[0].shadow_type = AlphaShadowType::StaticLightMap;
    expect_miss(&room, &environment, "_shadow_type sdf turned off");
    room.extras.alpha_lights.lights[0].shadow_type = AlphaShadowType::Sdf;

    let mut grid = ChunkLightListSection::placeholder();
    grid.has_grid = 1;
    grid.cell_size = 8.0;
    grid.grid_dimensions = [1, 1, 1];
    grid.grid_origin = [-4.0; 3];
    grid.offsets = vec![postretro_level_format::chunk_light_list::ChunkEntry {
        offset: 0,
        count: 1,
    }];
    grid.light_indices = vec![0];
    room.extras.chunk_list = grid;
    expect_miss(&room, &environment, "chunk light list");
    room.extras.chunk_list = ChunkLightListSection::placeholder();

    // A dynamic light the capture never reads keeps the key.
    let mut dynamic = alpha_light(
        [0.0, 2.0, 0.0],
        [1.0; 3],
        4.0,
        AlphaShadowType::StaticLightMap,
    );
    dynamic.is_dynamic = true;
    room.extras.alpha_lights.lights.push(dynamic);
    assert_eq!(base, stage_key(&room, &environment), "a dynamic-tier light");
    room.extras.alpha_lights.lights.pop();

    // Authored boxes enter as placement output: a box, a moved box and a
    // changed capture offset each miss.
    room.extras.authored = vec![probe_box([-2.0, 0.0, -2.0], [2.0, 2.5, 2.0], None)];
    let with_box = stage_key(&room, &environment);
    assert_ne!(base, with_box, "an authored probe");
    assert_eq!(with_box, stage_key(&room, &environment));
    room.extras.authored = vec![probe_box([-2.5, 0.0, -2.0], [2.0, 2.5, 2.0], None)];
    assert_ne!(
        with_box,
        stage_key(&room, &environment),
        "a moved probe brush"
    );
    room.extras.authored = vec![probe_box(
        [-2.0, 0.0, -2.0],
        [2.0, 2.5, 2.0],
        Some([0.25, 0.0]),
    )];
    let offset = stage_key(&room, &environment);
    assert_ne!(with_box, offset, "an authored capture offset");
    room.extras.authored = vec![probe_box(
        [-2.0, 0.0, -2.0],
        [2.0, 2.5, 2.0],
        Some([0.5, 0.0]),
    )];
    assert_ne!(
        offset,
        stage_key(&room, &environment),
        "a changed capture offset"
    );

    assert_eq!(base, {
        room.extras.authored.clear();
        stage_key(&room, &environment)
    });
    assert_eq!(misses, 12);
}

#[test]
fn unchanged_inputs_hit_the_stage_cache_with_the_same_section() {
    use log::Level as LogLevel;
    use postretro_test_log_capture::LogCapture;

    let room = every_term_room();
    let environment = pool();
    let dir = tempfile::tempdir().unwrap();
    let cache = StageCache::new(dir.path()).unwrap();
    let control = BakeControl::unrestricted();
    let logs = LogCapture::start();
    let (cold, warm) = with_inputs(&room, &environment, |inputs| {
        let cold = bake_or_load_reflection_probes(inputs, Some(&cache), &control).unwrap();
        let cold_logs = logs.records().len();
        let warm = bake_or_load_reflection_probes(inputs, Some(&cache), &control).unwrap();
        assert!(logs.records().len() > cold_logs);
        (cold, warm)
    });
    let access = cache.test_access(STAGE_ID);
    assert_eq!((access.writes, access.read_hits), (1, 1));
    assert_eq!(cold, warm);
    // The warm hit logs the covered share the cold capture computed.
    let covered: Vec<String> = logs
        .records()
        .into_iter()
        .filter(|r| r.level == LogLevel::Info && r.message.contains("animated light(s); stored"))
        .map(|r| r.message)
        .collect();
    assert_eq!(covered.len(), 2, "{covered:?}");
    assert_eq!(covered[0], covered[1]);
}

/// The stage's error for `room`, or `None` when it bakes.
fn bake_error(room: &LitFloorRoom) -> Option<String> {
    with_inputs(room, &pool(), |inputs| {
        bake_or_load_reflection_probes(inputs, None, &BakeControl::unrestricted())
            .err()
            .map(|e| format!("{e:#}"))
    })
}

// A share's roster index indexes the runtime descriptor buffer directly, so
// the loader refuses section 54 over a non-identity descriptor table in id
// 27 or id 45; the build refuses it first, naming the table.
#[test]
fn a_non_identity_descriptor_table_in_either_delta_section_refuses_the_build() {
    let mut room = every_term_room();
    room.extras.animated_direct = Some(animated_direct_table((0..5).collect()));
    assert_eq!(bake_error(&room), None, "identity tables bake");

    room.extras
        .animated_direct
        .as_mut()
        .unwrap()
        .animation_descriptor_indices[4] = u32::MAX;
    let message = bake_error(&room).expect("a non-identity id-45 table refuses");
    assert!(
        message.contains(
            "AnimatedDirectShDeltaVolumes (id 45) maps animated light 4 to descriptor 4294967295"
        ) && message.contains("need the identity roster"),
        "{message}"
    );

    room.extras.animated_direct = Some(animated_direct_table((0..4).collect()));
    let message = bake_error(&room).expect("a short id-45 table refuses");
    assert!(
        message.contains("AnimatedDirectShDeltaVolumes (id 45) has 4 animated light(s)"),
        "{message}"
    );

    room.extras.animated_direct = Some(animated_direct_table((0..5).collect()));
    room.extras
        .delta
        .as_mut()
        .unwrap()
        .animation_descriptor_indices[0] = 1;
    let message = bake_error(&room).expect("a non-identity id-27 table refuses");
    assert!(
        message.contains("DeltaShVolumes (id 27) maps animated light 0 to descriptor 1"),
        "{message}"
    );
}
