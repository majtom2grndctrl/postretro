// Lights, promoted records and per-render cases for the union subtraction tests,
// as the renderer packs them.
// See: context/lib/rendering_pipeline.md §4 (Static specular ownership)

use postretro_render_cpu::frame_uniforms::SPEC_SHADOWMASK_FLAG_MASK_PRESENT;

use super::renderer_types::{
    PromotedBakedLightRecord, PromotedBakedLightSource, PromotedShadowPoolKind,
};
use super::shadowmask::pack_forward_shadowmask_metadata;

pub(super) const E: u16 = 0xFFFF;
/// Spot pool slots in the shader's `light_space_matrices`; the cube stub's
/// visibilities follow them in one table.
const SPOT_SLOTS: usize = 96;
/// The pre-change metadata `.z` for a dropped or unslotted light.
pub(super) const PRE_CHANGE_DROPPED: f32 = 4.0;

/// A linear-falloff point light, or a spot aimed straight down, as
/// `pack_spec_lights` lays it out.
fn spec_light(position: [f32; 3], color: [f32; 3], spot: bool) -> Vec<u8> {
    let (aim, kind, cos_inner, cos_outer) = if spot {
        (
            [0.0, -1.0, 0.0],
            1.0,
            0.6f32.cos(),
            std::f32::consts::FRAC_PI_3.cos(),
        )
    } else {
        ([0.0; 3], 0.0, 1.0, -1.0)
    };
    let floats = [
        position[0],
        position[1],
        position[2],
        9.0,
        color[0],
        color[1],
        color[2],
        0.0,
        aim[0],
        aim[1],
        aim[2],
        kind,
        cos_inner,
        cos_outer,
        0.0,
        0.0,
    ];
    floats.iter().flat_map(|f| f.to_ne_bytes()).collect()
}

/// Four spec lights over the floor, all in range of every column.
pub(super) fn spec_lights() -> Vec<u8> {
    [
        spec_light([-2.0, 2.0, 0.0], [1.0, 0.2, 0.1], false),
        spec_light([1.5, 2.5, 0.5], [0.1, 1.0, 0.3], true),
        spec_light([0.0, 3.0, -1.0], [0.6, 0.6, 0.6], false),
        spec_light([2.5, 1.5, 0.0], [0.2, 0.3, 1.0], false),
    ]
    .concat()
}

/// A promoted selected-static light: its compact spec index, pool slot and
/// promotion weight.
pub(super) struct Promoted {
    pub(super) spec_index: u32,
    pool: PromotedShadowPoolKind,
    slot: u32,
    weight: f32,
}

/// Spec lights 3, 1 and 0 promoted, in that record order.
pub(super) fn promoted() -> [Promoted; 3] {
    [
        Promoted {
            spec_index: 3,
            pool: PromotedShadowPoolKind::Spot,
            slot: 2,
            weight: 1.0,
        },
        Promoted {
            spec_index: 1,
            pool: PromotedShadowPoolKind::Cube,
            slot: 1,
            weight: 0.6,
        },
        Promoted {
            spec_index: 0,
            pool: PromotedShadowPoolKind::Spot,
            slot: 7,
            weight: 0.35,
        },
    ]
}

/// The influence buffer the renderer uploads with no dynamic or animated
/// lights: one uncullable influence per promoted light, then the metadata
/// tail from the production packer. `pre_change_slots[p]` overwrites record
/// `p`'s reserved `.z` lane with the map-wide mask slot the pre-change packer
/// wrote there.
pub(super) fn influence_bytes(promoted: &[Promoted], pre_change_slots: &[f32]) -> Vec<u8> {
    let mut bytes: Vec<u8> = Vec::new();
    for _ in promoted {
        for value in [0.0f32, 0.0, 0.0, 2.0e30] {
            bytes.extend_from_slice(&value.to_ne_bytes());
        }
    }
    let records: Vec<PromotedBakedLightRecord> = promoted
        .iter()
        .enumerate()
        .map(|(index, light)| PromotedBakedLightRecord {
            global_light_index: 20 + index as u32,
            pool_kind: light.pool,
            slot: light.slot,
            weight: light.weight,
            source: PromotedBakedLightSource::SelectedStatic {
                selection_index: index as u32,
            },
        })
        .collect();
    let spec_indices: Vec<u32> = promoted.iter().map(|light| light.spec_index).collect();
    let mut metadata = Vec::new();
    pack_forward_shadowmask_metadata(
        0,
        &records,
        &vec![-1; records.len()],
        &spec_indices,
        &mut metadata,
    );
    for (record, slot) in pre_change_slots.iter().enumerate() {
        let lane = record * 32 + 16 + 8;
        metadata[lane..lane + 4].copy_from_slice(&slot.to_ne_bytes());
    }
    bytes.extend_from_slice(&metadata);
    bytes
}

/// Entity-only visibility per pool slot: each promoted light's occluder
/// leaves a different fraction of its pool light.
pub(super) fn pool_visibility() -> Vec<u8> {
    let mut visibility = vec![1.0f32; SPOT_SLOTS + 6];
    visibility[2] = 0.0;
    visibility[7] = 0.8;
    visibility[SPOT_SLOTS + 1] = 0.4;
    visibility.iter().flat_map(|v| v.to_ne_bytes()).collect()
}

#[derive(Clone, Copy)]
pub(super) struct Case {
    pub(super) promoted_count: u32,
    pub(super) sdf_shadow_mode: u32,
    pub(super) mask_present: bool,
    /// What the stub mask returns.
    pub(super) mask: [f32; 4],
    pub(super) owners: [u16; 4],
    /// The block id + 1 a sampled render resolves.
    pub(super) block: u32,
}

impl Case {
    pub(super) fn bytes(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        let flags = if self.mask_present {
            SPEC_SHADOWMASK_FLAG_MASK_PRESENT
        } else {
            0
        };
        for word in [
            0u32,
            self.promoted_count,
            0,
            self.sdf_shadow_mode,
            flags,
            self.block,
            0,
            0,
        ] {
            out.extend_from_slice(&word.to_ne_bytes());
        }
        for value in self.mask {
            out.extend_from_slice(&value.to_ne_bytes());
        }
        for owner in self.owners {
            out.extend_from_slice(&u32::from(owner).to_ne_bytes());
        }
        out
    }
}
