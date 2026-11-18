// Id 42's per-face owner table: each face's owner lights and its lightmap-to-
// mask texel offset, and the per-vertex form the loader stamps from it.
// Governing context: context/lib/build_pipeline.md §PRL section IDs

use crate::lightmap::invalid;

/// An empty channel in a face's owner entry.
pub const SHADOWMASK_OWNER_EMPTY: u16 = 0xFFFF;

/// Owner channels per face: one per mask channel, two per BC5 group.
pub const SHADOWMASK_OWNER_CHANNELS: usize = 4;

/// A face's owners: compact static spec-light indices (the `!is_dynamic`
/// order `pack_spec_lights` and id 23 use), ascending, empties trailing.
/// Position `s` is channel `s`: group `s / 2`, BC5 channel `s % 2`.
pub type FaceOwners = [u16; SHADOWMASK_OWNER_CHANNELS];

/// A face no light owns.
pub const NO_FACE_OWNERS: FaceOwners = [SHADOWMASK_OWNER_EMPTY; SHADOWMASK_OWNER_CHANNELS];

/// A face's texel offset from its lightmap block to its mask block, x then y:
/// mask texel = lightmap texel + offset. Zero for a face with no owner.
pub type MaskOffset = [i16; 2];

/// Four u16 owners, then i16 dx, i16 dy.
pub(super) const OWNER_ENTRY_BYTES: usize = 2 * SHADOWMASK_OWNER_CHANNELS + 4;

/// What the loader stamps onto each vertex of a face: its owners and the
/// offset its mask texels sit at. The forward pipeline's owner stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecOwnerVertex {
    pub owners: FaceOwners,
    pub mask_offset: MaskOffset,
}

impl SpecOwnerVertex {
    /// A vertex no light owns: it samples no mask.
    pub const EMPTY: Self = Self {
        owners: NO_FACE_OWNERS,
        mask_offset: [0; 2],
    };
}

/// One owner entry from its 12 wire bytes.
pub(super) fn read_owner_entry(entry: &[u8; OWNER_ENTRY_BYTES]) -> (FaceOwners, MaskOffset) {
    let u16_at = |at: usize| u16::from_le_bytes([entry[at], entry[at + 1]]);
    let owners = std::array::from_fn(|s| u16_at(2 * s));
    let base = 2 * SHADOWMASK_OWNER_CHANNELS;
    (owners, [u16_at(base) as i16, u16_at(base + 2) as i16])
}

pub(super) fn write_owner_entry(out: &mut Vec<u8>, owners: &FaceOwners, offset: MaskOffset) {
    for owner in owners {
        out.extend_from_slice(&owner.to_le_bytes());
    }
    for delta in offset {
        out.extend_from_slice(&delta.to_le_bytes());
    }
}

/// Rejects an owner at or past `static_spec_light_count`, a duplicate owner,
/// owners out of ascending order, and an owner after an empty channel.
pub fn validate_face_owners(
    face: usize,
    owners: &FaceOwners,
    static_spec_light_count: u32,
) -> crate::Result<()> {
    let mut previous: Option<u16> = None;
    let mut ended = false;
    for &owner in owners {
        if owner == SHADOWMASK_OWNER_EMPTY {
            ended = true;
            continue;
        }
        if ended {
            return Err(invalid(format!(
                "shadowmask face {face} has owner {owner} after an empty channel"
            )));
        }
        if u32::from(owner) >= static_spec_light_count {
            return Err(invalid(format!(
                "shadowmask face {face} owner {owner} is not below the static spec-light count \
                 {static_spec_light_count}"
            )));
        }
        match previous {
            Some(p) if p == owner => {
                return Err(invalid(format!(
                    "shadowmask face {face} lists owner {owner} twice"
                )));
            }
            Some(p) if p > owner => {
                return Err(invalid(format!(
                    "shadowmask face {face} owners {owners:?} are not ascending"
                )));
            }
            _ => {}
        }
        previous = Some(owner);
    }
    Ok(())
}

/// Rejects a mask offset on a face with no owner: such a face samples no mask,
/// so a nonzero offset could only be a writer fault.
pub(super) fn validate_mask_offset(
    face: usize,
    owners: &FaceOwners,
    offset: MaskOffset,
) -> crate::Result<()> {
    if *owners == NO_FACE_OWNERS && offset != [0; 2] {
        return Err(invalid(format!(
            "shadowmask face {face} has no owner but a mask offset {offset:?}; an unowned face's \
             offset is 0"
        )));
    }
    Ok(())
}
