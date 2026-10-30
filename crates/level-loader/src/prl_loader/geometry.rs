// Geometry section group: vertices, faces, texture names and cache keys.
// See: context/lib/build_pipeline.md §PRL Compilation

use std::collections::HashSet;

use postretro_level_format::SectionId;
use postretro_level_format::geometry::{GEOMETRY_CONTAINER_VERSION, GeometrySection, NO_TEXTURE};
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_render_data::geometry::WorldVertex;
use postretro_render_data::material;

use super::{section_validation, section_validation_from_error, stale_section};
use crate::prl_container::PrlContainer;
use crate::{FaceMeta, PrlLoadError};

fn derive_material_with_warning(
    texture_name: &str,
    warned_prefixes: &mut HashSet<String>,
) -> material::Material {
    let warned_count = warned_prefixes.len();
    let mat = material::derive_material(texture_name, warned_prefixes);
    let prefix = material::parse_prefix(texture_name);
    if mat == material::Material::Default
        && !prefix.is_empty()
        && warned_prefixes.len() > warned_count
    {
        log::warn!(
            "[Material] Unknown prefix '{}' in texture '{}' — using default material",
            prefix,
            texture_name,
        );
    }
    mat
}

pub(super) struct GeometryDecode {
    pub(super) vertices: Vec<WorldVertex>,
    pub(super) indices: Vec<u32>,
    pub(super) face_meta: Vec<FaceMeta>,
    pub(super) texture_names: Vec<String>,
    pub(super) texture_cache_keys: TextureCacheKeysSection,
}

pub(super) fn decode_geometry(container: &PrlContainer) -> Result<GeometryDecode, PrlLoadError> {
    let meta = container.metadata();
    let read_section = |section: SectionId| container.read_section(section as u32);

    // The vertex lightmap fields changed frame without changing their bytes,
    // so only the container entry version tells a stale geometry apart.
    if let Some(entry) = meta.find_section(SectionId::Geometry as u32)
        && entry.version != GEOMETRY_CONTAINER_VERSION
    {
        return Err(section_validation(
            "Geometry",
            format!(
                "container version {} (expected {GEOMETRY_CONTAINER_VERSION}); recompile with `prl-build`",
                entry.version
            ),
        ));
    }
    let geom_data = read_section(SectionId::Geometry)?
        .ok_or_else(|| stale_section("Geometry", SectionId::Geometry))?;
    let geom = GeometrySection::from_bytes(&geom_data)
        .map_err(|err| section_validation_from_error("Geometry", err))?;

    let texture_names_data = read_section(SectionId::TextureNames)?;
    let texture_names_section = match texture_names_data {
        Some(data) => Some(TextureNamesSection::from_bytes(&data)?),
        None => None,
    };
    let texture_names: Vec<String> = texture_names_section.map(|s| s.names).unwrap_or_default();

    // Required. Absence means the file is corrupt or was produced by a writer
    // that omitted section 32; reject so the texture cache never silently
    // degrades every surface to a placeholder on a bad file.
    let texture_cache_keys_data =
        read_section(SectionId::TextureCacheKeys)?.ok_or(PrlLoadError::NoTextureCacheKeys)?;
    let texture_cache_keys = TextureCacheKeysSection::from_bytes(&texture_cache_keys_data)?;

    let mut warned_prefixes = HashSet::new();
    let vertices: Vec<WorldVertex> = geom
        .vertices
        .iter()
        .map(|v| WorldVertex {
            position: v.position,
            base_uv: v.uv, // raw texel-space; normalized after texture dimensions are known
            normal_oct: v.normal_oct,
            tangent_packed: v.tangent_packed,
            lightmap_uv: v.lightmap_uv,
            lightmap_block: v.lightmap_block,
            animated_block: v.animated_block,
        })
        .collect();

    let face_meta: Vec<FaceMeta> = geom
        .faces
        .iter()
        .map(|f| {
            let (tex_idx, tex_name) = if f.texture_index == NO_TEXTURE {
                (None, String::new())
            } else {
                let name = texture_names
                    .get(f.texture_index as usize)
                    .cloned()
                    .unwrap_or_default();
                (Some(f.texture_index), name)
            };
            let mat = derive_material_with_warning(&tex_name, &mut warned_prefixes);
            FaceMeta {
                leaf_index: f.leaf_index,
                texture_index: tex_idx,
                texture_dimensions: (64, 64),
                texture_name: tex_name,
                material: mat,
            }
        })
        .collect();

    let indices = geom.indices;

    log::info!(
        "[PRL] Geometry: {} vertices, {} indices, {} faces, {} textures referenced",
        vertices.len(),
        indices.len(),
        face_meta.len(),
        texture_names.len()
    );

    Ok(GeometryDecode {
        vertices,
        indices,
        face_meta,
        texture_names,
        texture_cache_keys,
    })
}
