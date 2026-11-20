use postretro_level_format::prm::{PrmHeader, PrmReadError, PrmSlot, PrmSlots};

/// Raw source PNG bytes for every input a bundle can have, `None` where the
/// sibling is absent. Height, smoothness and tint are not wire slots: each
/// rides the specular slot, but each is a separately addressed input.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct BundleSources<'a> {
    pub diffuse: Option<&'a [u8]>,
    pub specular: Option<&'a [u8]>,
    pub normal: Option<&'a [u8]>,
    pub emissive: Option<&'a [u8]>,
    pub height: Option<&'a [u8]>,
    pub smoothness: Option<&'a [u8]>,
    pub tint: Option<&'a [u8]>,
}

impl<'a> BundleSources<'a> {
    /// Every input with its digest tag, in digest order. Input `i` owns bit
    /// `i` of the hashing mask and tag byte `i`: diffuse 0x00, specular 0x01,
    /// normal 0x02, emissive 0x03, height 0x04, smoothness 0x05, tint 0x06.
    /// The order is append-only.
    fn tagged(&self) -> [(u8, Option<&'a [u8]>); 7] {
        [
            (0x00, self.diffuse),
            (0x01, self.specular),
            (0x02, self.normal),
            (0x03, self.emissive),
            (0x04, self.height),
            (0x05, self.smoothness),
            (0x06, self.tint),
        ]
    }
}

/// Content address for a complete texture bundle.
///
/// The digest is a leading presence mask followed by, for each present input
/// in fixed order, a one-byte tag and that input's raw PNG bytes. The mask
/// leads so two bundles whose concatenated payloads coincide still differ —
/// the tag bytes alone do not delimit the payloads.
///
/// **Height, smoothness and tint are append-only.** When one is `None`
/// nothing at all is folded in: neither a mask bit nor a tag byte. Every
/// bundle authored before a sibling existed therefore hashes to exactly the
/// byte string it hashed to before, which is what keeps `baked/materials/`
/// from rebaking wholesale. This is a required property, not an accident.
/// Two tests pin it: `absent_siblings_keep_the_pre_sibling_bundle_hash`
/// replays the pre-surface-depth and pre-smoothness digests across every slot
/// combination, and
/// `bundle_without_smoothness_or_tint_is_a_cache_hit_and_is_not_rewritten`
/// proves sidecars written by older bakers survive a build byte-for-byte.
pub(super) fn bundle_hash_for(sources: &BundleSources) -> [u8; 32] {
    // Bits 4-6 of this *hashing* mask are not `PrmSlots` bits — height,
    // smoothness and tint are not wire slots, and `PrmSlots` bits 4-7 stay
    // reserved-and-rejected. Each bit is set only when its input is present,
    // so an absent input leaves the digest input byte-identical.
    let tagged = sources.tagged();
    let mask = tagged
        .iter()
        .enumerate()
        .filter(|(_, (_, bytes))| bytes.is_some())
        .fold(0u8, |mask, (bit, _)| mask | (1u8 << bit));
    let mut h = blake3::Hasher::new();
    h.update(&[mask]);
    for (tag, bytes) in tagged {
        if let Some(b) = bytes {
            h.update(&[tag]);
            h.update(b);
        }
    }
    *h.finalize().as_bytes()
}

/// `.prm` filename stem for a bundle.
///
/// Height, smoothness and tint participate exactly as they do in
/// [`bundle_hash_for`]: each counts toward the present-input tally and, alone,
/// gets its own tagged key. A bundle with none of them reaches the same arm —
/// and the same 32 bytes — it always did.
pub(super) fn filename_key_for(sources: &BundleSources) -> [u8; 32] {
    let mut present = sources
        .tagged()
        .into_iter()
        .filter_map(|(tag, bytes)| bytes.map(|bytes| (tag, bytes)));
    match (present.next(), present.next()) {
        (None, _) => [0u8; 32],
        // Diffuse-only keeps its historical key because model loading derives
        // that key at runtime.
        (Some((0x00, diffuse)), None) => *blake3::hash(diffuse).as_bytes(),
        // Any other single input still bakes a slot (height, smoothness or
        // tint alone bakes a specular slot with R = 0), so it needs an address
        // of its own rather than the all-absent zero key. The tag keeps two
        // single-input bundles apart when their bytes coincide.
        (Some((tag, bytes)), None) => {
            let mut h = blake3::Hasher::new();
            h.update(&[tag]);
            h.update(bytes);
            *h.finalize().as_bytes()
        }
        // Rich bundles need complete-content addressing so one material cannot
        // overwrite another that happens to share a diffuse. A diffuse plus a
        // height, smoothness or tint sibling is a *different* bundle from that
        // diffuse alone, so it correctly leaves the shared diffuse-only address.
        (Some(_), Some(_)) => bundle_hash_for(sources),
    }
}

/// The `slot_mask` bit each wire-order slot index declares itself with.
pub(super) const SLOT_MASK_BITS: [PrmSlots; 4] = [
    PrmSlots::DIFFUSE,
    PrmSlots::SPECULAR,
    PrmSlots::NORMAL,
    PrmSlots::EMISSIVE,
];

pub(super) fn cache_entry_has_valid_declared_slots(
    header: &PrmHeader,
    slots: &[Result<PrmSlot, PrmReadError>; 4],
) -> bool {
    SLOT_MASK_BITS
        .iter()
        .enumerate()
        .all(|(index, slot)| !header.slot_mask.contains(*slot) || slots[index].is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::prm::cache_filename_for_key;

    const DIFFUSE: &[u8] = b"diffuse-png";
    /// One byte string fed to every sibling, so only tags and mask bits can
    /// keep their keys apart.
    const SHARED: &[u8] = b"shared-sibling-png";

    /// A bundle holding one input, picked by its digest order.
    fn single(input: u8, bytes: &[u8]) -> BundleSources<'_> {
        let mut sources = BundleSources::default();
        let field = match input {
            0 => &mut sources.diffuse,
            1 => &mut sources.specular,
            2 => &mut sources.normal,
            3 => &mut sources.emissive,
            4 => &mut sources.height,
            5 => &mut sources.smoothness,
            6 => &mut sources.tint,
            _ => unreachable!("seven bundle inputs"),
        };
        *field = Some(bytes);
        sources
    }

    fn tagged_key(tag: u8, bytes: &[u8]) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(&[tag]);
        h.update(bytes);
        *h.finalize().as_bytes()
    }

    #[test]
    fn smoothness_and_tint_fold_in_at_mask_bits_five_and_six() {
        let sources = BundleSources {
            diffuse: Some(DIFFUSE),
            smoothness: Some(b"p"),
            tint: Some(b"t"),
            ..Default::default()
        };
        let mut h = blake3::Hasher::new();
        h.update(&[0b110_0001]);
        h.update(&[0x00]);
        h.update(DIFFUSE);
        h.update(&[0x05]);
        h.update(b"p");
        h.update(&[0x06]);
        h.update(b"t");
        assert_eq!(bundle_hash_for(&sources), *h.finalize().as_bytes());
        assert_eq!(filename_key_for(&sources), bundle_hash_for(&sources));
    }

    #[test]
    fn smoothness_or_tint_alone_bakes_to_its_own_nonzero_tagged_key() {
        let smoothness = filename_key_for(&single(5, SHARED));
        let tint = filename_key_for(&single(6, SHARED));
        assert_ne!(
            smoothness, [0u8; 32],
            "a `_p`-only bundle is not 'no source PNG'"
        );
        assert_ne!(tint, [0u8; 32], "a `_t`-only bundle is not 'no source PNG'");
        assert_eq!(smoothness, tagged_key(0x05, SHARED));
        assert_eq!(tint, tagged_key(0x06, SHARED));
        assert_ne!(smoothness, tint, "`_p` and `_t` from the same bytes");

        // Every single-input key from the same bytes is distinct, so no
        // sibling-only bundle overwrites another's sidecar.
        let keys: Vec<[u8; 32]> = (0..7)
            .map(|input| filename_key_for(&single(input, SHARED)))
            .collect();
        for (i, a) in keys.iter().enumerate() {
            for b in &keys[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn smoothness_and_tint_move_a_diffuse_off_every_other_bundle_address() {
        let with = |smoothness, tint| BundleSources {
            diffuse: Some(DIFFUSE),
            smoothness,
            tint,
            ..Default::default()
        };
        let bundles = [
            ("diffuse", with(None, None)),
            ("diffuse + _p", with(Some(SHARED), None)),
            ("diffuse + _t", with(None, Some(SHARED))),
            ("diffuse + _p + _t", with(Some(SHARED), Some(SHARED))),
            ("_p", single(5, SHARED)),
            ("_t", single(6, SHARED)),
        ];
        assert_eq!(
            filename_key_for(&bundles[0].1),
            *blake3::hash(DIFFUSE).as_bytes(),
            "diffuse alone keeps the address model loading derives"
        );
        // Distinct keys are distinct `.prm` filenames: none overwrites another.
        let filenames: Vec<(&str, String)> = bundles
            .iter()
            .map(|(label, sources)| (*label, cache_filename_for_key(&filename_key_for(sources))))
            .collect();
        for (i, (label_a, a)) in filenames.iter().enumerate() {
            for (label_b, b) in &filenames[i + 1..] {
                assert_ne!(a, b, "{label_a} and {label_b} share a sidecar");
            }
        }
    }
}
