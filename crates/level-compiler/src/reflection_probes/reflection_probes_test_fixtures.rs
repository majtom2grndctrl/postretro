// Synthetic scenes for the reflection-probe stage tests: box rooms and fluid faces.
// See: context/lib/testing_guide.md §4

use glam::Vec3;
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
    EnvironmentVolumesSection,
};
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

use crate::geometry::{FaceIndexRange, GeometryResult};

/// Texels per meter on every fixture face.
pub(crate) const FIXTURE_TEXELS_PER_METER: f32 = 32.0;

/// Face index, and texture index, of [`box_room`]'s floor and ceiling; the
/// walls follow as west, east, north, south (−X, +X, −Z, +Z).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoomFace {
    Floor = 0,
    Ceiling = 1,
}

/// An inward-facing box room: six quads wound counter-clockwise around their
/// inward normals, as geometry extraction winds static faces. Face `i` uses
/// texture `i`, named after its [`RoomFace`]; UVs run at
/// [`FIXTURE_TEXELS_PER_METER`] from the room's min corner.
pub(crate) fn box_room(min: [f32; 3], max: [f32; 3]) -> GeometryResult {
    let (lo, hi) = (Vec3::from(min), Vec3::from(max));
    let quads: [(Vec3, [Vec3; 4]); 6] = [
        (Vec3::Y, rect(lo, hi, 1, lo.y)),
        (Vec3::NEG_Y, rect(lo, hi, 1, hi.y)),
        (Vec3::X, rect(lo, hi, 0, lo.x)),
        (Vec3::NEG_X, rect(lo, hi, 0, hi.x)),
        (Vec3::Z, rect(lo, hi, 2, lo.z)),
        (Vec3::NEG_Z, rect(lo, hi, 2, hi.z)),
    ];
    let mut builder = GeometryBuilder::default();
    for (texture, (normal, corners)) in quads.into_iter().enumerate() {
        builder.push_face(normal, &corners, texture as u32, lo, 0, [[0.0; 2]; 4]);
    }
    builder.finish(
        ["floor", "ceiling", "west", "east", "north", "south"]
            .map(String::from)
            .to_vec(),
    )
}

/// Accumulates faces into a [`GeometryResult`].
#[derive(Default)]
pub(crate) struct GeometryBuilder {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    faces: Vec<FaceMeta>,
    ranges: Vec<FaceIndexRange>,
}

impl GeometryBuilder {
    /// One planar quad, re-wound if needed so it fronts `normal`. UVs project
    /// the two axes orthogonal to the normal's dominant axis from `uv_origin`.
    pub(crate) fn push_face(
        &mut self,
        normal: Vec3,
        corners: &[Vec3; 4],
        texture_index: u32,
        uv_origin: Vec3,
        lightmap_block: u16,
        lightmap_uvs: [[f32; 2]; 4],
    ) {
        let mut order = [0usize, 1, 2, 3];
        let winding = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
        if winding.dot(normal) < 0.0 {
            order.reverse();
        }
        let base = self.vertices.len() as u32;
        let (axis_u, axis_v) = uv_axes(normal);
        for &i in &order {
            let p = corners[i];
            let rel = (p - uv_origin) * FIXTURE_TEXELS_PER_METER;
            let tangent = Vec3::from(axis_unit(axis_u));
            self.vertices.push(Vertex::new(
                p.to_array(),
                [rel[axis_u], rel[axis_v]],
                normal.to_array(),
                tangent.to_array(),
                true,
                lightmap_uvs[i],
                lightmap_block,
            ));
        }
        let offset = self.indices.len() as u32;
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        self.faces.push(FaceMeta {
            leaf_index: 0,
            texture_index,
        });
        self.ranges.push(FaceIndexRange {
            index_offset: offset,
            index_count: 6,
        });
    }

    pub(crate) fn finish(self, texture_names: Vec<String>) -> GeometryResult {
        GeometryResult {
            geometry: GeometrySection {
                vertices: self.vertices,
                indices: self.indices,
                faces: self.faces,
            },
            texture_names: TextureNamesSection {
                names: texture_names,
            },
            face_index_ranges: self.ranges,
        }
    }
}

/// The rectangle of the box `[lo, hi]` lying on plane `axis = value`.
pub(crate) fn rect(lo: Vec3, hi: Vec3, axis: usize, value: f32) -> [Vec3; 4] {
    let (a, b) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let corner = |ua: f32, ub: f32| {
        let mut p = Vec3::ZERO;
        p[axis] = value;
        p[a] = ua;
        p[b] = ub;
        p
    };
    [
        corner(lo[a], lo[b]),
        corner(hi[a], lo[b]),
        corner(hi[a], hi[b]),
        corner(lo[a], hi[b]),
    ]
}

fn uv_axes(normal: Vec3) -> (usize, usize) {
    let n = normal.abs();
    if n.y >= n.x && n.y >= n.z {
        (0, 2)
    } else if n.x >= n.z {
        (2, 1)
    } else {
        (0, 1)
    }
}

fn axis_unit(axis: usize) -> [f32; 3] {
    let mut v = [0.0; 3];
    v[axis] = 1.0;
    v
}

/// A fluid volume record holding `faces`, each a polygon of engine-space
/// points, for entity `source_entity`.
pub(crate) fn fluid_volume(source_entity: u32, faces: &[Vec<[f32; 3]>]) -> EnvironmentVolumeRecord {
    EnvironmentVolumeRecord {
        source_entity,
        priority: 0,
        gravity: None,
        push: None,
        fluid: "water".to_string(),
        brushes: Vec::new(),
        surface_faces: faces
            .iter()
            .map(|points| EnvironmentSurfaceFace {
                texture_index: 0,
                vertices: points
                    .iter()
                    .map(|&position| EnvironmentSurfaceVertex {
                        position,
                        uv: [0.0, 0.0],
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// A horizontal rectangle at height `y`, wound counter-clockwise seen from
/// above, so its outward normal points up.
pub(crate) fn up_rect(x0: f32, z0: f32, x1: f32, z1: f32, y: f32) -> Vec<[f32; 3]> {
    vec![[x0, y, z0], [x0, y, z1], [x1, y, z1], [x1, y, z0]]
}

pub(crate) fn volumes(records: Vec<EnvironmentVolumeRecord>) -> EnvironmentVolumesSection {
    EnvironmentVolumesSection {
        level_gravity: -9.81,
        volumes: records,
    }
}

/// A dense valid-rank RGBA16F SH intermediate, the base bake's output before
/// stored-set packing: every probe's tile uniform at `value(x, y, z)`. A NaN
/// red channel marks the probe invalid (in solid).
pub(crate) fn dense_sh_volume(
    dimensions: [u32; 3],
    value: impl Fn(u32, u32, u32) -> [f32; 3],
) -> postretro_level_format::sh_volume::OctahedralShVolumeSection {
    use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_RGBA16F, f32_to_f16_bits};
    use postretro_level_format::octahedral::{
        DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
        irradiance_array_tile_location, irradiance_atlas_array_layout,
    };
    use postretro_level_format::sh_volume::{
        OCTAHEDRAL_PROBE_STRIDE, OctahedralShProbe, OctahedralShVolumeSection,
    };

    let tile = DEFAULT_IRRADIANCE_TILE_DIMENSION as usize;
    let [gx, gy, gz] = dimensions;
    let mut values = Vec::new();
    for z in 0..gz {
        for y in 0..gy {
            for x in 0..gx {
                values.push(value(x, y, z));
            }
        }
    }
    let probes: Vec<OctahedralShProbe> = values
        .iter()
        .map(|v| OctahedralShProbe {
            validity: u8::from(!v[0].is_nan()),
            // Far moments: Chebyshev visibility 1 at every sample.
            mean_distance: f32_to_f16_bits(100.0),
            mean_sq_distance: f32_to_f16_bits(10_000.0),
            ..Default::default()
        })
        .collect();
    let valid: Vec<[f32; 3]> = values.into_iter().filter(|v| !v[0].is_nan()).collect();
    let layout = irradiance_atlas_array_layout(
        [valid.len() as u32, 1, 1],
        DEFAULT_IRRADIANCE_TILE_DIMENSION,
        8192,
    )
    .expect("fixture atlas fits");
    let width = layout.atlas_width as usize;
    let layer_texels = width * layout.atlas_height as usize;
    let mut texels = vec![[0u16; 4]; layer_texels * layout.layer_count as usize];
    for (rank, rgb) in valid.iter().enumerate() {
        let [layer, tx, ty] = irradiance_array_tile_location(
            rank,
            layout.tiles_per_layer,
            layout.atlas_tiles_per_row,
        );
        let texel = [
            f32_to_f16_bits(rgb[0]),
            f32_to_f16_bits(rgb[1]),
            f32_to_f16_bits(rgb[2]),
            f32_to_f16_bits(1.0),
        ];
        for y in 0..tile {
            for x in 0..tile {
                let at = layer as usize * layer_texels
                    + (ty as usize * tile + y) * width
                    + tx as usize * tile
                    + x;
                texels[at] = texel;
            }
        }
    }
    OctahedralShVolumeSection {
        grid_origin: [0.0; 3],
        cell_size: [1.0; 3],
        grid_dimensions: dimensions,
        probe_stride: OCTAHEDRAL_PROBE_STRIDE,
        tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
        tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
        atlas_dimensions: [layout.atlas_width, layout.atlas_height],
        layer_count: layout.layer_count,
        tiles_per_layer: layout.tiles_per_layer,
        atlas_tiles_per_row: layout.atlas_tiles_per_row,
        probes,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        compact_atlas: texels
            .iter()
            .flat_map(|t| t.iter().flat_map(|c| c.to_le_bytes()))
            .collect(),
        animation_descriptors: Vec::new(),
        slot_for_map_light: Vec::new(),
    }
}
