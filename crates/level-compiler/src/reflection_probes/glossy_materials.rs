// Glossy material verdicts: each used material's baked specular slot read once and tested by the opt-in.
// See: context/lib/resource_management.md §4.7, §5.4

use std::collections::HashMap;
use std::path::Path;

use postretro_level_format::glossy_opt_in::specular_slot_reflects;
use postretro_level_format::prm::{PrmFile, PrmFormat, PrmReadError, cache_filename_for_key};
use rayon::prelude::*;

use crate::geometry::GeometryResult;
use crate::governor::Governor;

/// Which materials reflect, by texture index.
pub(crate) struct GlossyMaterials {
    /// Index-aligned with the geometry's texture names; an unused material
    /// reads `false`.
    reflects: Vec<bool>,
    /// Materials at least one face uses.
    pub used: usize,
    /// Used materials whose sidecar could not be read: they reflect nothing.
    pub unreadable: usize,
}

impl GlossyMaterials {
    /// Reads the specular slot of every material a face of `geometry` uses,
    /// once each, from its `.prm` under `prm_root` (`texture_keys` names the
    /// sidecar; an absent or zero key means the material has no slots).
    pub(crate) fn read(
        geometry: &GeometryResult,
        texture_keys: &HashMap<String, [u8; 32]>,
        prm_root: &Path,
        governor: &Governor,
    ) -> Self {
        let names = &geometry.texture_names.names;
        let mut used = vec![false; names.len()];
        for face in &geometry.geometry.faces {
            // `NO_TEXTURE` and indices past the table bind placeholders.
            if let Some(slot) = used.get_mut(face.texture_index as usize) {
                *slot = true;
            }
        }
        let verdicts: Vec<Option<Result<bool, String>>> = names
            .par_iter()
            .zip(&used)
            .map(|(name, &used)| {
                used.then(|| {
                    let _permit = governor.enter();
                    match texture_keys.get(name) {
                        Some(key) if *key != [0u8; 32] => read_verdict(key, prm_root),
                        _ => Ok(false),
                    }
                })
            })
            .collect();
        let mut unreadable = 0;
        let reflects = verdicts
            .iter()
            .zip(names)
            .map(|(verdict, name)| match verdict {
                Some(Ok(reflects)) => *reflects,
                Some(Err(error)) => {
                    log::warn!(
                        "[ReflectionProbes] texture '{name}': {error}; its faces reflect nothing"
                    );
                    unreadable += 1;
                    false
                }
                None => false,
            })
            .collect();
        Self {
            reflects,
            used: used.iter().filter(|&&used| used).count(),
            unreadable,
        }
    }

    /// Verdicts given directly, index-aligned with the texture names, for
    /// tests of what reads them.
    #[cfg(test)]
    pub(crate) fn from_verdicts(reflects: Vec<bool>) -> Self {
        Self {
            used: reflects.len(),
            reflects,
            unreadable: 0,
        }
    }

    pub(crate) fn reflects(&self, texture_index: u32) -> bool {
        self.reflects
            .get(texture_index as usize)
            .copied()
            .unwrap_or(false)
    }

    /// The number of materials that reflect.
    pub(crate) fn glossy(&self) -> usize {
        self.reflects.iter().filter(|&&reflects| reflects).count()
    }
}

/// The opt-in over mip 0 of the specular slot in the sidecar `key` names.
fn read_verdict(key: &[u8; 32], prm_root: &Path) -> Result<bool, String> {
    let path = prm_root.join(format!("{}.prm", cache_filename_for_key(key)));
    let bytes =
        std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let (header, slots) = PrmFile::from_bytes_partial(&bytes);
    header.map_err(|error| format!("{} is not a readable .prm: {error}", path.display()))?;
    // Wire order: diffuse, specular, normal, emissive.
    let [_, specular, _, _] = slots;
    let slot = match specular {
        Ok(slot) => slot,
        Err(PrmReadError::NotPresent) => return Ok(false),
        Err(error) => {
            return Err(format!(
                "{}'s specular slot is unreadable: {error}",
                path.display()
            ));
        }
    };
    if slot.format != PrmFormat::Rgba8Unorm {
        return Ok(false);
    }
    // Layer 0's chain comes first, mip 0 first within it.
    let mip0_len = usize::from(slot.width) * usize::from(slot.height) * 4;
    let mip0 = slot.payload.get(..mip0_len).ok_or_else(|| {
        format!(
            "{}'s specular slot holds {} byte(s), short of its {}x{} mip 0",
            path.display(),
            slot.payload.len(),
            slot.width,
            slot.height
        )
    })?;
    Ok(specular_slot_reflects(slot.format, mip0))
}
