// Whether a block's read payload can fill its pool places: the extent and format
// of every plane, and which mask planes it stores.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap::{LightmapBlockPayload, LightmapHeader};
use postretro_level_format::shadowmask_atlas::group_plane_len;

use super::PlanePresence;

/// Why `payload` cannot fill a `width × height` block, or `None` when every
/// plane the pools keep matches the block. `planes` is `None` for a level with
/// no mask pool, whose payload's masks go nowhere.
pub fn payload_mismatch(
    header: &LightmapHeader,
    (width, height): (u32, u32),
    planes: Option<PlanePresence>,
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
    planes.and_then(|planes| planes_mismatch((width, height), planes, payload))
}

/// Why `payload`'s mask planes disagree with the block's plane presence: a
/// stored plane must fill its extent, and an absent one must be empty.
pub(super) fn planes_mismatch(
    (width, height): (u32, u32),
    planes: PlanePresence,
    payload: &LightmapBlockPayload,
) -> Option<String> {
    let Some(groups) = &payload.shadowmask else {
        return Some("arrived without its groups".to_string());
    };
    let full = group_plane_len(width, height);
    let expected = planes.map(|stored| if stored { full } else { Some(0) });
    (expected != [Some(groups[0].len() as u64), Some(groups[1].len() as u64)]).then(|| {
        format!(
            "({width}x{height}) groups are {} and {} byte(s), expected {expected:?} for plane \
             presence {planes:?}",
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

    #[test]
    fn payload_disagreeing_with_presence_fails_whole() {
        let mismatch = |planes, payload: &LightmapBlockPayload| {
            payload_mismatch(&header(), EXTENT, Some(planes), payload)
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

    #[test]
    fn a_level_with_no_mask_pool_reads_past_a_payloads_masks() {
        assert_eq!(
            payload_mismatch(&header(), EXTENT, None, &payload(true, false)),
            None
        );
    }
}
