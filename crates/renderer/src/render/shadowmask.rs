// Renderer-side CPU packing for static-light shadowmask world receipt.
// Governing context: context/lib/rendering_pipeline.md

use super::renderer_types::{PromotedBakedLightRecord, PromotedShadowPoolKind};
use postretro_level_loader::MapLight;

pub(crate) const FORWARD_SHADOWMASK_META_VEC4S_PER_RECORD: usize = 2;
pub(crate) const FORWARD_SHADOWMASK_METADATA_BYTES_PER_RECORD: usize =
    FORWARD_SHADOWMASK_META_VEC4S_PER_RECORD * 16;
pub(crate) const FORWARD_SHADOWMASK_INVALID_INDEX: u32 = u32::MAX;
// Metadata is appended to `array<vec4<f32>>`, so sentinels must be ordinary
// numeric floats rather than raw integer bit patterns.
const FORWARD_SHADOWMASK_INVALID_INDEX_VALUE: f32 = -1.0;
/// The second vec4's `.z` lane, unread: a promoted light's mask channel is its
/// position among the receiving face's owners, found in the shader.
const FORWARD_SHADOWMASK_RESERVED_VALUE: f32 = -1.0;

pub(crate) fn influence_capacity_with_shadowmask_metadata(
    dynamic_light_count: usize,
    animated_baked_light_count: usize,
    selected_static_count: usize,
) -> usize {
    (dynamic_light_count
        + animated_baked_light_count
        + selected_static_count
        // The metadata tail has a raw section-45 prefix for cache-layer lookup,
        // followed by selected-static records with their shadowmask metadata.
        + (animated_baked_light_count + selected_static_count)
            * FORWARD_SHADOWMASK_META_VEC4S_PER_RECORD)
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

/// The promoted-light metadata tail after the influence records: two vec4s
/// per record, `(global index, selection index, compact spec index, weight)`
/// then `(pool kind, slot, reserved, cache layer)`. The union subtraction
/// reads a light's mask channel from the face's owners by its spec index.
pub(crate) fn pack_forward_shadowmask_metadata(
    animated_baked_light_count: usize,
    records: &[PromotedBakedLightRecord],
    cache_layers: &[i32],
    selection_spec_light_indices: &[u32],
    out: &mut Vec<u8>,
) {
    out.clear();
    out.reserve(
        (animated_baked_light_count + records.len()) * FORWARD_SHADOWMASK_METADATA_BYTES_PER_RECORD,
    );

    // Keep this prefix in raw AnimatedBakedLights order. The tail records are
    // already in that order, whereas MapLight animated_slot and descriptor
    // table indices are unrelated namespaces. An unpromoted row's -1 cache
    // layer is harmless because its forward shadow slot remains the sentinel.
    let mut animated_cache_layers = vec![-1i32; animated_baked_light_count];
    for (record_index, record) in records.iter().enumerate() {
        let Some(animated_baked_index) = record.source.animated_baked_index() else {
            continue;
        };
        if let Some(cache_layer) = animated_cache_layers.get_mut(animated_baked_index) {
            *cache_layer = cache_layers.get(record_index).copied().unwrap_or(-1);
        }
    }
    for (animated_index, cache_layer) in animated_cache_layers.into_iter().enumerate() {
        // Section-45 receivers consume only the second vec4's `.w` cache
        // layer. Keep the other lanes as ordinary sentinels; they must not
        // alias EntityShadowLights shadowmask/spec state.
        push_f32(out, -1.0);
        push_f32(out, animated_index as f32);
        push_f32(out, FORWARD_SHADOWMASK_INVALID_INDEX_VALUE);
        push_f32(out, 0.0);
        push_f32(out, -1.0);
        push_f32(out, -1.0);
        push_f32(out, FORWARD_SHADOWMASK_RESERVED_VALUE);
        push_f32(out, cache_layer as f32);
    }

    for (record_index, record) in records.iter().enumerate() {
        let Some(selection_index) = record.source.selected_static_index() else {
            continue;
        };
        let spec_index = selection_spec_light_indices
            .get(selection_index)
            .copied()
            .unwrap_or(FORWARD_SHADOWMASK_INVALID_INDEX);
        let pool_kind = match record.pool_kind {
            PromotedShadowPoolKind::Spot => 0u32,
            PromotedShadowPoolKind::Cube => 1u32,
        };

        push_f32(out, record.global_light_index as f32);
        push_f32(out, selection_index as f32);
        push_f32(out, metadata_index_value(spec_index));
        push_f32(out, record.weight.clamp(0.0, 1.0));

        push_f32(out, pool_kind as f32);
        push_f32(out, record.slot as f32);
        push_f32(out, FORWARD_SHADOWMASK_RESERVED_VALUE);
        push_f32(
            out,
            cache_layers.get(record_index).copied().unwrap_or(-1) as f32,
        );
    }
}

fn metadata_index_value(index: u32) -> f32 {
    if index == FORWARD_SHADOWMASK_INVALID_INDEX {
        FORWARD_SHADOWMASK_INVALID_INDEX_VALUE
    } else {
        index as f32
    }
}

fn push_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_ne_bytes());
}

#[cfg(test)]
mod tests {
    use super::super::renderer_types::PromotedBakedLightSource;
    use super::*;
    use postretro_level_loader::{FalloffModel, LightType, ShadowType};
    use postretro_lighting::spec_buffer::{SPEC_LIGHT_SIZE, pack_spec_lights};

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

    // Regression: frame capture used to pass a prefiltered static list with
    // selection indices still expressed against `world.lights`. A dynamic
    // light before a selected static light then made the metadata name the
    // wrong spec light, so the union looked up the wrong owner.
    #[test]
    fn dynamic_prefix_selected_static_maps_to_its_compact_spec_record() {
        let lights = vec![light(true), light(false)];
        let selection_spec_indices = build_selection_spec_light_indices(&lights, &[1]);
        assert_eq!(selection_spec_indices, vec![0]);
        let spec_bytes = pack_spec_lights(&lights);
        assert_eq!(
            spec_bytes.len(),
            SPEC_LIGHT_SIZE,
            "the selected light is spec record 0, the index face owners use"
        );
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
    fn promoted_record_uploads_its_spec_index_and_a_reserved_channel_lane() {
        let records = [PromotedBakedLightRecord {
            global_light_index: 7,
            pool_kind: PromotedShadowPoolKind::Spot,
            slot: 3,
            weight: 0.5,
            source: PromotedBakedLightSource::SelectedStatic { selection_index: 0 },
        }];
        let mut bytes = Vec::new();

        pack_forward_shadowmask_metadata(0, &records, &[6], &[2], &mut bytes);

        assert_eq!(bytes.len(), FORWARD_SHADOWMASK_METADATA_BYTES_PER_RECORD);
        assert_eq!(read_f32(&bytes, 0), 7.0);
        assert_eq!(read_f32(&bytes, 4), 0.0);
        assert_eq!(read_f32(&bytes, 8), 2.0);
        assert!((read_f32(&bytes, 12) - 0.5).abs() < 1.0e-6);
        assert_eq!(read_f32(&bytes, 16), 0.0);
        assert_eq!(read_f32(&bytes, 20), 3.0);
        assert_eq!(read_f32(&bytes, 24), FORWARD_SHADOWMASK_RESERVED_VALUE);
        assert_eq!(read_f32(&bytes, 28), 6.0);
    }

    #[test]
    fn invalid_spec_index_is_uploaded_as_float_safe_sentinel() {
        let records = [PromotedBakedLightRecord {
            global_light_index: 2,
            pool_kind: PromotedShadowPoolKind::Spot,
            slot: 0,
            weight: 1.0,
            source: PromotedBakedLightSource::SelectedStatic { selection_index: 4 },
        }];
        let mut bytes = Vec::new();

        pack_forward_shadowmask_metadata(0, &records, &[-1], &[], &mut bytes);

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
        pack_forward_shadowmask_metadata(0, &[], &[], &[], &mut bytes);
        assert!(bytes.is_empty());
    }

    #[test]
    fn animated_cache_metadata_uses_raw_roster_index_without_static_aliasing() {
        let records = [PromotedBakedLightRecord {
            global_light_index: 11,
            pool_kind: PromotedShadowPoolKind::Spot,
            slot: 5,
            weight: 0.75,
            source: PromotedBakedLightSource::AnimatedBaked {
                animated_baked_index: 2,
            },
        }];
        let mut bytes = Vec::new();

        pack_forward_shadowmask_metadata(4, &records, &[7], &[0], &mut bytes);

        assert_eq!(
            bytes.len(),
            4 * FORWARD_SHADOWMASK_METADATA_BYTES_PER_RECORD,
            "raw AnimatedBakedLights positions, including holes, own the metadata prefix"
        );
        let third_entry = 2 * FORWARD_SHADOWMASK_METADATA_BYTES_PER_RECORD;
        assert_eq!(
            read_f32(&bytes, third_entry + 28),
            7.0,
            "the cache layer follows AnimatedBakedLights index 2, not EntityShadowLights index 0"
        );
        assert_eq!(read_f32(&bytes, 28), -1.0);
        assert_eq!(
            read_f32(&bytes, FORWARD_SHADOWMASK_METADATA_BYTES_PER_RECORD + 28),
            -1.0
        );
    }

    // Regression: selected-static metadata used to be read from the animated
    // cache-prefix row when both promotion tiers coexisted.
    #[test]
    fn animated_and_selected_static_metadata_keep_disjoint_coexistence_regions() {
        let records = [
            PromotedBakedLightRecord {
                global_light_index: 11,
                pool_kind: PromotedShadowPoolKind::Spot,
                slot: 5,
                weight: 0.75,
                source: PromotedBakedLightSource::AnimatedBaked {
                    animated_baked_index: 1,
                },
            },
            PromotedBakedLightRecord {
                global_light_index: 19,
                pool_kind: PromotedShadowPoolKind::Cube,
                slot: 1,
                weight: 0.5,
                source: PromotedBakedLightSource::SelectedStatic { selection_index: 0 },
            },
        ];
        let mut bytes = Vec::new();

        pack_forward_shadowmask_metadata(2, &records, &[7, 9], &[3], &mut bytes);

        assert_eq!(
            bytes.len(),
            3 * FORWARD_SHADOWMASK_METADATA_BYTES_PER_RECORD,
            "two raw animated rows precede the one selected-static metadata row",
        );
        assert_eq!(read_f32(&bytes, 28), -1.0);
        assert_eq!(
            read_f32(&bytes, FORWARD_SHADOWMASK_METADATA_BYTES_PER_RECORD + 28),
            7.0,
        );
        let selected = 2 * FORWARD_SHADOWMASK_METADATA_BYTES_PER_RECORD;
        assert_eq!(read_f32(&bytes, selected), 19.0);
        assert_eq!(read_f32(&bytes, selected + 8), 3.0);
        assert_eq!(read_f32(&bytes, selected + 12), 0.5);
        assert_eq!(read_f32(&bytes, selected + 16), 1.0);
        assert_eq!(read_f32(&bytes, selected + 20), 1.0);
        assert_eq!(
            read_f32(&bytes, selected + 24),
            FORWARD_SHADOWMASK_RESERVED_VALUE
        );
        assert_eq!(read_f32(&bytes, selected + 28), 9.0);
    }
}
