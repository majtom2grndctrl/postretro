// Fluid reflection probe table, probe index and CPU reference tests.
// See: context/lib/testing_guide.md

use std::cell::Cell;

use super::*;
use crate::fluid_surface::fog::{FluidFogFragment, fog_fragment};
use crate::fluid_surface::pack_fluid_surfaces;
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex,
};
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H;
use postretro_level_format::octahedral::irradiance_tile_texel_coord;
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_NO_LIGHT, REFLECTION_PROBE_TILE_BORDER, REFLECTION_PROBE_TILE_DIMENSION,
    ReflectionProbeBinding,
};

const TILE: [u32; 2] = [
    REFLECTION_PROBE_TILE_DIMENSION,
    REFLECTION_PROBE_TILE_BORDER,
];
const NO_LIGHTS: &FluidReflectionLights<'static> = &FluidReflectionLights::NONE;

/// A distance tile holding no surface: every search ends at the box hit,
/// plain box projection.
fn no_surface(_layer: u32, _uv: Vec2) -> f32 {
    0.0
}

fn assert_close(actual: f32, expected: f32, tolerance: f32, what: &str) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{what}: {actual} vs {expected} (± {tolerance})"
    );
}

fn assert_vec_close(actual: Vec3, expected: Vec3, tolerance: f32, what: &str) {
    assert!(
        actual.abs_diff_eq(expected, tolerance),
        "{what}: {actual} vs {expected} (± {tolerance})"
    );
}

/// The east-wing shape: a room 10 × 8 m, 4 m tall, over a pool whose plane is
/// y = 0, fitted 1 m below it to take in the pit's sides.
fn room_probe(capture_point: [f32; 3], layer: u32) -> ReflectionProbeRecord {
    ReflectionProbeRecord {
        source_entity: 7,
        box_min: [-5.0, -1.0, -4.0],
        box_max: [5.0, 4.0, 4.0],
        capture_point,
        layer,
        lights: [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE],
    }
}

fn level_top(x: f32, z: f32, side: f32) -> EnvironmentSurfaceFace {
    let corner = |x: f32, z: f32| EnvironmentSurfaceVertex {
        position: [x, 0.0, z],
        uv: [x * 16.0, z * 16.0],
    };
    // CCW seen from +Y.
    EnvironmentSurfaceFace {
        texture_index: 0,
        vertices: vec![
            corner(x, z),
            corner(x, z + side),
            corner(x + side, z + side),
            corner(x + side, z),
        ],
    }
}

fn fluid(faces: Vec<EnvironmentSurfaceFace>) -> EnvironmentVolumeRecord {
    EnvironmentVolumeRecord {
        source_entity: 0,
        priority: 0,
        gravity: None,
        push: None,
        fluid: "water".to_string(),
        brushes: Vec::new(),
        surface_faces: faces,
    }
}

fn section(
    probes: Vec<ReflectionProbeRecord>,
    bindings: Vec<ReflectionProbeBinding>,
) -> ReflectionProbesSection {
    let tile = (REFLECTION_PROBE_TILE_DIMENSION as usize / 4).pow(2) * 16;
    ReflectionProbesSection {
        tile_dimension: REFLECTION_PROBE_TILE_DIMENSION,
        tile_border: REFLECTION_PROBE_TILE_BORDER,
        atlas_format: IRRADIANCE_FORMAT_BC6H,
        tiles: vec![0; tile * probes.len()],
        distance_format:
            postretro_level_format::reflection_probes::REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1,
        distances: postretro_level_format::reflection_probes::reflection_probe_zero_distances(
            probes.len(),
            REFLECTION_PROBE_TILE_DIMENSION,
        ),
        probes,
        bindings,
    }
}

fn binding(volume: u32, face: u32, probe: u32) -> ReflectionProbeBinding {
    ReflectionProbeBinding {
        volume,
        face,
        probe,
    }
}

/// A fragment on the pool at `world_position`, bound to probe 0 at
/// reflectivity 1, seen from `camera` with every lighting term on, in the
/// smooth style (the per-fragment reflection these rows measure; the pixel
/// style's cells are `reflection_style_tests`').
fn pool_fragment(camera: Vec3, world_position: Vec3) -> FluidReflectionFragment {
    FluidReflectionFragment {
        probe: 0,
        camera,
        world_position,
        outward: Vec3::Y,
        light_term_mask: LightTermMask::ALL.bits(),
        reflectivity: 1.0,
        style: crate::fluid_surface::reflection_style::FLUID_REFLECTION_STYLE_SMOOTH,
        texel_uv: Vec2::new(world_position.x, world_position.z) * 8.0,
        axes: crate::fluid_surface::reflection_style::FluidTexelAxes {
            u: Vec3::X / 8.0,
            v: Vec3::Z / 8.0,
        },
        time: 0.0,
        texels_per_pixel: 0.5,
    }
}

// --- Probe table and per-vertex index ---------------------------------------

// The table is laid out at the cap: header, then probes at a 240-byte stride
// (box min, box max, capture point with its layer, then K 48-byte entries of
// RGB reference, roster index and unorm10×3 shares). Unused entries pack as
// the reserved roster index with zero values.
#[test]
fn probe_table_packs_every_probe_at_its_stride_and_zeros_the_rest() {
    let mut lit = room_probe([1.0, 0.002, -0.5], 1);
    lit.lights[0] = ReflectionProbeLightShare {
        roster_index: 3,
        reference: [0.5, 1.0, 2.0],
        shares: [[1.0, 0.0, 0.5], [0.0; 3], [0.25; 3], [0.0; 3], [1.0; 3]],
    };
    let probes = vec![room_probe([0.0, 0.002, 0.0], 0), lit];
    let bytes = pack_probe_table(Some(&section(probes.clone(), Vec::new())));
    assert_eq!(bytes.len(), FLUID_PROBE_TABLE_BYTES);
    assert_eq!(FLUID_PROBE_TABLE_BYTES, 16 + 256 * 240);

    let header: FluidProbeTableHeader = *bytemuck::from_bytes(&bytes[..16]);
    assert_eq!(header.tile_dimension, REFLECTION_PROBE_TILE_DIMENSION);
    assert_eq!(header.tile_border, REFLECTION_PROBE_TILE_BORDER);
    assert_eq!(header.probe_count, 2);
    let probe = |index: usize| -> FluidProbeGpu {
        let start = 16 + index * FLUID_PROBE_STRIDE;
        *bytemuck::from_bytes(&bytes[start..start + FLUID_PROBE_STRIDE])
    };
    for (index, record) in probes.iter().enumerate() {
        let gpu = probe(index);
        assert_eq!(gpu.box_min, record.box_min);
        assert_eq!(gpu.box_max, record.box_max);
        assert_eq!(gpu.capture_point, record.capture_point);
        assert_eq!(gpu.layer, record.layer);
    }
    let entry = probe(1).lights[0];
    assert_eq!(entry.roster_index, 3);
    assert_eq!(entry.reference, [0.5, 1.0, 2.0]);
    assert_eq!(entry.shares[0], 1023 | (512 << 20));
    assert_eq!(entry.shares[2], 256 | (256 << 10) | (256 << 20));
    assert_eq!(entry.shares[4], 1023 | (1023 << 10) | (1023 << 20));
    assert_eq!(&entry.shares[REFLECTION_PROBE_LOBE_COUNT..], &[0; 3]);
    for unused in &probe(0).lights {
        assert_eq!(unused.roster_index, REFLECTION_PROBE_NO_LIGHT);
        assert_eq!(unused.reference, [0.0; 3]);
        assert_eq!(unused.shares, [0; FLUID_PROBE_SHARE_WORDS]);
    }
    let past = 16 + 2 * FLUID_PROBE_STRIDE;
    assert!(bytes[past..].iter().all(|&byte| byte == 0));

    // A level with no section binds a zeroed table.
    assert!(pack_probe_table(None).iter().all(|&byte| byte == 0));
    assert_eq!(pack_probe_table(None).len(), FLUID_PROBE_TABLE_BYTES);
}

#[test]
fn unorm10x3_packs_red_lowest_and_never_carries_a_nan() {
    assert_eq!(pack_unorm10x3([0.0; 3]), 0);
    assert_eq!(pack_unorm10x3([1.0, 0.0, 0.0]), 1023);
    assert_eq!(pack_unorm10x3([0.0, 1.0, 0.0]), 1023 << 10);
    assert_eq!(pack_unorm10x3([0.0, 0.0, 1.0]), 1023 << 20);
    assert_eq!(pack_unorm10x3([2.0, -1.0, f32::NAN]), 1023);
}

// P15: a grid cut keeps each face's vertices contiguous, so its binding's
// probe covers every grid vertex it owns and no other face's.
#[test]
fn probe_indices_cover_every_grid_vertex_of_a_bound_face() {
    let volumes = vec![fluid(vec![
        level_top(0.0, 0.0, 2.5),
        level_top(2.5, 0.0, 2.5),
    ])];
    let lattice = crate::fluid_surface::FluidGridLattice::new([0.0; 3], [1.0; 3]).unwrap();
    let mesh = pack_fluid_surfaces(&volumes, &[], 1, Some(&lattice));
    let ranges: Vec<_> = mesh
        .face_drift_maps
        .iter()
        .map(|map| {
            let map = map.expect("both faces draw");
            map.vertex_start as usize..(map.vertex_start + map.vertex_count) as usize
        })
        .collect();
    assert!(
        ranges.iter().all(|range| range.len() > 4),
        "both faces gridded"
    );
    let bound = section(
        vec![room_probe([0.0, 0.002, 0.0], 0)],
        vec![binding(0, 1, 0)],
    );
    let mut indices = vec![7; mesh.vertices.len()];
    fill_probe_indices(&volumes, &mesh.face_drift_maps, Some(&bound), &mut indices);
    assert!(
        indices[ranges[0].clone()]
            .iter()
            .all(|&i| i == FLUID_REFLECTION_NO_PROBE)
    );
    assert!(indices[ranges[1].clone()].iter().all(|&i| i == 0));
    assert_eq!(ranges[1].end, mesh.vertices.len());
}

// Every vertex a bound face owns takes its binding's probe, the centroid its
// triangulation adds included; faces no binding names, and every face of a
// level without probes, take the no-probe index.
#[test]
fn probe_indices_cover_each_bound_faces_vertices_and_nothing_else() {
    // Volume 0: two faces, the second with a collinear vertex (an added
    // centroid). Volume 1 is dry. Volume 2: one face, unbound.
    let mut inserted = level_top(2.0, 0.0, 1.0);
    inserted.vertices.insert(
        1,
        EnvironmentSurfaceVertex {
            position: [2.0, 0.0, 0.5],
            uv: [32.0, 8.0],
        },
    );
    let volumes = vec![
        fluid(vec![level_top(0.0, 0.0, 1.0), inserted]),
        EnvironmentVolumeRecord {
            fluid: String::new(),
            ..fluid(Vec::new())
        },
        fluid(vec![level_top(5.0, 0.0, 1.0)]),
    ];
    let mesh = pack_fluid_surfaces(&volumes, &[], 1, None);
    assert_eq!(mesh.vertices.len(), 4 + 6 + 4, "the insert adds a centroid");
    let probes = vec![
        room_probe([0.0, 0.002, 0.0], 0),
        room_probe([2.0, 0.002, 0.0], 1),
    ];
    let bound = section(probes, vec![binding(0, 0, 1), binding(0, 1, 0)]);
    let mut indices = vec![7; mesh.vertices.len()];
    fill_probe_indices(&volumes, &mesh.face_drift_maps, Some(&bound), &mut indices);
    let mut expected = vec![1; 4];
    expected.extend([0; 6]);
    expected.extend([FLUID_REFLECTION_NO_PROBE; 4]);
    assert_eq!(indices, expected);

    fill_probe_indices(&volumes, &mesh.face_drift_maps, None, &mut indices);
    assert!(
        indices
            .iter()
            .all(|&index| index == FLUID_REFLECTION_NO_PROBE)
    );

    // A binding naming a face or probe past the level's binds nothing.
    let stale = section(
        vec![room_probe([0.0, 0.002, 0.0], 0)],
        vec![binding(0, 0, 3), binding(0, 9, 0), binding(9, 0, 0)],
    );
    fill_probe_indices(&volumes, &mesh.face_drift_maps, Some(&stale), &mut indices);
    assert!(
        indices
            .iter()
            .all(|&index| index == FLUID_REFLECTION_NO_PROBE)
    );

    assert!(volume_has_bound_face(Some(&bound), 0));
    assert!(!volume_has_bound_face(Some(&bound), 1));
    assert!(!volume_has_bound_face(Some(&bound), 2));
    assert!(!volume_has_bound_face(None, 0));
    let later = section(
        vec![room_probe([0.0, 0.002, 0.0], 0)],
        vec![binding(2, 0, 0)],
    );
    assert!(volume_has_bound_face(Some(&later), 2));
    assert!(!volume_has_bound_face(Some(&later), 0));
}

// A section whose payloads or tile format disagree with its header is
// unusable whole: no table, no probe index and no bound face, so the draw
// predicate agrees with the renderer's install, which builds on this answer.
#[test]
fn a_section_with_a_bad_payload_or_tile_format_is_unusable() {
    let volumes = vec![fluid(vec![level_top(0.0, 0.0, 1.0)])];
    let mesh = pack_fluid_surfaces(&volumes, &[], 1, None);
    let good = section(
        vec![room_probe([0.0, 0.002, 0.0], 0)],
        vec![binding(0, 0, 0)],
    );
    assert!(usable_probe_section(Some(&good)).is_some());
    assert!(volume_has_bound_face(Some(&good), 0));
    let dimension = good.tile_dimension as usize;
    let uncompressed = ReflectionProbesSection {
        atlas_format: IRRADIANCE_FORMAT_RGBA16F,
        tiles: vec![0; dimension * dimension * 8],
        ..good.clone()
    };
    assert!(usable_probe_section(Some(&uncompressed)).is_some());

    let mut short_tiles = good.clone();
    short_tiles.tiles.pop();
    let mut long_distances = good.clone();
    long_distances.distances.extend([0, 0]);
    let malformed = [
        ("colour payload a byte short", short_tiles),
        ("distance payload a texel long", long_distances),
        (
            "unknown colour format",
            ReflectionProbesSection {
                atlas_format: 0xBAD,
                ..good.clone()
            },
        ),
        (
            "unknown distance format",
            ReflectionProbesSection {
                distance_format: 0xBAD,
                ..good.clone()
            },
        ),
        (
            "BC6H tile of partial 4×4 blocks",
            ReflectionProbesSection {
                tile_dimension: 6,
                tiles: vec![0; 16],
                distances:
                    postretro_level_format::reflection_probes::reflection_probe_zero_distances(1, 6),
                ..good
            },
        ),
    ];
    for (what, section) in &malformed {
        assert!(usable_probe_section(Some(section)).is_none(), "{what}");
        assert!(!volume_has_bound_face(Some(section), 0), "{what}");
        assert!(
            pack_probe_table(Some(section))
                .iter()
                .all(|&byte| byte == 0),
            "{what}"
        );
        let mut indices = vec![7; mesh.vertices.len()];
        fill_probe_indices(&volumes, &mesh.face_drift_maps, Some(section), &mut indices);
        assert!(
            indices
                .iter()
                .all(|&index| index == FLUID_REFLECTION_NO_PROBE),
            "{what}"
        );
    }
}

/// `base`'s probes and bindings over tiles of `dimension` in `atlas_format`,
/// with payloads that match.
fn retiled(
    base: &ReflectionProbesSection,
    atlas_format: u32,
    dimension: u32,
) -> ReflectionProbesSection {
    let probes = base.probes.len();
    let tile = postretro_level_format::reflection_probes::reflection_probe_tile_bytes(
        atlas_format,
        dimension,
    )
    .expect("a known colour format");
    ReflectionProbesSection {
        tile_dimension: dimension,
        atlas_format,
        tiles: vec![0; tile * probes],
        distances: postretro_level_format::reflection_probes::reflection_probe_zero_distances(
            probes, dimension,
        ),
        ..base.clone()
    }
}

// A hand-built section whose tile header the decoder would refuse (a
// dimension under one 4×4 block or past level-format's bound, a border off
// the octahedral convention), or whose probes do not each own a layer below
// the probe count, is unusable whole, as its payloads match: no table, no
// probe index and no bound face. Any one-to-one assignment of layers is
// usable.
#[test]
fn a_section_with_a_bad_tile_header_or_probe_layer_is_unusable() {
    let volumes = vec![fluid(vec![
        level_top(0.0, 0.0, 1.0),
        level_top(1.0, 0.0, 1.0),
    ])];
    let mesh = pack_fluid_surfaces(&volumes, &[], 1, None);
    let two = section(
        vec![
            room_probe([0.0, 0.002, 0.0], 0),
            room_probe([2.0, 0.002, 0.0], 1),
        ],
        vec![binding(0, 0, 0), binding(0, 1, 1)],
    );
    let mut swapped = two.clone();
    swapped.probes[0].layer = 1;
    swapped.probes[1].layer = 0;
    for (what, section) in [
        ("layers in order", two.clone()),
        ("layers swapped", swapped),
        (
            "RGBA16F at the least dimension",
            retiled(
                &two,
                IRRADIANCE_FORMAT_RGBA16F,
                FLUID_PROBE_MIN_TILE_DIMENSION,
            ),
        ),
        (
            "BC6H at the greatest dimension",
            retiled(&two, IRRADIANCE_FORMAT_BC6H, FLUID_PROBE_MAX_TILE_DIMENSION),
        ),
    ] {
        assert!(usable_probe_section(Some(&section)).is_some(), "{what}");
        assert!(volume_has_bound_face(Some(&section), 0), "{what}");
    }

    let mut layer_past = two.clone();
    layer_past.probes[1].layer = 2;
    let mut layer_reused = two.clone();
    layer_reused.probes[1].layer = 0;
    let malformed = [
        (
            "tile dimension 0",
            retiled(&two, IRRADIANCE_FORMAT_RGBA16F, 0),
        ),
        (
            "tile dimension 2, no interior",
            retiled(&two, IRRADIANCE_FORMAT_RGBA16F, 2),
        ),
        (
            "tile dimension past the decoder's bound",
            retiled(
                &two,
                IRRADIANCE_FORMAT_BC6H,
                FLUID_PROBE_MAX_TILE_DIMENSION + 4,
            ),
        ),
        (
            "tile border 0",
            ReflectionProbesSection {
                tile_border: 0,
                ..two.clone()
            },
        ),
        (
            "tile border 2",
            ReflectionProbesSection {
                tile_border: 2,
                ..two.clone()
            },
        ),
        ("a layer past the probe count", layer_past),
        ("a layer reused", layer_reused),
    ];
    for (what, section) in &malformed {
        assert!(usable_probe_section(Some(section)).is_none(), "{what}");
        assert!(!volume_has_bound_face(Some(section), 0), "{what}");
        assert!(
            pack_probe_table(Some(section))
                .iter()
                .all(|&byte| byte == 0),
            "{what}"
        );
        let mut indices = vec![7; mesh.vertices.len()];
        fill_probe_indices(&volumes, &mesh.face_drift_maps, Some(section), &mut indices);
        assert!(
            indices
                .iter()
                .all(|&index| index == FLUID_REFLECTION_NO_PROBE),
            "{what}"
        );
    }
}

// The usable tile dimensions are the section 54 decoder's own, in either
// colour format: it decodes a section at the least and the greatest
// dimension and refuses the next one past each, so this copy of
// level-format's private bound cannot drift from it; and it takes whole 4×4
// blocks only, RGBA16F included, so a dimension between them that is not a
// multiple of 4 is refused by both. Every dimension around the bounds and a
// stretch between agrees.
#[test]
fn the_usable_tile_dimensions_are_the_decoders() {
    let one = section(
        vec![room_probe([0.0, 0.002, 0.0], 0)],
        vec![binding(0, 0, 0)],
    );
    for format in [IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F] {
        let decodes = |dimension: u32| {
            let tiled = retiled(&one, format, dimension);
            ReflectionProbesSection::from_bytes(&tiled.to_bytes()).is_ok()
        };
        let usable = |dimension: u32| {
            usable_probe_section(Some(&retiled(&one, format, dimension))).is_some()
        };
        assert!(decodes(FLUID_PROBE_MIN_TILE_DIMENSION), "{format}");
        assert!(decodes(FLUID_PROBE_MAX_TILE_DIMENSION), "{format}");
        assert!(!decodes(FLUID_PROBE_MIN_TILE_DIMENSION - 4), "{format}");
        assert!(!decodes(FLUID_PROBE_MAX_TILE_DIMENSION + 4), "{format}");
        let dimensions = (1..=40)
            .chain(120..=136)
            .chain(FLUID_PROBE_MAX_TILE_DIMENSION - 8..=FLUID_PROBE_MAX_TILE_DIMENSION + 8);
        for dimension in dimensions {
            assert_eq!(
                usable(dimension),
                decodes(dimension),
                "format {format}, dimension {dimension}"
            );
        }
        for odd in [5, 6, 7, 1021] {
            assert!(!usable(odd), "format {format}, dimension {odd}");
        }
    }
}

// --- Acceptance row 17: box projection --------------------------------------

// A low eye at the pool's near end: the near edge's reflected ray climbs the
// far wall to (0, 3.5, 4) and the far edge's grazes its foot at (0, 1/14, 4),
// the walls a mirror would show there. The hit is the box's and never moves
// with the capture point; the lookup is hit minus capture point.
#[test]
fn box_projection_resolves_near_and_far_edges_to_the_right_wall_points() {
    let probe = room_probe([0.0, 0.002, 0.0], 0);
    let (box_min, box_max) = (Vec3::from(probe.box_min), Vec3::from(probe.box_max));
    let camera = Vec3::new(0.0, 0.5, -4.0);
    for (fragment, wall) in [
        (Vec3::new(0.0, 0.0, -3.0), Vec3::new(0.0, 3.5, 4.0)),
        (Vec3::new(0.0, 0.0, 3.0), Vec3::new(0.0, 0.5 / 7.0, 4.0)),
    ] {
        let view = (camera - fragment).normalize();
        let ray = reflection_mirror(view, Vec3::Y);
        assert_close(ray.length(), 1.0, 1.0e-6, "the mirror keeps unit length");
        assert_close(ray.y, view.y, 1.0e-6, "the mirror keeps height");
        let hit = reflection_box_hit(fragment, ray, box_min, box_max);
        assert_vec_close(hit, wall, 1.0e-5, "wall point");

        for capture in [Vec3::new(0.0, 0.002, 0.0), Vec3::new(3.0, 0.002, -2.0)] {
            let lookup = reflection_lookup(hit, capture, ray);
            assert_vec_close(lookup, hit - capture, 1.0e-6, "lookup = hit − capture");
        }
    }

    // A side wall and the ceiling resolve the same way.
    let fragment = Vec3::new(2.0, 0.0, 0.0);
    let ray = Vec3::new(1.0, 1.0, 0.0).normalize();
    assert_vec_close(
        reflection_box_hit(fragment, ray, box_min, box_max),
        Vec3::new(5.0, 3.0, 0.0),
        1.0e-5,
        "+X wall",
    );
    assert_vec_close(
        reflection_box_hit(fragment, Vec3::Y, box_min, box_max),
        Vec3::new(2.0, 4.0, 0.0),
        1.0e-5,
        "ceiling, with two components exactly zero",
    );
}

// The whole fragment reference reads the tile at the direction from the
// capture point to the box hit: moving the capture point moves the lookup by
// exactly that offset and nothing else.
#[test]
fn the_fragment_reference_samples_toward_the_box_hit_from_the_capture_point() {
    let camera = Vec3::new(-1.0, 1.7, -3.5);
    let world_position = Vec3::new(0.5, 0.0, 1.0);
    let view = (camera - world_position).normalize();
    let ray = reflection_mirror(view, Vec3::Y);
    let probe = room_probe([0.0, 0.002, 0.0], 0);
    let hit = reflection_box_hit(
        world_position,
        ray,
        Vec3::from(probe.box_min),
        Vec3::from(probe.box_max),
    );
    for capture in [[0.0, 0.002, 0.0], [-2.0, 0.002, 1.5]] {
        let probes = [room_probe(capture, 0)];
        let seen = Cell::new(None);
        reflection_fragment(
            Vec4::new(0.2, 0.3, 0.4, 0.6),
            &pool_fragment(camera, world_position),
            &probes,
            TILE,
            NO_LIGHTS,
            |layer, uv| {
                seen.set(Some((layer, uv)));
                Vec3::ONE
            },
            no_surface,
        );
        let (layer, uv) = seen.get().expect("a bound face seen from above samples");
        assert_eq!(layer, 0);
        let expected = reflection_tile_uv(hit - Vec3::from(capture), TILE[0], TILE[1]);
        assert!(uv.abs_diff_eq(expected, 1.0e-6), "{uv} vs {expected}");
    }
}

// A hit below the capture point (the pit's sides, under the 2 mm lift) reads
// the horizon, never the lower hemisphere.
#[test]
fn a_box_hit_below_the_capture_point_clamps_to_the_horizon() {
    let capture = Vec3::new(0.0, 0.002, 0.0);
    let hit = Vec3::new(5.0, -0.5, 1.0);
    let ray = Vec3::new(0.9, 0.1, 0.2);
    let lookup = reflection_lookup(hit, capture, ray);
    assert_eq!(lookup, Vec3::new(5.0, 0.0, 1.0));
    // A hit at the capture point itself takes the reflected ray's direction.
    assert_eq!(reflection_lookup(capture, capture, ray), ray);
    let down = Vec3::new(0.5, -0.5, 0.0);
    assert_eq!(
        reflection_lookup(capture, capture, down),
        Vec3::new(0.5, 0.0, 0.0)
    );
}

// The normalized tile UV is the shared octahedral texel coordinate over the
// tile dimension, so the probe reads the same direction mapping and border
// convention the capture wrote.
#[test]
fn tile_uv_is_the_shared_octahedral_texel_coordinate_over_the_tile() {
    for direction in [
        Vec3::Y,
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.3, 0.8, -0.5),
        Vec3::new(-0.7, 0.1, 0.7),
        Vec3::new(0.0, 0.2, -1.0),
    ] {
        for (dimension, border) in [(128, 1), (6, 1)] {
            let texel = irradiance_tile_texel_coord(direction.to_array(), dimension, border);
            let expected = Vec2::from(texel) / dimension as f32;
            let uv = reflection_tile_uv(direction, dimension, border);
            assert!(uv.abs_diff_eq(expected, 1.0e-6), "{direction}: {uv}");
            let interior = [border as f32, (dimension - border) as f32];
            for axis in [uv.x, uv.y] {
                let texel = axis * dimension as f32;
                assert!(texel >= interior[0] - 1.0e-4 && texel <= interior[1] + 1.0e-4);
            }
        }
    }
}

// --- Acceptance row 21: Fresnel ---------------------------------------------

#[test]
fn fresnel_scales_schlicks_base_reflectance_by_the_reflectivity() {
    let cos = |degrees: f32| degrees.to_radians().cos();
    // Water.
    assert_close(
        reflection_fresnel(1.0, 1.0),
        0.02,
        1.0e-6,
        "water at normal",
    );
    assert!(reflection_fresnel(cos(84.0), 1.0) >= 0.5);
    // Wet polished stone: F0 scaled, not F, so the grazing band keeps
    // Schlick's shape instead of clipping to a mirror.
    assert_close(reflection_fresnel(1.0, 10.0), 0.2, 1.0e-6, "10 at normal");
    assert_close(
        reflection_fresnel(cos(60.0), 10.0),
        0.225,
        0.005,
        "10 at 60°",
    );
    assert!(reflection_fresnel(cos(84.0), 10.0) < 0.7);
    // A mirror at every angle, never above 1; past 50 it stays a mirror.
    for degrees in (0..=90).map(|d| d as f32) {
        for reflectivity in [50.0, 60.0] {
            let f = reflection_fresnel(cos(degrees), reflectivity);
            assert!(f <= 1.0, "{reflectivity} at {degrees}°: {f}");
            assert_close(f, 1.0, 1.0e-6, "mirror");
        }
        // Monotone toward grazing.
        let f = reflection_fresnel(cos(degrees), 1.0);
        assert!(f <= reflection_fresnel(cos((degrees + 1.0).min(90.0)), 1.0) + 1.0e-7);
    }
}

// Reflectivity 0 (or a fluid with no bound face, which packs 0) gives today's
// picture exactly; reflectivity 1 changes it.
#[test]
fn reflectivity_zero_returns_the_surface_unchanged() {
    let camera = Vec3::new(0.0, 1.7, -3.5);
    let world_position = Vec3::new(0.0, 0.0, 1.0);
    let probes = [room_probe([0.0, 0.002, 0.0], 0)];
    let surface = Vec4::new(0.2, 0.3, 0.4, 0.6);
    let red = |_: u32, _: Vec2| Vec3::new(4.0, 0.0, 0.0);
    let off = FluidReflectionFragment {
        reflectivity: 0.0,
        ..pool_fragment(camera, world_position)
    };
    assert_eq!(
        reflection_fragment(surface, &off, &probes, TILE, NO_LIGHTS, red, no_surface),
        surface
    );
    let on = reflection_fragment(
        surface,
        &pool_fragment(camera, world_position),
        &probes,
        TILE,
        NO_LIGHTS,
        red,
        no_surface,
    );
    assert_ne!(on, surface);
}

// --- Acceptance row 22: underside -------------------------------------------

#[test]
fn a_face_seen_from_below_reflects_nothing_and_from_above_reflects() {
    let world_position = Vec3::new(0.0, 0.0, 1.0);
    let probes = [room_probe([0.0, 0.002, 0.0], 0)];
    let surface = Vec4::new(0.2, 0.3, 0.4, 0.6);
    let red = |_: u32, _: Vec2| Vec3::new(4.0, 0.0, 0.0);
    let below = Vec3::new(0.0, -1.2, -2.0);
    let above = Vec3::new(0.0, 1.2, -2.0);
    assert!(!reflection_seen_from_above(
        Vec3::Y,
        (below - world_position).normalize()
    ));
    assert_eq!(
        reflection_fragment(
            surface,
            &pool_fragment(below, world_position),
            &probes,
            TILE,
            NO_LIGHTS,
            red,
            no_surface
        ),
        surface,
        "from below: today's value"
    );
    let reflected = reflection_fragment(
        surface,
        &pool_fragment(above, world_position),
        &probes,
        TILE,
        NO_LIGHTS,
        red,
        no_surface,
    );
    assert!(
        reflected.x > surface.x && reflected.w > surface.w,
        "{reflected}"
    );
    // The gate reads the outward normal, so a face wound the other way (its
    // outward normal down) seen from above is its own underside.
    let flipped = FluidReflectionFragment {
        outward: -Vec3::Y,
        ..pool_fragment(above, world_position)
    };
    assert_eq!(
        reflection_fragment(surface, &flipped, &probes, TILE, NO_LIGHTS, red, no_surface),
        surface
    );
    // A face bound to no probe renders as today.
    let unbound = FluidReflectionFragment {
        probe: FLUID_REFLECTION_NO_PROBE,
        ..pool_fragment(above, world_position)
    };
    assert_eq!(
        reflection_fragment(surface, &unbound, &probes, TILE, NO_LIGHTS, red, no_surface),
        surface
    );
}

// From above, an eye inside the fog's band of the plane scales the term by
// its height through the band (the fog's own proximity): 0 at the plane,
// full one band up and higher. From below the gate already gives 0. Over a
// zero-alpha surface the output coverage is the term's weight alone.
#[test]
fn an_eye_inside_the_fog_band_scales_the_reflection_by_its_height() {
    use crate::fluid_surface::fog::FLUID_FOG_BAND;
    let world_position = Vec3::new(0.0, 0.0, 1.0);
    let probes = [room_probe([0.0, 0.002, 0.0], 0)];
    let clear = Vec4::new(0.2, 0.3, 0.4, 0.0);
    let red = |_: u32, _: Vec2| Vec3::new(4.0, 0.0, 0.0);
    let eye = |height: f32| Vec3::new(0.0, height, -2.0);
    let coverage = |height: f32| {
        let fragment = pool_fragment(eye(height), world_position);
        reflection_fragment(clear, &fragment, &probes, TILE, NO_LIGHTS, red, no_surface).w
    };
    for bands in [0.0, 0.25, 0.5, 0.75, 1.0, 2.0, 25.0] {
        let height = bands * FLUID_FOG_BAND;
        let view = (eye(height) - world_position).normalize();
        let expected = reflection_fresnel(view.dot(Vec3::Y), 1.0) * bands.min(1.0);
        assert_close(
            coverage(height),
            expected,
            1.0e-6,
            &format!("eye {bands} bands up"),
        );
    }
    assert_eq!(coverage(0.0), 0.0, "at the plane");
    assert_eq!(coverage(-0.5 * FLUID_FOG_BAND), 0.0, "from below");
    assert!(
        coverage(0.5 * FLUID_FOG_BAND) > 0.4,
        "half the band up reflects half the grazing term"
    );
}

// --- Acceptance row 23: composite -------------------------------------------

/// Straight-alpha `src` over `dst`, as `fluid_blend_state` blends colour.
fn over(src: Vec4, dst: Vec3) -> Vec3 {
    src.truncate() * src.w + dst * (1.0 - src.w)
}

#[test]
fn the_composite_is_reflection_over_surface_over_fog_over_scene() {
    let scene = Vec3::new(0.9, 0.8, 0.7);
    let reflection = Vec3::new(2.0, 1.5, 0.25);
    let rgb = Vec3::new(0.1, 0.4, 0.3);
    let camera = Vec3::new(0.0, 1.0, -1.0);
    let world_position = Vec3::new(0.0, 0.0, 0.5);
    let view_proj = glam::camera::rh::proj::directx::perspective(1.2, 1.0, 0.1, 100.0)
        * glam::camera::rh::view::look_at_mat4(camera, world_position, Vec3::Y);
    let fog = FluidFogFragment {
        view_proj,
        camera,
        world_position,
        outward: Vec3::Y,
        scene_depth: 0.999,
        extinction: 3.0,
        fog_albedo: Vec3::new(0.04, 0.055, 0.048),
        lighting: Vec3::splat(0.8),
        weight: 1.0,
    };
    let probes = [room_probe([0.0, 0.002, 0.0], 0)];
    let frag = pool_fragment(camera, world_position);
    let view = (camera - world_position).normalize();
    let f = reflection_fresnel(view.dot(Vec3::Y), 1.0);

    for alpha in [0.0, 0.3, 0.6, 1.0] {
        // With fog: surface over fog over scene, then the reflection over it.
        let fogged = fog_fragment(rgb, alpha, &fog);
        assert!(fogged.w >= alpha && fogged.w > 0.0, "fog adds coverage");
        let out = reflection_fragment(
            fogged,
            &frag,
            &probes,
            TILE,
            NO_LIGHTS,
            |_, _| reflection,
            no_surface,
        );
        let expected = reflection * f + over(fogged, scene) * (1.0 - f);
        assert_vec_close(over(out, scene), expected, 1.0e-5, "fogged composite");
        assert_close(out.w, f + (1.0 - f) * fogged.w, 1.0e-6, "fogged coverage");

        // Fog omitted: surface over scene.
        let clear = FluidFogFragment {
            extinction: 0.0,
            ..fog
        };
        let surface = fog_fragment(rgb, alpha, &clear);
        assert_eq!(surface, rgb.extend(alpha));
        let out = reflection_fragment(
            surface,
            &frag,
            &probes,
            TILE,
            NO_LIGHTS,
            |_, _| reflection,
            no_surface,
        );
        let expected = reflection * f + (rgb * alpha + scene * (1.0 - alpha)) * (1.0 - f);
        assert_vec_close(over(out, scene), expected, 1.0e-5, "clear composite");
        assert_close(out.w, f + (1.0 - f) * alpha, 1.0e-6, "clear coverage");
    }

    // Opacity never dims the reflection: at zero opacity the face draws the
    // reflection alone at coverage F.
    let alone = reflection_composite(Vec4::new(0.3, 0.3, 0.3, 0.0), reflection, f);
    assert_close(alone.w, f, 1.0e-7, "coverage F");
    assert_vec_close(alone.truncate(), reflection, 1.0e-5, "the capture's colour");
    // Coverage 0 (no Fresnel, no surface) stays finite.
    let empty = reflection_composite(Vec4::new(0.3, 0.3, 0.3, 0.0), reflection, 0.0);
    assert!(empty.is_finite() && empty.w == 0.0, "{empty}");
}

// --- Acceptance row 25: lighting mask ---------------------------------------

// The probe term draws only with every lighting bit of `LightTermMask::ALL`
// on (the literal reading: specular and depth AO included); with any one off
// the fragment is exactly today's. The reserved emissive bit plays no part.
#[test]
fn the_probe_term_draws_only_under_the_all_terms_mask() {
    assert_eq!(FLUID_REFLECTION_ALL_TERMS, 0x17F);
    let camera = Vec3::new(0.0, 1.7, -3.5);
    let world_position = Vec3::new(0.0, 0.0, 1.0);
    let probes = [room_probe([0.0, 0.002, 0.0], 0)];
    let surface = Vec4::new(0.2, 0.3, 0.4, 0.6);
    let red = |_: u32, _: Vec2| Vec3::new(4.0, 0.0, 0.0);
    let with_mask = |mask: u32| FluidReflectionFragment {
        light_term_mask: mask,
        ..pool_fragment(camera, world_position)
    };
    let all = LightTermMask::ALL.bits();
    for mask in [all, all | 0x80] {
        assert!(reflection_mask_on(mask));
        assert_ne!(
            reflection_fragment(
                surface,
                &with_mask(mask),
                &probes,
                TILE,
                NO_LIGHTS,
                red,
                no_surface
            ),
            surface
        );
    }
    for term in LightTermMask::ALL_TERMS {
        let mask = all & !term.bits();
        assert!(!reflection_mask_on(mask), "{}", term.label());
        assert_eq!(
            reflection_fragment(
                surface,
                &with_mask(mask),
                &probes,
                TILE,
                NO_LIGHTS,
                red,
                no_surface
            ),
            surface,
            "{} off",
            term.label()
        );
    }
    assert!(!reflection_mask_on(0));
}

// --- Acceptance rows 18–20: live lighting by light share --------------------

/// A flat (curveless) descriptor: its live value is `base_color` while
/// active, 0 while not.
fn flat_light(base_color: [f32; 3], is_active: bool) -> AnimatedLightScaleDescriptor<'static> {
    AnimatedLightScaleDescriptor {
        period: 1.0,
        phase: 0.0,
        base_color,
        brightness: &[],
        color: &[],
        is_active,
    }
}

/// A flickering descriptor: brightness swings 0 → 2 → 0.5 → 1.5 over 0.5 s.
fn flicker(base_color: [f32; 3], phase: f32) -> AnimatedLightScaleDescriptor<'static> {
    AnimatedLightScaleDescriptor {
        period: 0.5,
        phase,
        base_color,
        brightness: &[0.0, 2.0, 0.5, 1.5],
        color: &[],
        is_active: true,
    }
}

/// One packed share entry, as the probe table carries it.
fn entry(
    roster_index: u32,
    reference: [f32; 3],
    shares: [[f32; 3]; REFLECTION_PROBE_LOBE_COUNT],
) -> FluidProbeLightGpu {
    FluidProbeLightGpu::pack(&ReflectionProbeLightShare {
        roster_index,
        reference,
        shares,
    })
}

fn unused_entries() -> [FluidProbeLightGpu; REFLECTION_PROBE_LIGHTS_PER_PROBE] {
    [FluidProbeLightGpu::pack(&ReflectionProbeLightShare::UNUSED);
        REFLECTION_PROBE_LIGHTS_PER_PROBE]
}

/// Directions spread over the whole sphere (a Fibonacci lattice), the axes
/// and the straight-down limit included.
fn sphere_directions() -> Vec<Vec3> {
    let count = 400;
    let golden = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    let mut directions: Vec<Vec3> = (0..count)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f32 + 0.5) / count as f32;
            let radius = (1.0 - y * y).sqrt();
            let theta = golden * i as f32;
            Vec3::new(radius * theta.cos(), y, radius * theta.sin())
        })
        .collect();
    directions.extend([Vec3::Y, Vec3::X, -Vec3::X, Vec3::Z, -Vec3::Z, -Vec3::Y]);
    directions
}

fn lights<'a>(
    descriptors: &'a [AnimatedLightScaleDescriptor<'a>],
    time: f32,
) -> FluidReflectionLights<'a> {
    FluidReflectionLights { descriptors, time }
}

// Pin P3: with no animated lights, or with every stored light's live value at
// the reference its entry stores, the ratio is 1 in every direction and the
// capture shows unscaled.
#[test]
fn ratio_is_one_with_no_lights_or_every_light_at_its_reference() {
    for direction in sphere_directions() {
        assert_eq!(
            reflection_ratio(&unused_entries(), direction, NO_LIGHTS),
            Vec3::ONE,
            "no lights, {direction}"
        );
    }

    // Four flat lights at their references, zero channels included.
    let references = [
        [0.5, 1.0, 2.0],
        [3.0, 0.0, 0.0],
        [0.25, 0.25, 0.25],
        [0.0, 4.0, 1.0],
    ];
    let shares = [
        [0.3, 0.2, 0.1],
        [0.1; 3],
        [0.0; 3],
        [0.25; 3],
        [0.5, 0.0, 0.2],
    ];
    let descriptors: Vec<_> = references.iter().map(|&r| flat_light(r, true)).collect();
    let flat = lights(&descriptors, 3.25);
    let stored: [FluidProbeLightGpu; REFLECTION_PROBE_LIGHTS_PER_PROBE] =
        std::array::from_fn(|i| entry(i as u32, references[i], shares));
    for direction in sphere_directions() {
        assert_eq!(
            reflection_ratio(&stored, direction, &flat),
            Vec3::ONE,
            "flat lights at reference, {direction}"
        );
    }

    // A flickering light whose live value this frame equals its stored
    // reference also reads 1.
    let flickering = [flicker([1.0, 0.5, 0.25], 0.1)];
    let now = lights(&flickering, 0.7);
    let live = reflection_light_value(&now, 0);
    let mut stored = unused_entries();
    stored[0] = entry(0, live.to_array(), [[0.6; 3]; REFLECTION_PROBE_LOBE_COUNT]);
    for direction in sphere_directions() {
        assert_vec_close(
            reflection_ratio(&stored, direction, &now),
            Vec3::ONE,
            1.0e-6,
            "flicker at its reference",
        );
    }
}

// Pin P6: the compiler stores K = 4 lights per probe; a fifth light lighting
// the probe has no entry, so however it flickers or switches the ratio does
// not move: it holds its reference. Stored, the same flicker would move it.
#[test]
fn a_light_past_k_holds_its_reference() {
    let references = [[1.0; 3], [2.0, 1.0, 0.5], [0.5; 3], [1.0, 0.0, 1.0]];
    let shares = [[0.2; 3]; REFLECTION_PROBE_LOBE_COUNT];
    let stored: [FluidProbeLightGpu; REFLECTION_PROBE_LIGHTS_PER_PROBE] =
        std::array::from_fn(|i| entry(i as u32, references[i], shares));
    for (time, fifth) in [
        (0.0, flicker([5.0, 5.0, 5.0], 0.0)),
        (0.13, flicker([5.0, 5.0, 5.0], 0.0)),
        (0.37, flicker([0.1, 2.0, 0.0], 0.4)),
        (1.0, flat_light([9.0; 3], false)),
    ] {
        let mut descriptors: Vec<_> = references.iter().map(|&r| flat_light(r, true)).collect();
        descriptors.push(fifth);
        let frame = lights(&descriptors, time);
        for direction in sphere_directions() {
            assert_eq!(
                reflection_ratio(&stored, direction, &frame),
                Vec3::ONE,
                "the fifth light at t = {time} moved the ratio toward {direction}"
            );
        }
        // Had the compiler stored it in the fourth entry, it would show.
        let mut covered = stored;
        covered[3] = entry(4, [1.0; 3], shares);
        if (reflection_light_value(&frame, 4) - Vec3::ONE)
            .abs()
            .max_element()
            > 1.0e-3
        {
            assert_ne!(reflection_ratio(&covered, Vec3::Y, &frame), Vec3::ONE);
        }
    }
}

/// Step between adjacent lookup directions in the P7 sweep.
const P7_STEP_RADIANS: f32 = 1.0e-3;
/// Largest change of the ratio between two lookup directions one step apart,
/// for one light at a change of at most 1 in any channel. The lobe weights
/// are C¹ and move by at most one unit of weight per radian in total, so the
/// blend moves by at most the step; a hard lobe assignment would jump by the
/// full share difference (here up to 1) at an edge.
const P7_RATIO_STEP_EPSILON: f32 = 2.0e-3;

// Pin P7: sweep the lookup direction across every lobe edge — up against each
// horizontal lobe along meridians, and each horizontal lobe against its
// neighbours along parallels, below the horizon too — with one light away
// from its reference: adjacent samples differ by at most the pinned epsilon,
// while the ratio itself spans the lobes' different shares.
#[test]
fn ratio_is_continuous_across_every_lobe_edge() {
    // Per channel, adjacent lobes hold different shares: up, +X, −X, +Z, −Z.
    let shares = [
        [0.5, 0.0, 1.0],
        [1.0, 1.0, 0.0],
        [1.0, 1.0, 0.25],
        [0.0, 1.0, 0.75],
        [0.0, 1.0, 0.5],
    ];
    let mut stored = unused_entries();
    stored[0] = entry(0, [1.0, 2.0, 0.5], shares);
    // Twice the reference: a change of +1 in every channel.
    let descriptors = [flat_light([2.0, 4.0, 1.0], true)];
    let frame = lights(&descriptors, 0.0);
    let ratio = |direction: Vec3| reflection_ratio(&stored, direction, &frame);

    let steps = (std::f32::consts::TAU / P7_STEP_RADIANS) as usize + 1;
    let mut sweeps: Vec<Vec<Vec3>> = Vec::new();
    // Meridians: full great circles through up and down every 15°.
    for azimuth_step in 0..12 {
        let azimuth = (azimuth_step as f32 * 15.0).to_radians();
        let across = Vec3::new(azimuth.cos(), 0.0, azimuth.sin());
        sweeps.push(
            (0..steps)
                .map(|i| {
                    let a = i as f32 * P7_STEP_RADIANS;
                    across * a.cos() + Vec3::Y * a.sin()
                })
                .collect(),
        );
    }
    // Parallels: every horizontal lobe edge, at and around the horizon.
    for elevation in [-60.0f32, -30.0, -5.0, 0.0, 5.0, 30.0, 60.0, 85.0] {
        let (y, radius) = (elevation.to_radians().sin(), elevation.to_radians().cos());
        sweeps.push(
            (0..steps)
                .map(|i| {
                    let a = i as f32 * P7_STEP_RADIANS;
                    Vec3::new(radius * a.cos(), y, radius * a.sin())
                })
                .collect(),
        );
    }

    let (mut lowest, mut highest) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for sweep in &sweeps {
        for pair in sweep.windows(2) {
            let (a, b) = (ratio(pair[0]), ratio(pair[1]));
            let step = (a - b).abs().max_element();
            assert!(
                step <= P7_RATIO_STEP_EPSILON,
                "ratio stepped {step} between {} and {}",
                pair[0],
                pair[1]
            );
            lowest = lowest.min(a);
            highest = highest.max(a);
        }
    }
    // The sweep is not flat: each channel spans its lobes' shares.
    assert!(
        (highest - lowest).min_element() > 0.4,
        "{lowest} .. {highest}"
    );
}

// Pin P8: shares in a lobe summing past 1 with every light at 0 give ratio 0,
// never negative; a pure-red light's green and blue channels (reference 0)
// add no term and no NaN, whatever its live value there.
#[test]
fn ratio_clamps_at_zero_and_a_zero_reference_channel_adds_no_term() {
    let full = [[1.0; 3]; REFLECTION_PROBE_LOBE_COUNT];
    let stored: [FluidProbeLightGpu; REFLECTION_PROBE_LIGHTS_PER_PROBE] =
        std::array::from_fn(|i| entry(i as u32, [1.0, 0.5, 2.0], full));
    let off: Vec<_> = (0..REFLECTION_PROBE_LIGHTS_PER_PROBE)
        .map(|_| flat_light([1.0, 0.5, 2.0], false))
        .collect();
    let dark = lights(&off, 0.0);
    for direction in sphere_directions() {
        let ratio = reflection_ratio(&stored, direction, &dark);
        assert_eq!(
            ratio,
            Vec3::ZERO,
            "four full shares at 0 toward {direction}"
        );
    }

    let mut stored = unused_entries();
    stored[0] = entry(0, [1.0, 0.0, 0.0], full);
    for (live, expected) in [
        ([3.0, 0.0, 0.0], Vec3::new(3.0, 1.0, 1.0)),
        // Live green and blue the reference never held: still no term.
        ([3.0, 5.0, 0.25], Vec3::new(3.0, 1.0, 1.0)),
        ([0.0, 0.0, 0.0], Vec3::new(0.0, 1.0, 1.0)),
    ] {
        let descriptors = [flat_light(live, true)];
        let frame = lights(&descriptors, 0.0);
        for direction in sphere_directions() {
            let ratio = reflection_ratio(&stored, direction, &frame);
            assert!(ratio.is_finite(), "{ratio}");
            assert_vec_close(ratio, expected, 1.0e-6, "pure-red light");
        }
    }
    let change = reflection_light_change(Vec3::new(0.0, 2.0, 0.0), Vec3::ZERO);
    assert_eq!(change, Vec3::ZERO);
}

// Pin P28: a probe whose every share entry holds the reserved value reads 1
// in every direction while the level's lights flicker and switch; the
// reserved value indexes no descriptor, however many exist.
#[test]
fn a_probe_with_only_reserved_entries_reads_one_while_lights_flicker() {
    let descriptors: Vec<_> = (0..8)
        .map(|i| {
            if i % 3 == 0 {
                flat_light([1.0; 3], false)
            } else {
                flicker([i as f32, 1.0, 0.5], i as f32 * 0.1)
            }
        })
        .collect();
    for time in [0.0, 0.11, 0.29, 0.5, 2.71] {
        let frame = lights(&descriptors, time);
        assert!(!reflection_light_present(&frame, REFLECTION_PROBE_NO_LIGHT));
        for direction in sphere_directions() {
            assert_eq!(
                reflection_ratio(&unused_entries(), direction, &frame),
                Vec3::ONE
            );
        }
    }
    // An index past the bound descriptors indexes nothing either.
    let frame = lights(&descriptors, 0.0);
    assert!(reflection_light_present(&frame, 7));
    assert!(!reflection_light_present(&frame, 8));
}

/// Per-lobe shares that differ by lobe, so the ratio depends on direction.
const LOBED_SHARES: [[f32; 3]; REFLECTION_PROBE_LOBE_COUNT] = [
    [0.6, 0.5, 0.4],
    [0.1, 0.2, 0.3],
    [0.9, 0.0, 0.5],
    [0.3, 0.7, 0.2],
    [0.0, 0.4, 0.8],
];

// Two fragments of one probe whose reflected rays leave the box at the same
// wall point look the tile up in one direction and read one ratio, however
// far apart they sit and whatever their Fresnel; a fragment looking another
// way reads another ratio. At zero opacity the output colour is the scaled
// capture alone.
#[test]
fn two_fragments_of_one_probe_read_one_ratio_for_one_direction() {
    let mut probe = room_probe([0.0, 0.002, 0.0], 0);
    probe.lights[0] = ReflectionProbeLightShare {
        roster_index: 0,
        reference: [1.0; 3],
        shares: LOBED_SHARES,
    };
    let probes = [probe];
    // The light is switched off.
    let descriptors = [flat_light([1.0; 3], false)];
    let frame = lights(&descriptors, 0.0);
    let capture = Vec3::new(0.8, 0.6, 0.4);
    let surface = Vec4::new(0.2, 0.3, 0.4, 0.0);
    // The camera whose view mirrors about the plane y = 0 onto the ray from
    // `p` through `hit`.
    let camera_toward = |p: Vec3, hit: Vec3| Vec3::new(2.0 * p.x - hit.x, hit.y, 2.0 * p.z - hit.z);
    let out = |p: Vec3, hit: Vec3| {
        reflection_fragment(
            surface,
            &pool_fragment(camera_toward(p, hit), p),
            &probes,
            TILE,
            &frame,
            |_, _| capture,
            no_surface,
        )
    };
    let wall = Vec3::new(1.0, 2.0, 4.0);
    let near = out(Vec3::new(0.0, 0.0, 1.0), wall);
    let far = out(Vec3::new(-2.0, 0.0, -1.0), wall);
    let lookup = wall - Vec3::from(probes[0].capture_point);
    let ratio = reflection_ratio(&FluidProbeGpu::pack(&probes[0]).lights, lookup, &frame);
    assert!(ratio.cmplt(Vec3::ONE).all(), "the light is off: {ratio}");
    assert_vec_close(near.truncate(), capture * ratio, 1.0e-5, "near fragment");
    assert_vec_close(far.truncate(), capture * ratio, 1.0e-5, "far fragment");
    assert_ne!(near.w, far.w, "the two see different Fresnel");

    // Toward the ceiling instead, the up lobe's share sets another ratio.
    let ceiling = out(Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.5, 4.0, 2.0));
    assert!(
        (ceiling.truncate() - near.truncate()).abs().max_element() > 0.01,
        "{ceiling} vs {near}"
    );
}

// One light away from its reference moves the ratio by its share toward the
// lookup direction, measured against the reference its share entry stores,
// not the descriptor's own colour: at twice the reference by +share, off by
// −share, and at a live (1, 1, 1) against a stored (0.5, 1, 2) by
// share × (1, 0, −0.5). The share is the lobe blend of the stored shares,
// within unorm10 quantization.
#[test]
fn one_light_away_from_its_reference_moves_the_ratio_by_its_share() {
    let reference = Vec3::new(0.5, 1.0, 2.0);
    let mut stored = unused_entries();
    stored[0] = entry(0, reference.to_array(), LOBED_SHARES);
    let quantum = 0.5 / FLUID_REFLECTION_UNORM10_MAX as f32;
    for (live, is_active, change) in [
        (reference * 2.0, true, Vec3::ONE),
        (reference, false, -Vec3::ONE),
        (Vec3::ONE, true, Vec3::new(1.0, 0.0, -0.5)),
    ] {
        let descriptors = [flat_light(live.to_array(), is_active)];
        let frame = lights(&descriptors, 0.0);
        for direction in sphere_directions() {
            let lobes = reflection_probe_lobe_weights(direction.to_array());
            let share: Vec3 = LOBED_SHARES
                .iter()
                .zip(lobes)
                .map(|(lobe, weight)| Vec3::from(*lobe) * weight)
                .sum();
            let ratio = reflection_ratio(&stored, direction, &frame);
            let expected = (Vec3::ONE + share * change).max(Vec3::ZERO);
            assert_vec_close(
                ratio,
                expected,
                quantum * change.abs().max_element() + 1.0e-6,
                "ratio = 1 + share × change",
            );
            // The quantized blend is what the shader reads.
            let packed = reflection_light_share(&stored[0].shares, &lobes);
            assert_vec_close(ratio, Vec3::ONE + packed * change, 1.0e-6, "packed share");
        }
    }
}

// Pin P1: promotion crossfades a light between its baked and runtime paths by
// a weight w that Pass B (`animated_direct_sh_compose.wgsl`) multiplies into
// its own scale, and never writes the light's descriptor. The reflection's
// live value is the descriptor's own, read with no weight, so across the
// whole crossfade the ratio is identical while Pass B's weighted scale falls
// to 0.
#[test]
fn promotion_never_moves_the_ratio() {
    let descriptors = [flicker([1.5, 1.0, 0.5], 0.2)];
    let frame = lights(&descriptors, 0.42);
    let mut stored = unused_entries();
    stored[0] = entry(0, [1.0; 3], LOBED_SHARES);
    let direction = Vec3::new(0.4, 0.7, -0.3);
    let unweighted = Vec3::from(animated_light_scale(Some(descriptors[0]), frame.time));
    let before = reflection_ratio(&stored, direction, &frame);
    assert_ne!(
        before,
        Vec3::ONE,
        "the flicker sits away from its reference"
    );
    let mut weighted = Vec::new();
    for step in 0..=10 {
        let w = step as f32 / 10.0;
        weighted.push(unweighted * (1.0 - w));
        assert_eq!(reflection_light_value(&frame, 0), unweighted, "w = {w}");
        assert_eq!(
            reflection_ratio(&stored, direction, &frame),
            before,
            "w = {w}"
        );
    }
    assert_eq!(weighted.last(), Some(&Vec3::ZERO));
}
