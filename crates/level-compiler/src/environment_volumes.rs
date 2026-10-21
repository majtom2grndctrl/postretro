// Canonical environment volumes → PRL EnvironmentVolumes section (id 52) and its
// optional PushForces sibling (id 53).
// See: context/lib/build_pipeline.md §PRL section IDs

mod closure;
mod push_split;
mod solid_index;
mod surface;
#[cfg(test)]
mod surface_tests;

use std::collections::HashMap;
use std::time::Instant;

use postretro_level_format::environment_volumes::{
    EnvironmentBrush, EnvironmentPlane, EnvironmentSurfaceFace, EnvironmentSurfaceVertex,
    EnvironmentVolumeRecord, EnvironmentVolumesSection,
};
use postretro_level_format::push_forces::PushForcesSection;
use postretro_level_format::texture_names::TextureNamesSection;

use crate::geometry::brush_side_texel_uv;
use crate::map_data::{BrushVolume, MapEnvironmentVolume};

use closure::{ClosureStats, close_seams};
use push_split::PushBrushes;
use surface::{SolidWorld, SurfacePiece, fluid_surface_pieces};

/// Encode the always-present section: the level default gravity plus every
/// volume in source map order, each fluid volume with its surface faces.
///
/// `solid_world` is the static world brush set; a fluid face buried in it
/// draws nothing. Surface textures join the shared level texture table: a
/// texture the world already uses keeps its index, and a fluid-only texture
/// appends after every world entry, so no world index moves. Call this once
/// every other texture user has interned its names.
///
/// Each face is split along every push brush over it, so a current drifts
/// exactly the part of the surface its push covers. Then every seam between
/// fluid faces closes, across volumes: faces that meet carry bit-identical
/// vertices along their shared boundary.
pub fn encode_environment_volumes_section(
    volumes: &[MapEnvironmentVolume],
    level_gravity: f32,
    solid_world: &[BrushVolume],
    texture_names: &mut TextureNamesSection,
) -> EnvironmentVolumesSection {
    encode_section(volumes, level_gravity, solid_world, texture_names, true)
}

/// The section as built before seam closure, for tests comparing the faces
/// closure starts from.
#[cfg(test)]
fn encode_environment_volumes_section_unclosed(
    volumes: &[MapEnvironmentVolume],
    level_gravity: f32,
    solid_world: &[BrushVolume],
    texture_names: &mut TextureNamesSection,
) -> EnvironmentVolumesSection {
    encode_section(volumes, level_gravity, solid_world, texture_names, false)
}

/// Every volume's surface pieces, in volume then piece order; a volume without
/// fluid has none.
fn surface_pieces<'a>(
    volumes: &'a [MapEnvironmentVolume],
    solid_world: &[BrushVolume],
) -> Vec<Vec<SurfacePiece<'a>>> {
    if !volumes.iter().any(|volume| volume.fluid.is_some()) {
        return vec![Vec::new(); volumes.len()];
    }
    let solid = SolidWorld::new(solid_world);
    let pushes = PushBrushes::new(volumes);
    volumes
        .iter()
        .map(|volume| fluid_surface_pieces(volume, &solid, &pushes))
        .collect()
}

fn encode_section(
    volumes: &[MapEnvironmentVolume],
    level_gravity: f32,
    solid_world: &[BrushVolume],
    texture_names: &mut TextureNamesSection,
    close: bool,
) -> EnvironmentVolumesSection {
    let started = Instant::now();
    let has_fluid = volumes.iter().any(|volume| volume.fluid.is_some());
    // Closure needs every volume's pieces before any is encoded.
    let mut pieces = surface_pieces(volumes, solid_world);
    let closure = if close {
        close_seams(&mut pieces)
    } else {
        ClosureStats::default()
    };
    let mut textures = TextureTable::new(texture_names);
    let mut face_count = 0usize;

    let records = volumes
        .iter()
        .zip(pieces)
        .map(|(volume, pieces)| {
            let surface_faces: Vec<EnvironmentSurfaceFace> = pieces
                .iter()
                .map(|piece| surface_face(piece, &mut textures))
                .collect();
            face_count += surface_faces.len();
            EnvironmentVolumeRecord {
                source_entity: volume.source_entity,
                priority: volume.priority,
                gravity: volume.gravity,
                push: volume.push.map(|push| push.to_array()),
                fluid: volume.fluid.clone().unwrap_or_default(),
                brushes: volume
                    .brushes
                    .iter()
                    .map(|brush| EnvironmentBrush {
                        aabb_min: brush.min,
                        aabb_max: brush.max,
                        planes: brush
                            .planes
                            .iter()
                            .map(|&[nx, ny, nz, distance]| EnvironmentPlane {
                                normal: [nx, ny, nz],
                                distance,
                            })
                            .collect(),
                    })
                    .collect(),
                surface_faces,
            }
        })
        .collect();

    if has_fluid {
        log::info!(
            "[Compiler] Fluid surfaces: {face_count} faces, {} appended textures, {} welded and {} inserted vertices, built in {:.1} ms",
            textures.appended,
            closure.welded,
            closure.inserted,
            started.elapsed().as_secs_f64() * 1000.0,
        );
    }
    EnvironmentVolumesSection {
        level_gravity,
        volumes: records,
    }
}

/// The optional PushForces section (id 53): one force per record, in record
/// order, written only when some push volume authors a force other than 1.
/// A record without push carries 1.
pub fn encode_push_forces_section(volumes: &[MapEnvironmentVolume]) -> Option<PushForcesSection> {
    volumes
        .iter()
        .any(|volume| volume.force != 1.0)
        .then(|| PushForcesSection {
            forces: volumes.iter().map(|volume| volume.force).collect(),
        })
}

/// Engine-space positions and texel-space UVs from the side's own projection,
/// the derivation static world faces use; the engine normalizes UVs by the
/// texture's size at load, as for world faces.
fn surface_face(
    piece: &SurfacePiece<'_>,
    textures: &mut TextureTable<'_>,
) -> EnvironmentSurfaceFace {
    let side = piece.side;
    EnvironmentSurfaceFace {
        texture_index: textures.index_of(&side.texture),
        vertices: piece
            .vertices
            .iter()
            .map(|&position| EnvironmentSurfaceVertex {
                position: position.as_vec3().to_array(),
                uv: brush_side_texel_uv(position, side.normal, &side.tex_projection),
            })
            .collect(),
    }
}

/// The shared level texture table, appended to only past its existing end.
struct TextureTable<'a> {
    names: &'a mut TextureNamesSection,
    index: HashMap<String, u32>,
    appended: usize,
}

impl<'a> TextureTable<'a> {
    fn new(names: &'a mut TextureNamesSection) -> Self {
        let mut index = HashMap::with_capacity(names.names.len());
        for (position, name) in names.names.iter().enumerate() {
            // First occurrence wins, matching the world's own dedupe.
            index.entry(name.clone()).or_insert(texture_index(position));
        }
        Self {
            names,
            index,
            appended: 0,
        }
    }

    fn index_of(&mut self, name: &str) -> u32 {
        if let Some(&index) = self.index.get(name) {
            return index;
        }
        let index = texture_index(self.names.names.len());
        self.names.names.push(name.to_owned());
        self.index.insert(name.to_owned(), index);
        self.appended += 1;
        index
    }
}

fn texture_index(position: usize) -> u32 {
    u32::try_from(position).expect("texture table fits the PRL u32 texture index")
}
