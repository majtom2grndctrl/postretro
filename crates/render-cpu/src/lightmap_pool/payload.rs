// Whether a block's read payload can fill its pool places: the extent and format
// of every plane, and which mask planes it stores.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap::{LightmapBlockPayload, LightmapHeader};
use postretro_level_format::shadowmask_atlas::group_plane_len;

use super::BlockMasks;

/// Why `payload` cannot fill a `width × height` block, or `None` when every
/// plane the pools keep matches the block. `masks` is `None` for a level with
/// no mask pool, whose payload's masks go nowhere.
pub fn payload_mismatch(
    header: &LightmapHeader,
    (width, height): (u32, u32),
    masks: Option<BlockMasks>,
    payload: &LightmapBlockPayload,
) -> Option<String> {
    let irradiance = header.irradiance_len(width, height);
    let direction = header.direction_len(width, height);
    if irradiance != Some(payload.irradiance.len() as u64)
        || direction != Some(payload.direction.len() as u64)
    {
        return Some(format!(
            "{width}x{height} block carries {} irradiance and {} direction byte(s), expected \
             {irradiance:?} and {direction:?}",
            payload.irradiance.len(),
            payload.direction.len(),
        ));
    }
    masks.and_then(|masks| planes_mismatch(masks, payload))
}

/// Why `payload`'s mask planes disagree with the block's plane presence: a
/// stored plane must fill its own extent, and an absent one must be empty.
pub(super) fn planes_mismatch(masks: BlockMasks, payload: &LightmapBlockPayload) -> Option<String> {
    let Some(groups) = &payload.shadowmask else {
        return Some("arrived without its groups".to_string());
    };
    let BlockMasks { planes, extents } = masks;
    let expected: [Option<u64>; 2] = std::array::from_fn(|plane| match planes[plane] {
        true => group_plane_len(extents[plane].0, extents[plane].1),
        false => Some(0),
    });
    (expected != [Some(groups[0].len() as u64), Some(groups[1].len() as u64)]).then(|| {
        format!(
            "groups are {} and {} byte(s), expected {expected:?} for plane presence {planes:?} \
             at plane extents {extents:?}",
            groups[0].len(),
            groups[1].len(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, LightmapMode};

    const EXTENT: (u32, u32) = (8, 8);

    fn header() -> LightmapHeader {
        LightmapHeader {
            block_count: 1,
            direction_texel_scale: 2,
            irradiance_format: IRRADIANCE_FORMAT_BC6H,
            mode: LightmapMode::Shadowed,
        }
    }

    fn payload(a: bool, b: bool) -> LightmapBlockPayload {
        let header = header();
        let len = group_plane_len(EXTENT.0, EXTENT.1).unwrap() as usize;
        let plane = |stored: bool| vec![7; if stored { len } else { 0 }];
        LightmapBlockPayload {
            irradiance: vec![0; header.irradiance_len(EXTENT.0, EXTENT.1).unwrap() as usize],
            direction: vec![0; header.direction_len(EXTENT.0, EXTENT.1).unwrap() as usize],
            shadowmask: Some([plane(a), plane(b)]),
        }
    }

    fn whole_block(planes: [bool; 2]) -> BlockMasks {
        BlockMasks {
            planes,
            extents: [EXTENT; 2],
        }
    }

    #[test]
    fn payload_disagreeing_with_presence_fails_whole() {
        let mismatch = |planes, payload: &LightmapBlockPayload| {
            payload_mismatch(&header(), EXTENT, Some(whole_block(planes)), payload)
        };
        for (a, b) in [(true, true), (true, false), (false, true), (false, false)] {
            assert_eq!(mismatch([a, b], &payload(a, b)), None, "{a} {b}");
            for (other_a, other_b) in [(!a, b), (a, !b), (!a, !b)] {
                let reason = mismatch([a, b], &payload(other_a, other_b)).expect("disagrees");
                assert!(reason.contains("plane presence"), "{reason}");
            }
        }

        let mut short = payload(true, true);
        short.shadowmask.as_mut().unwrap()[1].pop();
        assert!(mismatch([true, true], &short).is_some(), "a short plane");
        let mut none = payload(false, false);
        none.shadowmask = None;
        assert!(
            mismatch([false, false], &none).is_some(),
            "no planes at all"
        );
    }

    // A plane fills its own extent, not its lightmap block's: plane B at a
    // 4x4 sub-rect of an 8x4 mask block stores one BC5 block.
    #[test]
    fn each_plane_is_checked_at_its_own_extent() {
        let masks = BlockMasks {
            planes: [true, true],
            extents: [(8, 4), (4, 4)],
        };
        let mut payload = payload(true, true);
        payload.shadowmask = Some([vec![1; 32], vec![2; 16]]);
        assert_eq!(
            payload_mismatch(&header(), EXTENT, Some(masks), &payload),
            None
        );
        payload.shadowmask = Some([vec![1; 32], vec![2; 32]]);
        let reason = payload_mismatch(&header(), EXTENT, Some(masks), &payload).expect("disagrees");
        assert!(reason.contains("plane extents"), "{reason}");
    }

    #[test]
    fn a_level_with_no_mask_pool_reads_past_a_payloads_masks() {
        assert_eq!(
            payload_mismatch(&header(), EXTENT, None, &payload(true, false)),
            None
        );
    }
}
