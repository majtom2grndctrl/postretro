// The bounce half's per-mover memo: one mover's bounce and closed-pose depth
// states, keyed on id 34's whole volume key and the mover as it is posed.
// See: context/lib/build_pipeline.md §Build Cache

use postretro_level_format::indirect_display::{DEFAULT_INDIRECT_SCALE, TONEMAP_KNEE_START};

use super::depth::DEPTH_SIGNIFICANCE_FLOOR;
use super::domain::{BOUNCE_FLOOR_LEVELS, DISPLAY_ALBEDO};
use super::posed_scene::PosedScene;
use super::states::{self, MoverBounceStates};
use crate::bc6h::BC6H_CODEC_VERSION;
use crate::cache::{CacheKey, StageCache};
use crate::sh_bake::ProbeGridLayout;
use crate::surface_materials::stats::BounceMaterials;

/// The memo's cache stage. Bump the version when the posed bake, the
/// affected-probe rule's code (the round trip's padding, the display
/// measure's sRGB encode or tonemap curve), the depth selection or the
/// payload changes. The rule's constants (the floors, the display albedo,
/// indirect scale and knee, the BC6H codec version) and the tracer epoch
/// (through id 34's volume key) fold on their own.
const STAGE_ID: &str = "mover_probe_bounce";
/// 2: posed patch-direct segments meet the posed sides through the static
/// tracer's own segment test ([`super::posed_solid::PosedSolid`]).
/// 3: per-terminus storage. A terminus within the round-trip bar of id 34,
/// or a depth pose no gather ray meets, stores nothing.
/// 4: id 34's kept later bounces add one by one in bounce order; posed
/// gather rays take the pose's inside-brush rule (lifted origin, no band).
const STAGE_VERSION: u32 = 4;

/// The per-mover memo over one stage cache, under one id-34 volume key: an
/// edit that re-bakes id 34 re-keys every mover.
pub(crate) struct BounceMemo<'c> {
    cache: &'c StageCache,
    prefix: blake3::Hasher,
}

impl<'c> BounceMemo<'c> {
    /// `volume_key` is id 34's whole volume key; `uncompressed` sets the
    /// affected-probe bar (exact when id 34 stores f16). The prefix folds
    /// every input the affected and stored decisions read beside the posed
    /// tiles: the round-trip bar's codec, the perceptual floor and the
    /// display it measures through, and the depth floor.
    pub(crate) fn new(cache: &'c StageCache, volume_key: &CacheKey, uncompressed: bool) -> Self {
        let mut prefix = blake3::Hasher::new();
        prefix.update(b"mover probe bounce\0");
        prefix.update(volume_key.as_filename().as_bytes());
        prefix.update(&[u8::from(uncompressed)]);
        prefix.update(&BC6H_CODEC_VERSION.to_le_bytes());
        for constant in [
            BOUNCE_FLOOR_LEVELS,
            DISPLAY_ALBEDO,
            DEFAULT_INDIRECT_SCALE,
            TONEMAP_KNEE_START,
            DEPTH_SIGNIFICANCE_FLOOR,
        ] {
            prefix.update(&constant.to_bits().to_le_bytes());
        }
        Self { cache, prefix }
    }

    /// The prefix, then the mover's posed bounce count and each pose: its
    /// terminus, its solid (bounds, brush planes and side triangles), each
    /// side's texture index and the albedo of every texture those sides name.
    /// The mover id is not folded: the payload never names it.
    pub(crate) fn key(
        &self,
        bounces: u32,
        scenes: &[PosedScene],
        materials: Option<&BounceMaterials>,
    ) -> CacheKey {
        let mut hasher = self.prefix.clone();
        hasher.update(&bounces.to_le_bytes());
        hasher.update(&(scenes.len() as u32).to_le_bytes());
        for scene in scenes {
            hasher.update(&[scene.terminus.index() as u8]);
            scene.fold_key(&mut hasher);
            match materials {
                Some(materials) => {
                    hasher.update(&[1]);
                    materials.fold_key(&mut hasher, scene.geometry());
                }
                None => {
                    hasher.update(&[0]);
                }
            }
        }
        CacheKey::new(STAGE_ID, STAGE_VERSION, hasher.finalize().as_bytes())
    }

    /// The states stored under `key`, or `None` on a miss. A payload that
    /// names an invalid probe, lists probes out of order or has a torn tile
    /// is corrupt: a miss that re-bakes. Runs outside any governor permit.
    pub(crate) fn get(
        &self,
        key: &CacheKey,
        mover_id: u32,
        layout: &ProbeGridLayout,
    ) -> Option<MoverBounceStates> {
        let bytes = self.cache.get(key)?;
        #[cfg(test)]
        let _payload = crate::live_bytes::hold_vec("mover bounce memo", &bytes);
        let decoded = states::decode(&bytes, mover_id, layout);
        if decoded.is_none() {
            log::warn!("[cache] corrupt {STAGE_ID} entry, re-baking");
        }
        decoded
    }

    /// Store `states`, empty included, so an unchanged mover hits next
    /// build. Runs outside any governor permit.
    pub(crate) fn put(&self, key: &CacheKey, states: &MoverBounceStates) {
        let bytes = states::encode(states);
        #[cfg(test)]
        let _payload = crate::live_bytes::hold_vec("mover bounce memo", &bytes);
        self.cache.put(key, &bytes);
    }

    /// One line per build: `hit` when every mover hit, else `miss` with the
    /// share that re-bakes.
    pub(crate) fn log_outcome(probed: usize, hits: usize) {
        if probed == 0 {
            return;
        }
        if hits == probed {
            log::info!("[cache] {STAGE_ID} hit ({probed} mover(s))");
        } else {
            log::info!(
                "[cache] {STAGE_ID} miss ({} of {probed} mover(s) re-bake)",
                probed - hits
            );
        }
    }
}
