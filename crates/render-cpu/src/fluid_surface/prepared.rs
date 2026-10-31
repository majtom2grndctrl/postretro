// CPU half of the fluid-surface install: the packed mesh, its per-vertex
// probe indices and its planar runs, from level data, diffuse dimensions and
// the SH sampler lattice alone. The renderer's GPU half uploads the result.
// See: context/lib/rendering_pipeline.md §7.3a

use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use postretro_level_format::reflection_probes::ReflectionProbesSection;

use super::reflection::{FLUID_REFLECTION_NO_PROBE, fill_probe_indices, usable_probe_section};
use super::{FluidGridLattice, FluidSurfaceMesh, pack_fluid_surfaces};
use crate::planar_reflection::{PLANE_HEIGHT_TOLERANCE, PLANE_UP_NORMAL_MIN};

/// The texel format a usable probe section's colour tiles upload as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeArrayFormat {
    /// BC6H tiles stay compressed through upload, as the SH atlases do.
    Bc6hRgbUfloat,
    /// The uncompressed debug tag.
    Rgba16Float,
}

/// The probe array format for `section`'s colour tiles, or `None` when the
/// array cannot upload them (a BC6H tile edge must be a multiple of 4).
pub fn probe_array_format(section: &ReflectionProbesSection) -> Option<ProbeArrayFormat> {
    match section.atlas_format {
        IRRADIANCE_FORMAT_BC6H if section.tile_dimension.is_multiple_of(4) => {
            Some(ProbeArrayFormat::Bc6hRgbUfloat)
        }
        IRRADIANCE_FORMAT_RGBA16F => Some(ProbeArrayFormat::Rgba16Float),
        _ => None,
    }
}

/// The section whose probes the renderer installs: the usable one (some
/// probe within the cap, a tile dimension and border the decoder accepts,
/// one probe per layer, a tile format the array uploads, colour and distance
/// payloads matching the header) with an uploadable array format. The loader
/// refuses anything else; a hand-built section that slips past is dropped
/// whole and reflects nothing rather than failing texture creation. The draw
/// predicate's bound face reads the same usable answer, and install feeds
/// this one to the table, the probe indices and the arrays, so a face never
/// draws for or samples a probe that is not there.
pub fn installable_probe_section(
    section: Option<&ReflectionProbesSection>,
) -> Option<&ReflectionProbesSection> {
    usable_probe_section(section).filter(|section| probe_array_format(section).is_some())
}

/// One run of a material range's triangles: `height` is the plane of a run of
/// horizontal, up-facing, probe-bound triangles at one height, else `None`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanarIndexRange {
    pub start: u32,
    pub count: u32,
    pub height: Option<f32>,
}

/// Whether two runs' planes are the same plane, within the height tolerance.
pub fn same_plane(a: Option<f32>, b: Option<f32>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= PLANE_HEIGHT_TOLERANCE,
        (None, None) => true,
        _ => false,
    }
}

/// Split every material range of `mesh` into runs by plane, in index order,
/// never across a range. A triangle has a plane only when all three corners
/// face up, sit within the height tolerance of its first corner and are bound
/// to a probe, so a plane run always takes the reflecting gate. Install-time.
pub fn split_planar_ranges(
    mesh: &FluidSurfaceMesh,
    volumes: &[EnvironmentVolumeRecord],
    probes: Option<&ReflectionProbesSection>,
) -> Vec<PlanarIndexRange> {
    let mut probe = vec![FLUID_REFLECTION_NO_PROBE; mesh.vertices.len()];
    fill_probe_indices(volumes, &mesh.face_drift_maps, probes, &mut probe);
    split_planar_ranges_by_probe(mesh, &probe)
}

/// [`split_planar_ranges`] over per-vertex probe indices already filled for
/// `mesh`.
fn split_planar_ranges_by_probe(mesh: &FluidSurfaceMesh, probe: &[u32]) -> Vec<PlanarIndexRange> {
    let mut parts: Vec<PlanarIndexRange> = Vec::new();
    for volume in &mesh.volumes {
        for material in &volume.material_ranges {
            let first_part = parts.len();
            let end = material.index_start + material.index_count;
            for start in (material.index_start..end).step_by(3) {
                let corners = [0, 1, 2].map(|k| mesh.indices[(start + k) as usize] as usize);
                let h = mesh.vertices[corners[0]].position[1];
                let height = corners
                    .iter()
                    .all(|&corner| {
                        let vertex = &mesh.vertices[corner];
                        vertex.normal[1] > PLANE_UP_NORMAL_MIN
                            && (vertex.position[1] - h).abs() <= PLANE_HEIGHT_TOLERANCE
                            && probe[corner] != FLUID_REFLECTION_NO_PROBE
                    })
                    .then_some(h);
                match parts[first_part..].last() {
                    Some(last) if same_plane(last.height, height) => {
                        parts.last_mut().expect("a run exists").count += 3;
                    }
                    _ => parts.push(PlanarIndexRange {
                        start,
                        count: 3,
                        height,
                    }),
                }
            }
        }
    }
    parts
}

/// One level's fluid surfaces, ready for the renderer's upload: the mesh,
/// one probe index per mesh vertex, whether any face is probe-bound (which
/// keys the screen-space reflection target), and every material range's
/// planar runs.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedFluidSurfaces {
    pub mesh: FluidSurfaceMesh,
    pub probe_indices: Vec<u32>,
    pub has_probe_bound_face: bool,
    pub planar_parts: Vec<PlanarIndexRange>,
}

// Compile-time guard: prepared on a load-pool thread, uploaded elsewhere.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<PreparedFluidSurfaces>();
};

/// Pack and route a level's fluid surfaces. `diffuse_dims` is each world
/// material's diffuse `(width, height)` (fluid UVs normalize by them, as
/// world UVs do), `material_count` the world material count, and `lattice`
/// the SH sampler lattice faces are cut along
/// (`sh_volume::sh_sampler_lattice`). `None` when no volume has a surface
/// face: the level never opens the pass and allocates nothing.
pub fn prepare_fluid_surfaces(
    volumes: &[EnvironmentVolumeRecord],
    reflection_probes: Option<&ReflectionProbesSection>,
    diffuse_dims: &[(u32, u32)],
    material_count: usize,
    lattice: Option<&FluidGridLattice>,
) -> Option<PreparedFluidSurfaces> {
    let texture_dimensions: Vec<[u32; 2]> = diffuse_dims
        .iter()
        .map(|&(width, height)| [width, height])
        .collect();
    let mesh = pack_fluid_surfaces(volumes, &texture_dimensions, material_count, lattice);
    if mesh.indices.is_empty() {
        return None;
    }
    let reflection_probes = installable_probe_section(reflection_probes);
    let mut probe_indices = vec![FLUID_REFLECTION_NO_PROBE; mesh.vertices.len()];
    fill_probe_indices(
        volumes,
        &mesh.face_drift_maps,
        reflection_probes,
        &mut probe_indices,
    );
    let has_probe_bound_face = probe_indices
        .iter()
        .any(|&index| index != FLUID_REFLECTION_NO_PROBE);
    let planar_parts = split_planar_ranges_by_probe(&mesh, &probe_indices);
    Some(PreparedFluidSurfaces {
        mesh,
        probe_indices,
        has_probe_bound_face,
        planar_parts,
    })
}
