// The bounce half's per-pose memo: one pose's raw bounce and closed-pose
// depth states, keyed on id 34's whole volume key, the pose's kind, the
// bounce count it bakes and the mover as that pose places it.
// See: context/lib/build_pipeline.md §Build Cache

use postretro_level_format::indirect_display::{DEFAULT_INDIRECT_SCALE, TONEMAP_KNEE_START};

use super::bake::MoverBounceJob;
use super::domain::{BOUNCE_FLOOR_LEVELS, DISPLAY_ALBEDO};
use super::pose_payload;
use super::states::PoseState;
use crate::bc6h::BC6H_CODEC_VERSION;
use crate::cache::{CacheKey, StageCache};
use crate::sh_bake::ProbeGridLayout;
use crate::surface_materials::stats::BounceMaterials;

/// The memo's cache stage. Bump the version when the posed bake, the
/// affected-probe rule's code (the round trip's padding, the display
/// measure's sRGB encode or tonemap curve), a mid pose's classification or
/// the payload changes. The rule's constants (the bounce floor, the display
/// albedo, indirect scale and knee, the BC6H codec version) and the tracer
/// epoch (through id 34's volume key) fold on their own.
const STAGE_ID: &str = "mover_probe_bounce";
/// 2: posed patch-direct segments meet the posed sides through the static
/// tracer's own segment test ([`super::posed_solid::PosedSolid`]).
/// 3: per-terminus storage. A terminus within the round-trip bar of id 34,
/// or a depth pose no gather ray meets, stores nothing.
/// 4: id 34's kept later bounces add one by one in bounce order; posed
/// gather rays take the pose's inside-brush rule (lifted origin, no band).
/// 5: a terminus that encloses the probe stores no state and reads id 34;
/// its depth still bakes dark.
/// 6: enclosure takes the gather's lift as an inset: a probe on a face or
/// less than the lift inside it keeps the per-ray rule.
/// 7: states at every pose along the path: per-pose marks, mid poses over
/// the near-field domain with the termini's later shares blended in.
/// 8: a mid pose that encloses the probe stores no bounce state and blends
/// across; its depth still bakes dark.
/// 9: a mid pose traces its own posed later bounces over its near-field
/// domain, rays leaving it reading id 34's, in place of the termini's
/// blended later shares.
/// 10: one entry per pose holding its raw states, before its mover's poses
/// reduce; the depth floor and the tile range apply after every read.
const STAGE_VERSION: u32 = 10;

/// What a pose bakes: a terminus gathers every valid probe, a mid pose its
/// mover's near-field domain. Folded into the key, so the two never share
/// an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PoseKind {
    Terminus,
    Mid,
}

impl PoseKind {
    pub(crate) fn of(pose: usize, pose_count: usize) -> Self {
        if pose == 0 || pose + 1 == pose_count {
            Self::Terminus
        } else {
            Self::Mid
        }
    }
}

/// The per-pose memo over one stage cache, under one id-34 volume key: an
/// edit that re-bakes id 34 re-keys every pose of every mover.
pub(crate) struct PoseMemo<'c> {
    cache: &'c StageCache,
    prefix: blake3::Hasher,
}

impl<'c> PoseMemo<'c> {
    /// `volume_key` is id 34's whole volume key: the patches, static bounce
    /// one, lights, bounce materials, fill, probe grid and validity, the
    /// gathered id 34 a pose's bars compare against and the mover-absent
    /// later bounces a pose keeps or reads past its domain, the tracer epoch
    /// and the tile range. `uncompressed` sets the round-trip bar (exact
    /// when id 34 stores f16). The prefix also folds the round-trip bar's
    /// codec and the perceptual floor with the display it measures through,
    /// which decide what a pose's raw states hold. The depth significance
    /// floor is not folded: it applies after every pose resolves.
    pub(crate) fn new(cache: &'c StageCache, volume_key: &CacheKey, uncompressed: bool) -> Self {
        let mut prefix = blake3::Hasher::new();
        prefix.update(b"mover probe pose\0");
        prefix.update(volume_key.as_filename().as_bytes());
        prefix.update(&[u8::from(uncompressed)]);
        prefix.update(&BC6H_CODEC_VERSION.to_le_bytes());
        for constant in [
            BOUNCE_FLOOR_LEVELS,
            DISPLAY_ALBEDO,
            DEFAULT_INDIRECT_SCALE,
            TONEMAP_KNEE_START,
        ] {
            prefix.update(&constant.to_bits().to_le_bytes());
        }
        Self { cache, prefix }
    }

    /// The prefix, then pose `pose` of `job` as it bakes at `bounces`
    /// posed bounces (the effective count: `light_bounces` clamped to id
    /// 34's, and whatever cap a build fidelity puts on it): its kind, the
    /// bounce count, its solid (bounds, brush planes and side triangles),
    /// each side's texture index and the albedo of every texture those
    /// sides name; and, at a mid pose only, what it reads of the near-field
    /// domain: the probes it gathers (`job.domain_probes`) and, re-baking
    /// bounce, the static patches its later bounces trace
    /// (`domain_patches`, [`super::NearFieldDomain::static_patches`]). The
    /// domain is read as nothing else, so a domain edit that moves neither
    /// list keeps the pose. A terminus gathers every valid probe and traces
    /// every static patch, so it never folds the domain, and a
    /// middle-waypoint move keeps it. Neither kind folds its knot, its
    /// index, the mover's pose count, the author ramp or the mover id: none
    /// changes what the pose bakes, so a `light_poses` edit keeps the
    /// termini and every mid pose it leaves in place.
    pub(crate) fn key(
        &self,
        job: &MoverBounceJob,
        pose: usize,
        bounces: u32,
        materials: Option<&BounceMaterials>,
        domain_patches: &[u32],
    ) -> CacheKey {
        let kind = PoseKind::of(pose, job.scenes.len());
        let scene = &job.scenes[pose];
        let mut hasher = self.prefix.clone();
        hasher.update(&[match kind {
            PoseKind::Terminus => 0,
            PoseKind::Mid => 1,
        }]);
        hasher.update(&bounces.to_le_bytes());
        if kind == PoseKind::Mid {
            let patches = if bounces > 0 { domain_patches } else { &[] };
            for list in [job.domain_probes.as_slice(), patches] {
                hasher.update(&(list.len() as u64).to_le_bytes());
                for index in list {
                    hasher.update(&index.to_le_bytes());
                }
            }
        }
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
        CacheKey::new(STAGE_ID, STAGE_VERSION, hasher.finalize().as_bytes())
    }

    /// The pose's states stored under `key`, or `None` on a miss. A payload
    /// of the other kind, or one that names an invalid probe, lists probes
    /// out of order, marks a probe badly, lists a mid pose's probes other
    /// than its domain's (`domain`, ascending) or has a torn tile, is
    /// corrupt: a miss that re-bakes. Runs outside any governor permit.
    pub(crate) fn get(
        &self,
        key: &CacheKey,
        kind: PoseKind,
        layout: &ProbeGridLayout,
        domain: &[u32],
    ) -> Option<PoseState> {
        let bytes = self.cache.get(key)?;
        #[cfg(test)]
        let _payload = crate::live_bytes::hold_vec("mover pose memo", &bytes);
        let decoded = pose_payload::decode(&bytes, kind, layout, domain);
        if decoded.is_none() {
            log::warn!("[cache] corrupt {STAGE_ID} entry, re-baking");
        }
        decoded
    }

    /// Store one pose's states, empty included, so an unchanged pose hits
    /// next build. Runs outside any governor permit.
    pub(crate) fn put(&self, key: &CacheKey, state: &PoseState) {
        let bytes = pose_payload::encode(state);
        #[cfg(test)]
        let _payload = crate::live_bytes::hold_vec("mover pose memo", &bytes);
        self.cache.put(key, &bytes);
    }

    /// One line per build: `hit` when every pose of every mover hit, else
    /// `miss` with the poses and movers that re-bake.
    pub(crate) fn log_outcome(poses: usize, missed_poses: usize, movers: usize, missed: usize) {
        if poses == 0 {
            return;
        }
        if missed_poses == 0 {
            log::info!("[cache] {STAGE_ID} hit ({poses} pose(s) of {movers} mover(s))");
        } else {
            log::info!(
                "[cache] {STAGE_ID} miss ({missed_poses} of {poses} pose(s) re-bake, over \
                 {missed} of {movers} mover(s))"
            );
        }
    }
}

#[cfg(test)]
#[path = "pose_memo_tests.rs"]
mod tests;
