use postretro_level_format::prm::{PrmHeader, PrmReadError, PrmSlot, PrmSlots};

/// Content address for a complete texture bundle.
///
/// The digest is a leading presence mask followed by, for each present input
/// in fixed order, a one-byte tag and that input's raw PNG bytes. The mask
/// leads so two bundles whose concatenated payloads coincide still differ —
/// the tag bytes alone do not delimit the payloads.
///
/// **`height` is append-only.** When it is `None` nothing at all is folded in:
/// neither a mask bit nor a tag byte. Every bundle authored before the height
/// sibling existed therefore hashes to exactly the byte string it hashed to
/// before, which is what keeps `baked/materials/` from rebaking wholesale.
/// This is a required property, not an accident. Two tests pin it:
/// `height_absent_bundle_hash_matches_the_pre_height_algorithm` replays the
/// pre-surface-depth digest across every slot combination, and
/// `heightless_bundle_is_a_cache_hit_and_is_not_rewritten` proves a sidecar
/// written by the old baker survives a build byte-for-byte.
pub(super) fn bundle_hash_for(
    diffuse: Option<&[u8]>,
    specular: Option<&[u8]>,
    normal: Option<&[u8]>,
    emissive: Option<&[u8]>,
    height: Option<&[u8]>,
) -> [u8; 32] {
    let mut mask: u8 = 0;
    if diffuse.is_some() {
        mask |= 0b001;
    }
    if specular.is_some() {
        mask |= 0b010;
    }
    if normal.is_some() {
        mask |= 0b100;
    }
    if emissive.is_some() {
        mask |= 0b1000;
    }
    if height.is_some() {
        // Bit 4 of this *hashing* mask. It is not a `PrmSlots` bit — height is
        // not a wire slot, and `PrmSlots` bits 4-7 stay reserved-and-rejected.
        // The bit is set only when height is present, so an absent height
        // leaves the digest input byte-identical.
        mask |= 0b1_0000;
    }
    let mut h = blake3::Hasher::new();
    h.update(&[mask]);
    if let Some(b) = diffuse {
        h.update(&[0x00]);
        h.update(b);
    }
    if let Some(b) = specular {
        h.update(&[0x01]);
        h.update(b);
    }
    if let Some(b) = normal {
        h.update(&[0x02]);
        h.update(b);
    }
    if let Some(b) = emissive {
        h.update(&[0x03]);
        h.update(b);
    }
    if let Some(b) = height {
        h.update(&[0x04]);
        h.update(b);
    }
    *h.finalize().as_bytes()
}

/// `.prm` filename stem for a bundle.
///
/// Height participates exactly as it does in [`bundle_hash_for`]: it counts
/// toward the present-input tally and, alone, gets its own tagged key. A
/// bundle with no height sibling reaches the same arm — and the same 32 bytes
/// — it always did.
pub(super) fn filename_key_for(
    diffuse: Option<&[u8]>,
    specular: Option<&[u8]>,
    normal: Option<&[u8]>,
    emissive: Option<&[u8]>,
    height: Option<&[u8]>,
) -> [u8; 32] {
    let present_count = [diffuse, specular, normal, emissive, height]
        .iter()
        .filter(|slot| slot.is_some())
        .count();
    // Rich bundles need complete-content addressing so one material cannot
    // overwrite another that happens to share a diffuse. Diffuse-only keeps
    // its historical key because model loading derives that key at runtime.
    // A diffuse plus a height sibling is a *different* bundle from that
    // diffuse alone, so it correctly leaves the shared diffuse-only address.
    if present_count > 1 {
        return bundle_hash_for(diffuse, specular, normal, emissive, height);
    }

    match (diffuse, specular, normal, emissive, height) {
        (Some(d), None, None, None, None) => *blake3::hash(d).as_bytes(),
        (None, Some(s), None, None, None) => {
            let mut h = blake3::Hasher::new();
            h.update(&[0x01]);
            h.update(s);
            *h.finalize().as_bytes()
        }
        (None, None, Some(n), None, None) => {
            let mut h = blake3::Hasher::new();
            h.update(&[0x02]);
            h.update(n);
            *h.finalize().as_bytes()
        }
        (None, None, None, Some(e), None) => {
            let mut h = blake3::Hasher::new();
            h.update(&[0x03]);
            h.update(e);
            *h.finalize().as_bytes()
        }
        // Height alone still bakes a specular slot (R = 0, G = depth), so it
        // needs an address of its own rather than the all-absent zero key.
        (None, None, None, None, Some(hh)) => {
            let mut h = blake3::Hasher::new();
            h.update(&[0x04]);
            h.update(hh);
            *h.finalize().as_bytes()
        }
        (None, None, None, None, None) => [0u8; 32],
        _ => unreachable!("multi-input bundles return before the single-input match"),
    }
}

pub(super) fn cache_entry_has_valid_declared_slots(
    header: &PrmHeader,
    slots: &[Result<PrmSlot, PrmReadError>; 4],
) -> bool {
    [
        PrmSlots::DIFFUSE,
        PrmSlots::SPECULAR,
        PrmSlots::NORMAL,
        PrmSlots::EMISSIVE,
    ]
    .iter()
    .enumerate()
    .all(|(index, slot)| !header.slot_mask.contains(*slot) || slots[index].is_ok())
}
