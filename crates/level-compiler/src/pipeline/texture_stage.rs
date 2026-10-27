// Early world/mover materials and the late fluid/model/sprite tail.
// See: context/lib/build_pipeline.md §Compiler pipeline, §Baked texture mips

use super::*;
use crate::surface_materials::stats::{BOUNCE_SATURATION, BounceMaterials};
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
    let mut texture_bytes = texture_mips::StageTextureBytes::new();
    let (name_to_key, stats) = texture_mips::bake_materials_into(
        names,
        texture_root,
        prm_root,
        &mut texture_bytes,
        Some(BOUNCE_SATURATION),
    )?;
    Ok(MaterialStageResult {
        name_to_key,
        texture_bytes,
        bounce_materials: BounceMaterials::new(stats, BOUNCE_SATURATION)?,
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
    let fluid_names = &texture_names[materials.world_name_count..];
    if !fluid_names.is_empty() {
        let (keys, _) = texture_mips::bake_materials_into(
            fluid_names,
            texture_root,
            prm_root,
            &mut materials.texture_bytes,
            None,
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
    texture_mips::log_texture_byte_summary(materials.texture_bytes.summary());
    Ok(())
}
