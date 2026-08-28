// PRM texture payload sizing, mip slicing, and slot normalization.
// See: context/lib/rendering_pipeline.md §3

use postretro_level_format::prm::{PrmFormat, PrmReadError, PrmSlot, PrmSlots};

pub fn level_byte_size(format: PrmFormat, w: u32, h: u32) -> usize {
    match format {
        PrmFormat::Rgba8Unorm | PrmFormat::Rgba8UnormSrgb => (4 * w * h) as usize,
        PrmFormat::R8Unorm => (w * h) as usize,
        PrmFormat::Bc5RgUnorm => (w.div_ceil(4) * h.div_ceil(4) * 16) as usize,
    }
}

/// Splits one layer's mip chain for a `texture_2d` upload.
///
/// Call only after the PRM header establishes `layer_count == 1`. Layered
/// payloads are layer-major; `texture_2d_array` consumers must split every layer.
pub fn slot_levels(slot: &PrmSlot) -> Vec<(u32, u32, &[u8])> {
    let format = slot.format;
    debug_assert_eq!(
        slot.payload.len(),
        (0..slot.level_count)
            .map(|n| {
                let w = ((slot.width as u32) >> n).max(1);
                let h = ((slot.height as u32) >> n).max(1);
                level_byte_size(format, w, h)
            })
            .sum::<usize>(),
        "slot payload length must equal the sum of per-level byte sizes across all {} mip \
         levels (width={}, height={}, format={:?}); uncompressed levels are bpp*w*h, BC5 \
         levels are ceil(w/4)*ceil(h/4)*16 — in-process-constructed slots must match the \
         pyramid implied by width/height/level_count",
        slot.level_count,
        slot.width,
        slot.height,
        format,
    );
    let mut out = Vec::with_capacity(slot.level_count as usize);
    let mut offset = 0usize;
    for n in 0..slot.level_count {
        let w = ((slot.width as u32) >> n).max(1);
        let h = ((slot.height as u32) >> n).max(1);
        let size = level_byte_size(format, w, h);
        out.push((w, h, &slot.payload[offset..offset + size]));
        offset += size;
    }
    out
}

/// Maximum mip levels across all four slots. Takes the max (not diffuse-only)
/// so a corrupted diffuse with intact siblings doesn't clamp those siblings to
/// LOD 0. Defaults to 1 when no slot parses cleanly — disables mip filtering
/// rather than clamping to a wrong level. Used to key the sampler pool
/// (`Renderer::mip_count_aniso_samplers`).
pub fn header_mip_count(slots: &[Result<PrmSlot, PrmReadError>; 4]) -> u32 {
    slots
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|s| s.level_count as u32)
        .max()
        .unwrap_or(1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureSlotPolicy {
    WorldBundle,
    ModelDiffuseOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureSlotPlan {
    pub consume: [bool; 4],
    pub mip_count: u32,
}

pub fn texture_slot_plan(
    header_slots: PrmSlots,
    slots: &[Result<PrmSlot, PrmReadError>; 4],
    policy: TextureSlotPolicy,
) -> TextureSlotPlan {
    match policy {
        TextureSlotPolicy::WorldBundle => TextureSlotPlan {
            consume: [true, true, true, true],
            mip_count: header_mip_count(slots),
        },
        TextureSlotPolicy::ModelDiffuseOnly => TextureSlotPlan {
            consume: [
                header_slots.contains(PrmSlots::DIFFUSE),
                false,
                false,
                false,
            ],
            mip_count: slots[0]
                .as_ref()
                .map(|slot| slot.level_count as u32)
                .unwrap_or(1),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::prm::{PrmFile, PrmHeader, STAGE_VERSION};

    fn make_diffuse_only_prm(width: u16, height: u16) -> Vec<u8> {
        let level_count = {
            let m = width.max(height).max(1) as u32;
            (m.ilog2() + 1) as u8
        };
        let mut payload: Vec<u8> = Vec::new();
        for n in 0..level_count {
            let w = ((width as u32) >> n).max(1);
            let h = ((height as u32) >> n).max(1);
            let bytes = (4 * w * h) as usize;
            payload.extend((0..bytes).map(|i| ((i as u16 + n as u16 * 7) & 0xFF) as u8));
        }
        let slot = PrmSlot {
            format: PrmFormat::Rgba8UnormSrgb,
            width,
            height,
            level_count,
            payload,
        };
        let file = PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask: PrmSlots::DIFFUSE,
                bundle_hash: [0u8; 32],
                total_body_bytes: 0,
                layer_count: 1,
            },
            slots: [Some(slot), None, None, None],
        };
        file.to_bytes().expect("diffuse-only .prm serializes")
    }

    #[test]
    fn diffuse_only_prm_parses_to_single_slot() {
        let bytes = make_diffuse_only_prm(4, 4);
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        let header = header.expect("header parses");
        assert_eq!(header.slot_mask, PrmSlots::DIFFUSE);
        assert!(slots[0].is_ok());
        assert!(matches!(&slots[1], Err(PrmReadError::NotPresent)));
        assert!(matches!(&slots[2], Err(PrmReadError::NotPresent)));
        assert_eq!(header_mip_count(&slots), 3);
    }

    #[test]
    fn model_slot_plan_consumes_only_diffuse_from_richer_world_bundle() {
        let file = PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask: PrmSlots::DIFFUSE
                    | PrmSlots::SPECULAR
                    | PrmSlots::NORMAL
                    | PrmSlots::EMISSIVE,
                bundle_hash: [0u8; 32],
                total_body_bytes: 0,
                layer_count: 1,
            },
            slots: [
                Some(PrmSlot {
                    format: PrmFormat::Rgba8UnormSrgb,
                    width: 4,
                    height: 4,
                    level_count: 3,
                    payload: vec![0u8; 84],
                }),
                Some(PrmSlot {
                    format: PrmFormat::R8Unorm,
                    width: 1,
                    height: 1,
                    level_count: 1,
                    payload: vec![255],
                }),
                Some(PrmSlot {
                    format: PrmFormat::Rgba8Unorm,
                    width: 1,
                    height: 1,
                    level_count: 1,
                    payload: vec![127, 127, 255, 255],
                }),
                Some(PrmSlot {
                    format: PrmFormat::Rgba8UnormSrgb,
                    width: 1,
                    height: 1,
                    level_count: 1,
                    payload: vec![0, 0, 0, 255],
                }),
            ],
        };
        let bytes = file.to_bytes().unwrap();
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        let plan = texture_slot_plan(
            header.unwrap().slot_mask,
            &slots,
            TextureSlotPolicy::ModelDiffuseOnly,
        );
        assert_eq!(plan.consume, [true, false, false, false]);
        assert_eq!(plan.mip_count, 3);
    }

    #[test]
    fn slot_levels_walks_pyramid_in_order() {
        let bytes = make_diffuse_only_prm(4, 4);
        let (_, slots) = PrmFile::from_bytes_partial(&bytes);
        let levels = slot_levels(slots[0].as_ref().expect("diffuse parses"));
        assert_eq!(levels.len(), 3);
        assert_eq!((levels[0].0, levels[0].1, levels[0].2.len()), (4, 4, 64));
        assert_eq!((levels[1].0, levels[1].1, levels[1].2.len()), (2, 2, 16));
        assert_eq!((levels[2].0, levels[2].1, levels[2].2.len()), (1, 1, 4));
    }

    #[test]
    fn slot_levels_splits_bc5_into_block_aligned_levels() {
        use postretro_level_format::prm::bc5_level_count;

        let width: u16 = 8;
        let height: u16 = 8;
        let level_count = bc5_level_count(width, height);
        assert_eq!(level_count, 2);

        let slot = PrmSlot {
            format: PrmFormat::Bc5RgUnorm,
            width,
            height,
            level_count,
            payload: vec![0u8; 64 + 16],
        };

        let levels = slot_levels(&slot);
        assert_eq!(levels.len(), 2);
        assert_eq!((levels[0].0, levels[0].1, levels[0].2.len()), (8, 8, 64));
        assert_eq!((levels[1].0, levels[1].1, levels[1].2.len()), (4, 4, 16));
    }
}
