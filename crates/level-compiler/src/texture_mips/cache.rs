use postretro_level_format::prm::{PrmHeader, PrmReadError, PrmSlot, PrmSlots};

pub(super) fn bundle_hash_for(
    diffuse: Option<&[u8]>,
    specular: Option<&[u8]>,
    normal: Option<&[u8]>,
    emissive: Option<&[u8]>,
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
    *h.finalize().as_bytes()
}

pub(super) fn filename_key_for(
    diffuse: Option<&[u8]>,
    specular: Option<&[u8]>,
    normal: Option<&[u8]>,
    emissive: Option<&[u8]>,
) -> [u8; 32] {
    match (diffuse, specular, normal, emissive) {
        (Some(d), _, _, _) => *blake3::hash(d).as_bytes(),
        (None, Some(s), _, _) => {
            let mut h = blake3::Hasher::new();
            h.update(&[0x01]);
            h.update(s);
            *h.finalize().as_bytes()
        }
        (None, None, Some(n), _) => {
            let mut h = blake3::Hasher::new();
            h.update(&[0x02]);
            h.update(n);
            *h.finalize().as_bytes()
        }
        (None, None, None, Some(e)) => {
            let mut h = blake3::Hasher::new();
            h.update(&[0x03]);
            h.update(e);
            *h.finalize().as_bytes()
        }
        (None, None, None, None) => [0u8; 32],
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
