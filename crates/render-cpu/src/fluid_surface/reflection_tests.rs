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
/// reflectivity 1, seen from `camera` with every lighting term on.
fn pool_fragment(camera: Vec3, world_position: Vec3) -> FluidReflectionFragment {
    FluidReflectionFragment {
        probe: 0,
        camera,
        world_position,
        outward: Vec3::Y,
        light_term_mask: LightTermMask::ALL.bits(),
        reflectivity: 1.0,
    }
}

// --- Probe table and per-vertex index ---------------------------------------

// The table is laid out at the cap: header, then probes at a 240-byte stride
// (box min, box max, capture point with its layer, then K 48-byte entries of
// RGB reference, roster index and unorm10×3 shares). Unused entries pack as
// the reserved roster index with zero values, so task 6 adds only reads.
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
    let mesh = pack_fluid_surfaces(&volumes, &[], 1);
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
            |layer, uv| {
                seen.set(Some((layer, uv)));
                Vec3::ONE
            },
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
        reflection_fragment(surface, &off, &probes, TILE, red),
        surface
    );
    let on = reflection_fragment(
        surface,
        &pool_fragment(camera, world_position),
        &probes,
        TILE,
        red,
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
            red
        ),
        surface,
        "from below: today's value"
    );
    let reflected = reflection_fragment(
        surface,
        &pool_fragment(above, world_position),
        &probes,
        TILE,
        red,
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
        reflection_fragment(surface, &flipped, &probes, TILE, red),
        surface
    );
    // A face bound to no probe renders as today.
    let unbound = FluidReflectionFragment {
        probe: FLUID_REFLECTION_NO_PROBE,
        ..pool_fragment(above, world_position)
    };
    assert_eq!(
        reflection_fragment(surface, &unbound, &probes, TILE, red),
        surface
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
        let out = reflection_fragment(fogged, &frag, &probes, TILE, |_, _| reflection);
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
        let out = reflection_fragment(surface, &frag, &probes, TILE, |_, _| reflection);
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
            reflection_fragment(surface, &with_mask(mask), &probes, TILE, red),
            surface
        );
    }
    for term in LightTermMask::ALL_TERMS {
        let mask = all & !term.bits();
        assert!(!reflection_mask_on(mask), "{}", term.label());
        assert_eq!(
            reflection_fragment(surface, &with_mask(mask), &probes, TILE, red),
            surface,
            "{} off",
            term.label()
        );
    }
    assert!(!reflection_mask_on(0));
}
