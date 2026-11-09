// The bounce half's per-mover memo: one mover's bounce and closed-pose depth
// states, keyed on id 34's whole volume key and the mover as it is posed.
// See: context/lib/build_pipeline.md §Build Cache

use super::depth::DEPTH_SIGNIFICANCE_FLOOR;
use super::domain::BOUNCE_FLOOR_LEVELS;
use super::posed_scene::{PosedScene, PosedSurfaces};
use super::states::{self, MoverBounceStates};
use super::visibility::InsideBrushRule;
use crate::cache::{CacheKey, StageCache};
use crate::sh_bake::ProbeGridLayout;
use crate::surface_materials::stats::BounceMaterials;

/// The memo's cache stage. Bump the version when the posed bake, the
/// affected-probe rule, the depth selection or the payload changes; the
/// floors, the inside-brush rule and the tracer epoch (through id 34's
/// volume key) fold on their own.
const STAGE_ID: &str = "mover_probe_bounce";
const STAGE_VERSION: u32 = 1;

/// The per-mover memo over one stage cache, under one id-34 volume key: an
/// edit that re-bakes id 34 re-keys every mover.
pub(crate) struct BounceMemo<'c> {
    cache: &'c StageCache,
    prefix: blake3::Hasher,
}

impl<'c> BounceMemo<'c> {
    /// `volume_key` is id 34's whole volume key; `uncompressed` sets the
    /// affected-probe bar (exact when id 34 stores f16).
    pub(crate) fn new(cache: &'c StageCache, volume_key: &CacheKey, uncompressed: bool) -> Self {
        let mut prefix = blake3::Hasher::new();
        prefix.update(b"mover probe bounce\0");
        prefix.update(volume_key.as_filename().as_bytes());
        prefix.update(&[u8::from(uncompressed)]);
        prefix.update(&BOUNCE_FLOOR_LEVELS.to_le_bytes());
        prefix.update(&DEPTH_SIGNIFICANCE_FLOOR.to_le_bytes());
        Self { cache, prefix }
    }

    /// The prefix, then the mover's posed bounce count and each pose: its
    /// inside-brush rule, whether its sides reflect, its brushes' planes and
    /// bounds, its sides' geometry (the patches it bounces from) and the
    /// albedo of every texture they name. The mover id is not folded: the
    /// payload never names it.
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
            hasher.update(&[match scene.rule {
                InsideBrushRule::SkipContainingBrush => 0,
                InsideBrushRule::Dark => 1,
            }]);
            hasher.update(&[match scene.surfaces {
                PosedSurfaces::Patches => 0,
                PosedSurfaces::Black => 1,
            }]);
            scene.fold_key(&mut hasher);
            match materials {
                Some(materials) => {
                    hasher.update(&[1]);
                    materials.fold_key(&mut hasher, &scene.geometry);
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
