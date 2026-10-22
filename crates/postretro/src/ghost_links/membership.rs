// Ghost membership ramp: keys, presented membership, frozen poses and the per-frame output.
// See: context/lib/entity_model.md §7d · context/lib/rendering_pipeline.md §7.9, §7.8

use std::collections::HashMap;

use postretro_entities::{EntityId, Transform};
use postretro_foundation::SeamlessMapping;
use postretro_render_cpu::flash_clamp::INTENSITY_RATE_PER_SECOND;
use postretro_render_cpu::flash_limiter::HITCH_CEILING_SECONDS;

use super::GhostLook;
use super::clocks::MemberLag;
use super::eligibility::GhostCategory;

/// One ghost: a member entity seen through one link. An entity inside two links
/// is two keys. `EntityId` is generation-checked, so a re-tenanted slot is a new
/// key and its ghost starts fresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct GhostKey {
    pub link: u16,
    pub entity: EntityId,
}

/// One ghost this frame, for the draw. Its pose is `source` carried through
/// `mapping` (`mapped`); a mesh draws at `mapping.matrix() * model(source)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GhostMember {
    pub link: u16,
    pub entity: EntityId,
    pub category: GhostCategory,
    /// Presented membership in `[0, 1]`; presented opacity is the link's
    /// opacity times this.
    pub membership: f32,
    /// The membership target is 0 and the ghost fades at its last in-volume
    /// pose. The entity may be gone, so a draw keeps the last instance group it
    /// built for this key rather than sampling the entity again.
    pub frozen: bool,
    pub mapping: SeamlessMapping,
    /// The presented pose in the source copy: live, or the last one inside the
    /// volume while frozen. Never a pose outside the volume.
    pub source: Transform,
    /// `source` mapped into the other copy.
    pub mapped: Transform,
    /// The link's look as this member presents it, before membership and the
    /// flash clamp: read at the host time the member presents at on this
    /// machine, so a `setColorFilter` and the motion it rides with present on
    /// one frame.
    pub look: GhostLook,
}

/// Membership at or below this after a fade step is fully faded.
const FADED_OUT: f32 = 1e-6;

#[derive(Debug, Clone, Copy)]
struct Entry {
    category: GhostCategory,
    membership: f32,
    /// This frame's membership target.
    target: bool,
    /// Created this frame: presents at its starting membership, unstepped.
    created: bool,
    /// The last presented pose evaluated inside the volume with target 1.
    source: Transform,
    /// How far behind this machine's script clock the member last presented.
    /// A fading ghost keeps it, so its look runs on at the same delay.
    lag: MemberLag,
}

/// Presented membership per ghost key, ramping linearly toward each frame's
/// target at the photosensitivity limiter's rate cap and reversing from the
/// presented value. Storage is reused frame to frame.
#[derive(Debug)]
pub(crate) struct MembershipTable {
    entries: HashMap<GhostKey, Entry>,
    members: Vec<GhostMember>,
    /// The first evaluation after a level install: a new key whose link's
    /// local spawn state already shows it starts at full, so members of links
    /// armed on spawn present in full on frame 1. Every other key starts at 0.
    settle_at_target: bool,
    /// The viewer the last frame began with, so a viewer switch is seen.
    viewer: Option<EntityId>,
}

impl Default for MembershipTable {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            members: Vec::new(),
            settle_at_target: true,
            viewer: None,
        }
    }
}

/// How far presented membership moves this frame: the limiter's rate cap over
/// presented-frame time, a hitch stepping at most the hitch ceiling. The ramp
/// runs whether the limiter is on or off, and continues while script time is
/// frozen (the dev `freeze_time`).
pub(crate) fn ramp_step(elapsed_seconds: f32) -> f32 {
    let elapsed = if elapsed_seconds.is_finite() {
        elapsed_seconds.max(0.0)
    } else {
        0.0
    };
    INTENSITY_RATE_PER_SECOND * elapsed.min(HITCH_CEILING_SECONDS)
}

impl MembershipTable {
    /// Start a frame: every target drops to 0 until a member marks it. Ghosts
    /// of `viewer`'s pawn and of its projectiles drop at once rather than fade:
    /// the camera can come to follow a pawn that was ghosting, and a viewer
    /// never sees its own pawn's or projectiles' ghosts. `projectile_owner`
    /// names a projectile's owning pawn; it is asked only on a frame whose
    /// viewer differs from the last frame's, since candidates never offer the
    /// viewer's projectiles while the viewer holds.
    pub(crate) fn begin_frame(
        &mut self,
        viewer: Option<EntityId>,
        projectile_owner: impl Fn(EntityId) -> Option<EntityId>,
    ) {
        let switched = viewer.is_some() && viewer != self.viewer;
        self.viewer = viewer;
        self.entries.retain(|key, entry| {
            entry.target = false;
            Some(key.entity) != viewer && !(switched && projectile_owner(key.entity) == viewer)
        });
    }

    /// `key`'s member is inside the volume of an armed link showing its
    /// category, presented at `source`, `lag` behind this machine's script clock.
    /// `spawn_shows` says the link's local spawn state already shows it: only
    /// then may the install settle start a new key at full. An arm or category
    /// the first evaluation reads from a tick or a received record ramps in.
    pub(crate) fn mark_target(
        &mut self,
        key: GhostKey,
        category: GhostCategory,
        source: Transform,
        lag: MemberLag,
        spawn_shows: bool,
    ) {
        let settle = self.settle_at_target && spawn_shows;
        let entry = self.entries.entry(key).or_insert(Entry {
            category,
            membership: if settle { 1.0 } else { 0.0 },
            target: true,
            created: true,
            source,
            lag,
        });
        entry.category = category;
        entry.target = true;
        entry.source = source;
        entry.lag = lag;
    }

    /// Step every ramp by `step`, drop ghosts that faded out, and rebuild the
    /// output in key order. `resolve` names each link's landmark → target map
    /// and its look for a member presenting at a lag.
    pub(crate) fn finish_frame(
        &mut self,
        step: f32,
        resolve: impl Fn(u16, MemberLag) -> Option<(SeamlessMapping, GhostLook)>,
    ) {
        self.settle_at_target = false;
        self.entries.retain(|_, entry| {
            if entry.created {
                entry.created = false;
            } else if entry.target {
                entry.membership = (entry.membership + step).min(1.0);
            } else {
                // Snap the float residue of equal up and down steps, so a fade
                // ends on the frame its last step lands.
                let faded = entry.membership - step;
                entry.membership = if faded <= FADED_OUT { 0.0 } else { faded };
            }
            entry.target || entry.membership > 0.0
        });
        self.members.clear();
        for (key, entry) in &self.entries {
            let Some((mapping, look)) = resolve(key.link, entry.lag) else {
                continue;
            };
            self.members.push(GhostMember {
                link: key.link,
                entity: key.entity,
                category: entry.category,
                membership: entry.membership,
                frozen: !entry.target,
                mapping,
                source: entry.source,
                mapped: mapped_transform(&mapping, &entry.source),
                look,
            });
        }
        self.members
            .sort_unstable_by_key(|member| (member.link, member.entity));
    }

    pub(crate) fn members(&self) -> &[GhostMember] {
        &self.members
    }
}

fn mapped_transform(mapping: &SeamlessMapping, source: &Transform) -> Transform {
    Transform {
        position: mapping.point(source.position),
        rotation: mapping.rotation() * source.rotation,
        scale: source.scale,
    }
}

#[cfg(test)]
#[path = "membership_tests.rs"]
mod tests;
