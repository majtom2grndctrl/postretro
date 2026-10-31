// The SH base's gathered volume as one build-cache entry, keyed on every input the gather reads.
// See: context/lib/build_pipeline.md §Build Cache

use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::bounce::{BOUNCE_RAYS_PER_PATCH, LATER_BOUNCE_COUNT};
use super::gather::PatchLookup;
use super::layer::{LayerKeys, SH_PATCH_LAYER_STAGE_VERSION};
use super::layer_set::LayerPlan;
use crate::cache::{CacheKey, StageCache};
use crate::sh_bake::{ProbeGridLayout, fold_gather_constants};

/// Build-cache stage id of the gathered id-34 volume.
pub(crate) const SH_BASE_VOLUME_STAGE_ID: &str = "sh_base_volume";
/// Epoch of the entry's payload and of everything the gather computes that
/// its key does not fold; bump when either changes.
pub(crate) const SH_BASE_VOLUME_STAGE_VERSION: u32 = 2;

/// The key of the gathered volume: every input the sum, the later bounces
/// and the probe gather read.
///
/// - The layer plan in content order, each layer's digest, effective
///   embedded flag and occurrence count, so the summed bounce one with its
///   multiplicity (a duplicate light misses), plus the layers' own epoch.
/// - What every layer key shares: the geometry a layer reads (so patches,
///   occluders and gather hits), the bounce materials, and the patch and
///   shading constants.
/// - Every face's cell id, which orders the BVH and so which of two
///   equal-distance hits a later-bounce or gather ray keeps
///   ([`LayerKeys::cell_id_fingerprint`]).
/// - The probe grid: spacing, origin, cell size, dimensions, miss distance
///   and every probe's validity, which the tree decides and no geometry
///   digest covers.
/// - The fill, with −0.0 read as 0.0 so one fill has one key.
/// - The later bounces' count and rays per patch, the patch lookup, and the
///   gather's own constants ([`fold_gather_constants`]).
///
/// No animated light enters: the descriptors and slot table rebuild every
/// build and the entry stores neither.
pub(crate) fn volume_key(
    layer_keys: &LayerKeys,
    plan: &LayerPlan,
    layout: &ProbeGridLayout,
    probe_spacing: f32,
    bounce_fill: f32,
) -> CacheKey {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"sh base volume\0");
    hasher.update(&layer_keys.shared_fingerprint());
    hasher.update(&layer_keys.cell_id_fingerprint());
    hasher.update(&SH_PATCH_LAYER_STAGE_VERSION.to_le_bytes());
    hasher.update(&(plan.layers.len() as u64).to_le_bytes());
    for planned in &plan.layers {
        hasher.update(planned.digest.as_bytes());
        hasher.update(&[u8::from(planned.embedded)]);
        hasher.update(&planned.occurrences.to_le_bytes());
    }

    hasher.update(&probe_spacing.to_le_bytes());
    for v in layout.world_min.to_array() {
        hasher.update(&v.to_le_bytes());
    }
    for v in layout.cell_size {
        hasher.update(&v.to_le_bytes());
    }
    for v in layout.dims {
        hasher.update(&v.to_le_bytes());
    }
    hasher.update(&layout.far_sentinel.to_le_bytes());
    hasher.update(&(layout.validity.len() as u64).to_le_bytes());
    hasher.update(&layout.validity);

    let fill = if bounce_fill == 0.0 { 0.0 } else { bounce_fill };
    hasher.update(&fill.to_bits().to_le_bytes());
    hasher.update(&LATER_BOUNCE_COUNT.to_le_bytes());
    hasher.update(&BOUNCE_RAYS_PER_PATCH.to_le_bytes());
    hasher.update(&[PatchLookup::PRODUCTION.key_tag()]);
    fold_gather_constants(&mut hasher);
    CacheKey::new(
        SH_BASE_VOLUME_STAGE_ID,
        SH_BASE_VOLUME_STAGE_VERSION,
        hasher.finalize().as_bytes(),
    )
}

/// The build cache as the gathered volume reads and writes it.
pub(crate) struct VolumeCache<'a> {
    pub(crate) cache: &'a StageCache,
    pub(crate) key: CacheKey,
}

impl VolumeCache<'_> {
    /// The cached volume over `layout`, without its animation tables, or
    /// `None` on a miss. An entry that does not decode, or decodes to
    /// another grid, is a miss. Runs outside any permit.
    pub(crate) fn load(&self, layout: &ProbeGridLayout) -> Option<OctahedralShVolumeSection> {
        note_cache_io();
        let section = self.cache.get(&self.key).and_then(|bytes| {
            #[cfg(test)]
            let _payload = crate::live_bytes::hold_vec("id-34 cache payload", &bytes);
            let section = OctahedralShVolumeSection::from_bytes(&bytes)
                .ok()
                .filter(|section| fits(section, layout));
            // The decoded copy beside the raw payload; the stage holds it
            // once it returns.
            #[cfg(test)]
            let _decoded = section.as_ref().map(|section| {
                crate::live_bytes::hold(
                    "id-34 section",
                    crate::sh_bake::section_heap_bytes(section),
                )
            });
            if section.is_none() {
                log::warn!("[cache] corrupt sh_base_volume entry, re-gathering");
            }
            section
        });
        log::info!(
            "[cache] sh_base_volume {}",
            if section.is_some() { "hit" } else { "miss" }
        );
        section
    }

    /// Write the gathered volume with its animation tables left out, so a
    /// later build rebuilds them from its own light set. Runs outside any
    /// permit.
    pub(crate) fn store(&self, section: &mut OctahedralShVolumeSection) {
        note_cache_io();
        let descriptors = std::mem::take(&mut section.animation_descriptors);
        let slots = std::mem::take(&mut section.slot_for_map_light);
        match section.try_to_bytes() {
            Ok(bytes) => {
                #[cfg(test)]
                let _encoded = crate::live_bytes::hold_vec("id-34 encoded copy", &bytes);
                self.cache.put(&self.key, &bytes);
            }
            Err(error) => log::warn!("[cache] sh_base_volume not cached: {error}"),
        }
        section.animation_descriptors = descriptors;
        section.slot_for_map_light = slots;
    }
}

/// Whether a decoded entry is the pre-encode volume of `layout`'s grid, with
/// no animation table: the shape the key promises.
fn fits(section: &OctahedralShVolumeSection, layout: &ProbeGridLayout) -> bool {
    let origin = layout.world_min.to_array().map(|v| v as f32);
    section.grid_dimensions == layout.dims
        && section.grid_origin == origin
        && section.cell_size == layout.cell_size
        && section.irradiance_format == IRRADIANCE_FORMAT_RGBA16F
        && section.animation_descriptors.is_empty()
        && section.slot_for_map_light.is_empty()
}

/// Count one volume cache read or write, and whether this thread holds a
/// governor permit while making it.
fn note_cache_io() {
    #[cfg(test)]
    {
        let under_permit = crate::governor::permits_held_on_this_thread() > 0;
        crate::bake_counters::bump(|counts| {
            counts.volume_cache_io += 1;
            counts.volume_cache_io_under_permit += u64::from(under_permit);
        });
    }
}
