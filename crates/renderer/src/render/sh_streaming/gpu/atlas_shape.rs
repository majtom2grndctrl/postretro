//! The dense atlas's geometry: one layer size for the session, whole layers.
//! See: context/lib/rendering_pipeline.md §4 (Pool growth at device limits)

use super::ShResidencyDrainError;

pub(super) const PHYSICAL_TILE_DIMENSION: u32 = 8;
// Fixes the session's layer size only: growth stops at the whole map, so this
// no longer buys room past it.
// Seeded whole-demand churn measurement and the additional lifetime margin
// are pinned by geometry_tests::fixture_churn_fits_fragmentation_headroom.
pub(super) const DENSE_FRAGMENTATION_HEADROOM_NUMERATOR: u64 = 9;
pub(super) const DENSE_FRAGMENTATION_HEADROOM_DENOMINATOR: u64 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::render::sh_streaming) struct AtlasShape {
    pub(in crate::render::sh_streaming) tiles_per_row: u32,
    pub(in crate::render::sh_streaming) tiles_per_layer: u32,
    pub(in crate::render::sh_streaming) layers: u32,
    pub(in crate::render::sh_streaming) slots: u32,
}

impl AtlasShape {
    pub(in crate::render::sh_streaming) fn for_slots(
        slots: u32,
        whole_slots: u32,
        limits: &wgpu::Limits,
    ) -> Result<Self, ShResidencyDrainError> {
        Self::for_slots_with_headroom(
            slots,
            whole_slots,
            limits,
            DENSE_FRAGMENTATION_HEADROOM_NUMERATOR,
            DENSE_FRAGMENTATION_HEADROOM_DENOMINATOR,
        )
    }

    pub(super) fn for_slots_with_headroom(
        slots: u32,
        whole_slots: u32,
        limits: &wgpu::Limits,
        headroom_numerator: u64,
        headroom_denominator: u64,
    ) -> Result<Self, ShResidencyDrainError> {
        let maximum = limits.max_texture_dimension_2d / PHYSICAL_TILE_DIMENSION;
        if maximum == 0 || slots == 0 || limits.max_texture_array_layers == 0 {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "adapter cannot allocate an 8x8 streamed SH cell",
            });
        }
        let whole_with_headroom = u64::from(whole_slots)
            .checked_mul(headroom_numerator)
            .ok_or(ShResidencyDrainError::SlotOverflow)?
            .div_ceil(headroom_denominator);
        let whole_minimum =
            whole_with_headroom.div_ceil(u64::from(limits.max_texture_array_layers));
        let full_layer = u64::from(maximum) * u64::from(maximum);
        // Past one full layer, equal layers spare the pool a nearly empty last
        // one. Below it, the layer is the whole target and pads by under a row.
        let layer_target = u64::from(slots)
            .div_ceil(u64::from(slots).div_ceil(full_layer))
            .max(whole_minimum)
            .min(full_layer)
            .max(1);
        let layer_target =
            u32::try_from(layer_target).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        let tiles_per_row = integer_sqrt_ceil(layer_target).min(maximum).max(1);
        // Width/height are fixed for a session. Once a layer is full, capacity
        // grows only by appending array layers, never by changing slot geometry.
        let rows = layer_target.div_ceil(tiles_per_row).min(maximum).max(1);
        let tiles_per_layer = tiles_per_row
            .checked_mul(rows)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let layers = slots.div_ceil(tiles_per_layer);
        if layers == 0 || layers > limits.max_texture_array_layers {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed SH pool needs unsupported array layers",
            });
        }
        Ok(Self {
            tiles_per_row,
            tiles_per_layer,
            layers,
            // The texture has a complete final layer. Treat its padding as
            // allocatable capacity rather than stranding it behind the
            // requested minimum, while preserving width/height geometry.
            slots: tiles_per_layer
                .checked_mul(layers)
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
        })
    }

    pub(in crate::render::sh_streaming) fn extent(self) -> wgpu::Extent3d {
        wgpu::Extent3d {
            width: self.tiles_per_row * PHYSICAL_TILE_DIMENSION,
            height: (self.tiles_per_layer / self.tiles_per_row) * PHYSICAL_TILE_DIMENSION,
            depth_or_array_layers: self.layers,
        }
    }

    /// The shape holding `needed_slots`, which must fit the device and the
    /// whole map, sized up to `wanted_slots` as far as both allow.
    pub(super) fn grown_for_slots(
        self,
        needed_slots: u32,
        wanted_slots: u32,
        whole_slots: u32,
        limits: &wgpu::Limits,
    ) -> Result<Self, ShResidencyDrainError> {
        if needed_slots <= self.slots {
            return Ok(self);
        }
        let minimum_layers = needed_slots.div_ceil(self.tiles_per_layer);
        if minimum_layers > limits.max_texture_array_layers {
            return Err(ShResidencyDrainError::GpuDeviceLimit {
                reason: "streamed dense pool cannot append enough texture-array layers",
            });
        }
        let whole_layers = whole_slots.div_ceil(self.tiles_per_layer);
        if minimum_layers > whole_layers {
            return Err(ShResidencyDrainError::PoolAtWholeMap {
                family: "dense atlas",
            });
        }
        let layers = wanted_slots
            .div_ceil(self.tiles_per_layer)
            .max(minimum_layers)
            .min(limits.max_texture_array_layers)
            .min(whole_layers);
        let slots = self
            .tiles_per_layer
            .checked_mul(layers)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        Ok(Self {
            tiles_per_row: self.tiles_per_row,
            tiles_per_layer: self.tiles_per_layer,
            layers,
            slots,
        })
    }
}

fn integer_sqrt_ceil(value: u32) -> u32 {
    let root = (value as f64).sqrt() as u32;
    if root.saturating_mul(root) == value {
        root
    } else {
        root + 1
    }
}
