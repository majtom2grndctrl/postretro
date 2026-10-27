// Level texture sidecars and one combined memory report.
// See: context/lib/build_pipeline.md §Baked texture mips

use super::*;
use std::path::Path;

pub(super) fn bake_level_textures(
    input: &Path,
    texture_names: &[String],
    map_entities: &[map_data::MapEntityRecord],
    texture_root: &Path,
    prm_root: &Path,
) -> anyhow::Result<HashMap<String, [u8; 32]>> {
    let (name_to_key, mut texture_bytes) =
        texture_mips::bake_world_texture_mips(texture_names, texture_root, prm_root)?;
    let content_root = resolve_content_root(input);
    bake_model_textures(map_entities, &content_root, prm_root, &mut texture_bytes);
    bake_sprite_textures(map_entities, texture_root, prm_root, &mut texture_bytes);
    // Logged HERE, at the stage boundary, rather than inside the world bake:
    // the stage bakes world bundles, model sidecars AND sprite collections, and
    // a report flushed after the first omitted the other two entirely. Sprite
    // collections are the heaviest bundles the format produces — one complete
    // mip chain per frame — so a level with a billboard emitter under-reported
    // by more than a rounding error, and this total is what the streaming work
    // builds residency decisions on.
    texture_mips::log_texture_byte_summary(texture_bytes.summary());
    Ok(name_to_key)
}
