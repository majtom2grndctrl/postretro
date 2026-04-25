// Per-animated-light delta SH volume baker.
//
// For each light in `AnimatedBakedLights`, places probes on a regular grid
// over that light's influence-sphere AABB and bakes the light's full
// (direct + indirect) contribution at peak brightness (brightness = 1.0,
// authored color × intensity) into SH L2 RGB coefficients. The runtime
// compose pass adds `delta[i] × brightness_curve(t)` to the base SH volume
// to recover the animated light's instantaneous contribution.
//
// Index-space contract: `grids[i]` and `header.animation_descriptor_indices[i]`
// match `AnimatedBakedLights.entries()[i]` — same iteration order, no remap.
//
// See: context/plans/in-progress/lighting-animated-sh/

use std::collections::HashSet;

use bvh::bvh::Bvh;
use glam::{DVec3, Vec3};
use postretro_level_format::delta_sh_volumes::{
    DeltaLightGrid, DeltaShProbe, DeltaShVolumeHeader, DeltaShVolumesSection, PROBE_F16_COUNT,
};
use rayon::prelude::*;

use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;
use crate::light_namespaces::AnimatedBakedLights;
use crate::map_data::{LightType, MapLight};
use crate::partition::BspTree;
use crate::sh_bake::{
    RaytracingCtx, bake_probe_direct_rgb, bake_probe_indirect_rgb, probe_is_valid_pub,
};

/// Default probe spacing for delta grids. Coarser than the base SH grid is
/// acceptable: the runtime compose pass trilinear-samples the delta grid into
/// the base grid's resolution.
pub const DEFAULT_DELTA_PROBE_SPACING: f32 = 1.0;

/// AABB padding past the light's falloff sphere, meters. The trilinear
/// reconstruction at the boundary needs probes slightly outside the sphere
/// or it darkens the rim of the light's influence.
const AABB_PADDING_METERS: f32 = 0.5;

/// Hard cap on directional-light AABB size, meters. Directional lights
/// nominally cover the whole world, but the delta volume only needs to
/// span the playable geometry — the world AABB substitutes for the
/// "infinite" influence sphere.
const DIRECTIONAL_FALLBACK_RANGE_METERS: f32 = 100.0;

/// Inputs for the delta SH bake. Mirrors `sh_bake::BakeInputs` (same BVH,
/// same geometry, same BSP tree) plus the animated-light envelope.
pub struct DeltaBakeInputs<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
    pub tree: &'a BspTree,
    pub exterior_leaves: &'a HashSet<usize>,
    pub animated_lights: &'a AnimatedBakedLights<'a>,
}

/// Bake one delta grid per animated light. Returns `None` when the envelope
/// is empty — the caller should omit the `DeltaShVolumes` section entirely
/// in that case (an empty section is wasted bytes).
pub fn bake_delta_sh_volumes(
    inputs: &DeltaBakeInputs<'_>,
    probe_spacing_meters: f32,
) -> Option<DeltaShVolumesSection> {
    if inputs.animated_lights.is_empty() {
        return None;
    }

    let world_aabb = world_aabb_for_directional(inputs);
    let entries = inputs.animated_lights.entries();
    let descriptor_indices: Vec<u32> = (0..entries.len() as u32).collect();

    // Each light is independent — bake them in parallel.
    let grids: Vec<DeltaLightGrid> = entries
        .par_iter()
        .map(|entry| bake_one_light_grid(inputs, entry.light, probe_spacing_meters, world_aabb))
        .collect();

    Some(DeltaShVolumesSection {
        header: DeltaShVolumeHeader {
            animation_descriptor_indices: descriptor_indices,
        },
        grids,
    })
}

/// Log bake statistics in the same shape as other compile stages.
pub fn log_stats(section: &DeltaShVolumesSection) {
    let total_probes: usize = section.grids.iter().map(|g| g.total_probes()).sum();
    log::info!(
        "DeltaShVolumes: {} animated light(s), {total_probes} total probes",
        section.grids.len(),
    );
    for (i, grid) in section.grids.iter().enumerate() {
        let dims = grid.grid_dimensions;
        log::info!(
            "  light {i}: grid {}x{}x{} = {} probes, cell {}m",
            dims[0],
            dims[1],
            dims[2],
            grid.total_probes(),
            grid.cell_size,
        );
    }
}

// ---------------------------------------------------------------------------
// Per-light bake

fn bake_one_light_grid(
    inputs: &DeltaBakeInputs<'_>,
    light: &MapLight,
    probe_spacing_meters: f32,
    world_aabb: (DVec3, DVec3),
) -> DeltaLightGrid {
    let (aabb_min, aabb_max) = light_aabb(light, world_aabb);
    let dims = grid_dimensions(aabb_min, aabb_max, probe_spacing_meters);
    let total = dims[0] as usize * dims[1] as usize * dims[2] as usize;

    let ctx = RaytracingCtx {
        bvh: inputs.bvh,
        primitives: inputs.primitives,
        geometry: inputs.geometry,
    };

    let probes: Vec<DeltaShProbe> = (0..total)
        .into_par_iter()
        .map(|i| {
            let pos_d = probe_position(i, dims, aabb_min, probe_spacing_meters);
            if !probe_is_valid_pub(inputs.tree, inputs.exterior_leaves, pos_d) {
                return DeltaShProbe::default();
            }
            let pos = Vec3::new(pos_d.x as f32, pos_d.y as f32, pos_d.z as f32);
            let lights_slice: [&MapLight; 1] = [light];
            // Indirect: rays from the probe bounce off geometry and pick up
            // the animated light's reflected radiance. Same path the base
            // SH bake uses, with a single-element light slice.
            let indirect = bake_probe_indirect_rgb(&ctx, pos, &lights_slice);
            // Direct: light delivered straight to the probe at peak.
            let direct = bake_probe_direct_rgb(&ctx, pos, light);
            let mut combined = [0f32; PROBE_F16_COUNT];
            for (out, (a, b)) in combined.iter_mut().zip(direct.iter().zip(indirect.iter())) {
                *out = a + b;
            }
            DeltaShProbe::from_f32(&combined)
        })
        .collect();

    DeltaLightGrid {
        aabb_origin: [aabb_min.x as f32, aabb_min.y as f32, aabb_min.z as f32],
        cell_size: probe_spacing_meters,
        grid_dimensions: dims,
        probes,
    }
}

// ---------------------------------------------------------------------------
// Grid layout

fn light_aabb(light: &MapLight, world_aabb: (DVec3, DVec3)) -> (DVec3, DVec3) {
    match light.light_type {
        LightType::Directional => world_aabb,
        LightType::Point | LightType::Spot => {
            let r = (light.falloff_range + AABB_PADDING_METERS).max(0.01) as f64;
            // Spot lights have a forward cone; using the full sphere AABB is
            // a small over-coverage but keeps the grid layout uniform with
            // point lights and avoids cone-bound edge cases. Cell counts at
            // 1m spacing remain small (~ 2r+1 cubed).
            let _ = light.cone_direction;
            let center = DVec3::new(light.origin.x, light.origin.y, light.origin.z);
            (center - DVec3::splat(r), center + DVec3::splat(r))
        }
    }
}

/// Substitute world AABB used as the directional-light influence volume. The
/// extracted geometry's vertex bounds are the natural choice — directional
/// lights matter only where surfaces exist. Falls back to a small box around
/// the origin when the geometry is empty (test fixtures, mostly).
fn world_aabb_for_directional(inputs: &DeltaBakeInputs<'_>) -> (DVec3, DVec3) {
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for v in &inputs.geometry.geometry.vertices {
        let p = DVec3::new(
            v.position[0] as f64,
            v.position[1] as f64,
            v.position[2] as f64,
        );
        min = min.min(p);
        max = max.max(p);
    }
    if !min.x.is_finite() {
        let r = DIRECTIONAL_FALLBACK_RANGE_METERS as f64;
        return (DVec3::splat(-r), DVec3::splat(r));
    }
    (min, max)
}

fn grid_dimensions(min: DVec3, max: DVec3, spacing: f32) -> [u32; 3] {
    let extents = (max - min).max(DVec3::splat(0.0));
    let spacing = spacing.max(1.0e-4) as f64;
    [
        ((extents.x / spacing).ceil() as u32 + 1).max(1),
        ((extents.y / spacing).ceil() as u32 + 1).max(1),
        ((extents.z / spacing).ceil() as u32 + 1).max(1),
    ]
}

fn probe_position(linear: usize, dims: [u32; 3], origin: DVec3, spacing: f32) -> DVec3 {
    let nx = dims[0] as usize;
    let ny = dims[1] as usize;
    let z = linear / (nx * ny);
    let rem = linear - z * nx * ny;
    let y = rem / nx;
    let x = rem - y * nx;
    DVec3::new(
        origin.x + x as f64 * spacing as f64,
        origin.y + y as f64 * spacing as f64,
        origin.z + z as f64 * spacing as f64,
    )
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh_build::build_bvh;
    use crate::geometry::FaceIndexRange;
    use crate::map_data::{FalloffModel, LightAnimation, LightType};
    use crate::partition::{Aabb as CompilerAabb, BspLeaf, BspTree};
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;

    fn tri_vertex(pos: [f32; 3]) -> Vertex {
        Vertex::new(
            pos,
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
        )
    }

    fn one_triangle_geometry(positions: [[f32; 3]; 3]) -> GeometryResult {
        GeometryResult {
            geometry: GeometrySection {
                vertices: positions.iter().map(|&p| tri_vertex(p)).collect(),
                indices: vec![0, 1, 2],
                faces: vec![FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                }],
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: vec![FaceIndexRange {
                index_offset: 0,
                index_count: 3,
            }],
        }
    }

    fn tree_all_empty() -> BspTree {
        BspTree {
            nodes: Vec::new(),
            leaves: vec![BspLeaf {
                face_indices: Vec::new(),
                bounds: CompilerAabb {
                    min: DVec3::splat(-1000.0),
                    max: DVec3::splat(1000.0),
                },
                is_solid: false,
            }],
        }
    }

    fn tree_all_solid() -> BspTree {
        BspTree {
            nodes: Vec::new(),
            leaves: vec![BspLeaf {
                face_indices: Vec::new(),
                bounds: CompilerAabb {
                    min: DVec3::splat(-1000.0),
                    max: DVec3::splat(1000.0),
                },
                is_solid: true,
            }],
        }
    }

    fn animated_point_light(origin: DVec3, range: f32) -> MapLight {
        MapLight {
            origin,
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: range,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: Some(LightAnimation {
                period: 1.0,
                phase: 0.0,
                brightness: Some(vec![0.0, 1.0, 0.0]),
                color: None,
                direction: None,
                start_active: true,
            }),
            cast_shadows: true,
            bake_only: false,
            is_dynamic: false,
            tag: None,
        }
    }

    #[test]
    fn empty_envelope_returns_none() {
        let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        let lights: Vec<MapLight> = Vec::new();
        let envelope = AnimatedBakedLights::from_lights(&lights);
        let inputs = DeltaBakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            animated_lights: &envelope,
        };
        assert!(bake_delta_sh_volumes(&inputs, 1.0).is_none());
    }

    #[test]
    fn single_point_light_produces_correctly_sized_grid() {
        let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 0.0, 4.0]]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        // Range 2.0, padding 0.5 → AABB half-extent 2.5m, full extent 5m.
        // ceil(5 / 1) + 1 = 6 cells per axis.
        let lights = vec![animated_point_light(DVec3::ZERO, 2.0)];
        let envelope = AnimatedBakedLights::from_lights(&lights);
        let inputs = DeltaBakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            animated_lights: &envelope,
        };
        let section = bake_delta_sh_volumes(&inputs, 1.0).expect("expected a section");
        assert_eq!(section.grids.len(), 1);
        assert_eq!(section.header.animation_descriptor_indices, vec![0]);
        let grid = &section.grids[0];
        assert_eq!(grid.cell_size, 1.0);
        assert_eq!(grid.grid_dimensions, [6, 6, 6]);
        assert_eq!(grid.probes.len(), 6 * 6 * 6);
        // AABB origin should be at center - half-extent = (0, 0, 0) - (2.5, 2.5, 2.5).
        assert!((grid.aabb_origin[0] - -2.5).abs() < 1e-4);
        assert!((grid.aabb_origin[1] - -2.5).abs() < 1e-4);
        assert!((grid.aabb_origin[2] - -2.5).abs() < 1e-4);
    }

    #[test]
    fn solid_leaf_map_yields_zero_probes_without_panic() {
        // Every probe falls into a solid leaf — validity gate rejects all of
        // them, leaving the grid full of default (zero-coefficient) probes.
        let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 0.0, 4.0]]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_solid();
        let exterior: HashSet<usize> = HashSet::new();
        let lights = vec![animated_point_light(DVec3::new(0.5, 0.5, 0.5), 2.0)];
        let envelope = AnimatedBakedLights::from_lights(&lights);
        let inputs = DeltaBakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            animated_lights: &envelope,
        };
        let section = bake_delta_sh_volumes(&inputs, 1.0).expect("expected a section");
        assert_eq!(section.grids.len(), 1);
        let grid = &section.grids[0];
        for probe in &grid.probes {
            assert_eq!(probe.sh_coefficients_f16, [0u16; PROBE_F16_COUNT]);
        }
    }

    #[test]
    fn descriptor_indices_match_envelope_order() {
        let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 0.0, 4.0]]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        // Mix animated, non-animated, and dynamic lights: only the two
        // animated-baked ones must appear in the delta section, in the same
        // order they appear in the source array.
        let lights = vec![
            animated_point_light(DVec3::new(-3.0, 0.0, 0.0), 2.0),
            MapLight {
                animation: None,
                ..animated_point_light(DVec3::new(0.0, 0.0, 0.0), 2.0)
            },
            animated_point_light(DVec3::new(3.0, 0.0, 0.0), 2.0),
            MapLight {
                is_dynamic: true,
                ..animated_point_light(DVec3::new(6.0, 0.0, 0.0), 2.0)
            },
        ];
        let envelope = AnimatedBakedLights::from_lights(&lights);
        assert_eq!(envelope.len(), 2);
        let inputs = DeltaBakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            animated_lights: &envelope,
        };
        let section = bake_delta_sh_volumes(&inputs, 1.0).expect("expected a section");
        assert_eq!(section.grids.len(), 2);
        assert_eq!(section.header.animation_descriptor_indices, vec![0, 1]);
    }

    #[test]
    fn round_trip_through_format_crate() {
        let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 0.0, 4.0]]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        let lights = vec![animated_point_light(DVec3::new(0.5, 0.5, 0.5), 1.5)];
        let envelope = AnimatedBakedLights::from_lights(&lights);
        let inputs = DeltaBakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            animated_lights: &envelope,
        };
        let section = bake_delta_sh_volumes(&inputs, 1.0).expect("expected a section");
        let bytes = section.to_bytes();
        let restored = DeltaShVolumesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }
}
