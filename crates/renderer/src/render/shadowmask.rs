// Renderer-side CPU packing for static-light shadowmask world receipt.
// Governing context: context/lib/rendering_pipeline.md

use super::renderer_types::{PromotedShadowPoolKind, PromotedStaticLightRecord};
use postretro_level_format::shadowmask_atlas::SHADOWMASK_CHANNEL_DROPPED;
use postretro_level_loader::MapLight;

pub(crate) const FORWARD_SHADOWMASK_META_VEC4S_PER_RECORD: usize = 2;
pub(crate) const FORWARD_SHADOWMASK_METADATA_BYTES_PER_RECORD: usize =
    FORWARD_SHADOWMASK_META_VEC4S_PER_RECORD * 16;
pub(crate) const FORWARD_SHADOWMASK_INVALID_INDEX: u32 = u32::MAX;
// Metadata is appended to `array<vec4<f32>>`, so sentinels must be ordinary
// numeric floats rather than raw integer bit patterns.
const FORWARD_SHADOWMASK_INVALID_INDEX_VALUE: f32 = -1.0;
const FORWARD_SHADOWMASK_DROPPED_CHANNEL_VALUE: f32 = 4.0;

pub(crate) fn influence_capacity_with_shadowmask_metadata(
    dynamic_light_count: usize,
    selected_static_count: usize,
) -> usize {
    (dynamic_light_count
        + selected_static_count
        + selected_static_count * FORWARD_SHADOWMASK_META_VEC4S_PER_RECORD)
        .max(1)
}

/// Build `selection_index -> spec_lights_index` for the compact baked-tier
/// `spec_lights` buffer. `spec_lights` skips dynamic-tier lights, so a full
/// AlphaLights/global index must never be used directly.
pub(crate) fn build_selection_spec_light_indices(
    lights: &[MapLight],
    entity_shadow_lights: &[u32],
) -> Vec<u32> {
    entity_shadow_lights
        .iter()
        .map(|&global_index| {
            spec_light_index_for_global_light(lights, global_index as usize)
                .unwrap_or(FORWARD_SHADOWMASK_INVALID_INDEX)
        })
        .collect()
}

fn spec_light_index_for_global_light(lights: &[MapLight], global_index: usize) -> Option<u32> {
    let light = lights.get(global_index)?;
    if light.is_dynamic {
        return None;
    }
    Some(
        lights[..global_index]
            .iter()
            .filter(|light| !light.is_dynamic)
            .count() as u32,
    )
}

pub(crate) fn pack_forward_shadowmask_metadata(
    records: &[PromotedStaticLightRecord],
    selection_spec_light_indices: &[u32],
    channels: &[u8],
    shadowmask_present: bool,
    out: &mut Vec<u8>,
) {
    out.clear();
    out.reserve(records.len() * FORWARD_SHADOWMASK_METADATA_BYTES_PER_RECORD);

    for record in records {
        let selection_index = record.selection_index as usize;
        let spec_index = selection_spec_light_indices
            .get(selection_index)
            .copied()
            .unwrap_or(FORWARD_SHADOWMASK_INVALID_INDEX);
        let channel = if shadowmask_present {
            channels
                .get(selection_index)
                .copied()
                .unwrap_or(SHADOWMASK_CHANNEL_DROPPED)
        } else {
            SHADOWMASK_CHANNEL_DROPPED
        };
        let pool_kind = match record.pool_kind {
            PromotedShadowPoolKind::Spot => 0u32,
            PromotedShadowPoolKind::Cube => 1u32,
        };

        push_f32(out, record.global_light_index as f32);
        push_f32(out, record.selection_index as f32);
        push_f32(out, metadata_index_value(spec_index));
        push_f32(out, record.weight.clamp(0.0, 1.0));

        push_f32(out, pool_kind as f32);
        push_f32(out, record.slot as f32);
        push_f32(out, metadata_channel_value(channel));
        push_f32(out, 0.0);
    }
}

fn metadata_index_value(index: u32) -> f32 {
    if index == FORWARD_SHADOWMASK_INVALID_INDEX {
        FORWARD_SHADOWMASK_INVALID_INDEX_VALUE
    } else {
        index as f32
    }
}

fn metadata_channel_value(channel: u8) -> f32 {
    if channel == SHADOWMASK_CHANNEL_DROPPED {
        FORWARD_SHADOWMASK_DROPPED_CHANNEL_VALUE
    } else {
        channel as f32
    }
}

fn push_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_ne_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_loader::{FalloffModel, LightType, ShadowType};

    fn light(is_dynamic: bool) -> MapLight {
        MapLight {
            origin: [0.0; 3],
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0; 3],
            falloff_model: FalloffModel::Linear,
            falloff_range: 8.0,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0, 0.0, 1.0],
            is_dynamic,
            casts_entity_shadows: is_dynamic,
            animated_slot: None,
            tags: Vec::new(),
            cell_index: 0,
            shadow_type: ShadowType::StaticLightMap,
        }
    }

    fn read_f32(bytes: &[u8], offset: usize) -> f32 {
        f32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    fn selection_spec_indices_count_prior_non_dynamic_lights() {
        let lights = vec![light(false), light(true), light(false), light(false)];
        let indices = build_selection_spec_light_indices(&lights, &[2, 3, 1]);

        assert_eq!(
            indices,
            vec![1, 2, FORWARD_SHADOWMASK_INVALID_INDEX],
            "selection indices must map through compact baked-tier order, never raw global index"
        );
    }

    #[test]
    fn dropped_channel_is_uploaded_without_suppressing_promoted_record() {
        let records = [PromotedStaticLightRecord {
            global_light_index: 7,
            selection_index: 0,
            pool_kind: PromotedShadowPoolKind::Spot,
            slot: 3,
            weight: 0.5,
        }];
        let mut bytes = Vec::new();

        pack_forward_shadowmask_metadata(
            &records,
            &[2],
            &[SHADOWMASK_CHANNEL_DROPPED],
            true,
            &mut bytes,
        );

        assert_eq!(bytes.len(), FORWARD_SHADOWMASK_METADATA_BYTES_PER_RECORD);
        assert_eq!(read_f32(&bytes, 0), 7.0);
        assert_eq!(read_f32(&bytes, 4), 0.0);
        assert_eq!(read_f32(&bytes, 8), 2.0);
        assert!((read_f32(&bytes, 12) - 0.5).abs() < 1.0e-6);
        assert_eq!(read_f32(&bytes, 16), 0.0);
        assert_eq!(read_f32(&bytes, 20), 3.0);
        assert_eq!(
            read_f32(&bytes, 24),
            FORWARD_SHADOWMASK_DROPPED_CHANNEL_VALUE
        );
    }

    #[test]
    fn absent_shadowmask_forces_dropped_channel_only() {
        let records = [PromotedStaticLightRecord {
            global_light_index: 2,
            selection_index: 0,
            pool_kind: PromotedShadowPoolKind::Cube,
            slot: 1,
            weight: 1.0,
        }];
        let mut bytes = Vec::new();

        pack_forward_shadowmask_metadata(&records, &[0], &[0], false, &mut bytes);

        assert_eq!(read_f32(&bytes, 16), 1.0);
        assert_eq!(read_f32(&bytes, 20), 1.0);
        assert_eq!(
            read_f32(&bytes, 24),
            FORWARD_SHADOWMASK_DROPPED_CHANNEL_VALUE
        );
    }

    #[test]
    fn invalid_spec_index_is_uploaded_as_float_safe_sentinel() {
        let records = [PromotedStaticLightRecord {
            global_light_index: 2,
            selection_index: 4,
            pool_kind: PromotedShadowPoolKind::Spot,
            slot: 0,
            weight: 1.0,
        }];
        let mut bytes = Vec::new();

        pack_forward_shadowmask_metadata(&records, &[], &[0], true, &mut bytes);

        assert_eq!(read_f32(&bytes, 8), FORWARD_SHADOWMASK_INVALID_INDEX_VALUE);
        assert_ne!(
            u32::from_ne_bytes(bytes[8..12].try_into().unwrap()),
            FORWARD_SHADOWMASK_INVALID_INDEX,
            "invalid metadata must not depend on preserving raw u32 payload bits through a float load",
        );
    }

    #[test]
    fn no_promoted_records_produces_no_metadata() {
        let mut bytes = vec![1, 2, 3];
        pack_forward_shadowmask_metadata(&[], &[], &[], true, &mut bytes);
        assert!(bytes.is_empty());
    }
}
