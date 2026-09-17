// Canonical id-34 load-derived indirection-word decode. This helper declares
// no resources; sampler and compose consumers supply their own carriers.

const SH_INDIRECTION_LEVEL_MASK: u32 = 0x00000003u;
const SH_INDIRECTION_VALID_BIT: u32 = 0x00000004u;
const SH_INDIRECTION_SCALE_MASK: u32 = 0x00000018u;
const SH_INDIRECTION_SCALE_SHIFT: u32 = 0x00000003u;
const SH_INDIRECTION_SLOT_SHIFT: u32 = 0x00000005u;

struct ShProbeIndirection {
    valid: bool,
    level: u32,
    scale: u32,
    slot: u32,
}

fn decode_sh_probe_indirection(word: u32) -> ShProbeIndirection {
    return ShProbeIndirection(
        (word & SH_INDIRECTION_VALID_BIT) != 0u,
        word & SH_INDIRECTION_LEVEL_MASK,
        (word & SH_INDIRECTION_SCALE_MASK) >> SH_INDIRECTION_SCALE_SHIFT,
        word >> SH_INDIRECTION_SLOT_SHIFT,
    );
}
