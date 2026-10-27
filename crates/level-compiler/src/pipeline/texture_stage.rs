// Early world/mover materials and the late fluid/model/sprite tail.
// See: context/lib/build_pipeline.md §Compiler pipeline, §Baked texture mips

use super::*;
use crate::surface_materials::stats::{BOUNCE_SATURATION, BounceMaterials, validate_saturation};
use std::path::Path;

pub(super) struct MaterialStageResult {
    pub name_to_key: HashMap<String, [u8; 32]>,
    pub texture_bytes: texture_mips::StageTextureBytes,
    pub bounce_materials: BounceMaterials,
    world_name_count: usize,
}

pub(super) fn bake_world_materials(
    names: &[String],
    texture_root: &Path,
    prm_root: &Path,
) -> anyhow::Result<MaterialStageResult> {
    bake_world_materials_at(names, texture_root, prm_root, BOUNCE_SATURATION)
}

fn bake_world_materials_at(
    names: &[String],
    texture_root: &Path,
    prm_root: &Path,
    saturation: f32,
) -> anyhow::Result<MaterialStageResult> {
    // Validate even an empty material stage before walking the texture root.
    validate_saturation(saturation)?;
    let mut texture_bytes = texture_mips::StageTextureBytes::new();
    let (name_to_key, stats) =
        texture_mips::bake_materials_into(names, texture_root, prm_root, &mut texture_bytes, true)?;
    // SH hits index the table by texture index, so one record per name is the contract.
    anyhow::ensure!(
        stats.len() == names.len(),
        "material stage produced {} stats for {} texture names",
        stats.len(),
        names.len()
    );
    Ok(MaterialStageResult {
        name_to_key,
        texture_bytes,
        bounce_materials: BounceMaterials::new(stats, saturation)?,
        world_name_count: names.len(),
    })
}

pub(super) fn bake_remaining_textures(
    input: &Path,
    texture_names: &[String],
    map_entities: &[map_data::MapEntityRecord],
    texture_root: &Path,
    prm_root: &Path,
    materials: &mut MaterialStageResult,
) -> anyhow::Result<()> {
    // Fluid names append after every world/mover name, so existing indices and
    // first-name byte-report ownership survive the split.
    debug_assert!(
        texture_names.len() >= materials.world_name_count,
        "texture names may only grow between the material stage and the fluid tail"
    );
    let fluid_names = &texture_names[materials.world_name_count..];
    if !fluid_names.is_empty() {
        let (keys, _) = texture_mips::bake_materials_into(
            fluid_names,
            texture_root,
            prm_root,
            &mut materials.texture_bytes,
            false,
        )?;
        materials.name_to_key.extend(keys);
    }
    let content_root = resolve_content_root(input);
    bake_model_textures(
        map_entities,
        &content_root,
        prm_root,
        &mut materials.texture_bytes,
    );
    bake_sprite_textures(
        map_entities,
        texture_root,
        prm_root,
        &mut materials.texture_bytes,
    );
    // Flush once, after world, fluid, model and sprite bundles are all charged:
    // the report describes the level's whole texture cost, deduplicated.
    texture_mips::log_texture_byte_summary(materials.texture_bytes.summary());
    Ok(())
}

#[cfg(test)]
#[path = "texture_stage_tests.rs"]
mod tests;
