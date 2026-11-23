// Glossy placement's gate: each material's opt-in read once, the census line, and the world faces placement binds under `--glossy-reflections`.
// See: context/lib/resource_management.md §5.4

use crate::governor::Governor;

use super::ReflectionProbeInputs;
use super::glossy_census::report_census;
use super::glossy_materials::GlossyMaterials;
use super::placement::ReflectorFace;
use super::world_reflectors::{bound_reflector_faces, world_reflector_faces};

/// The glossy world faces placement binds, in id-17 order: none without
/// `inputs.glossy_reflections`. The census line logs either way.
pub(crate) fn glossy_world_faces(
    inputs: &ReflectionProbeInputs<'_>,
    governor: &Governor,
) -> Vec<ReflectorFace> {
    let materials = GlossyMaterials::read(
        inputs.geometry,
        inputs.texture_keys,
        inputs.prm_root,
        governor,
    );
    report_census(inputs.geometry, &materials);
    if !inputs.glossy_reflections {
        return Vec::new();
    }
    let reflectors = world_reflector_faces(inputs.geometry, &materials);
    let faces = bound_reflector_faces(&reflectors);
    log::info!(
        "[ReflectionProbes] glossy reflections on: {} of {} glossy world face(s) face up and \
         take probes; walls, ceilings and slopes bind none yet",
        faces.len(),
        reflectors.faces.len(),
    );
    faces
}
