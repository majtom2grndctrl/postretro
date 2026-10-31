// Whether a level's legacy SH volume binds on this device, and the CPU values
// that follow from that verdict: the animated descriptor count and the
// lattice the SH sampler (and the fluid grid) reads.
// See: context/lib/rendering_pipeline.md §4, §7.3a

use std::fmt;

use postretro_level_format::sh_volume::{AnimationDescriptor, OctahedralShVolumeSection};

use crate::device_limits::DeviceLimits;
use crate::fluid_surface::FluidGridLattice;
use crate::sh_compose::{
    DYNAMIC_COMPOSE_GRID_DIMS_SIZE, DYNAMIC_COMPOSE_ROW_CAPACITY, affinity_dims_for_grid,
};

/// Why a present SH volume is refused on this device. Every SH consumer then
/// takes the no-volume path for the level.
#[derive(Debug, Clone, PartialEq)]
pub enum ShVolumeRejection {
    /// A zero-dimension grid: treated as no section, silently.
    EmptyGrid,
    /// A non-finite origin or a non-positive cell size.
    DegenerateLattice {
        grid_origin: [f32; 3],
        cell_size: [f32; 3],
    },
    AtlasExceedsLimits {
        atlas_dimensions: [u32; 2],
        layer_count: u32,
        max_texture_dimension_2d: u32,
        max_texture_array_layers: u32,
    },
    DepthMomentExceedsLimits {
        grid_dimensions: [u32; 3],
        max_texture_dimension_3d: u32,
    },
    /// The compose gather cannot address every affinity row of the grid
    /// within the device's dispatch, alignment and buffer limits.
    ComposeGridExceedsLimits { affinity_dims: [u32; 3] },
}

impl ShVolumeRejection {
    /// Whether the renderer logs this refusal (an empty grid is silent).
    pub fn is_logged(&self) -> bool {
        !matches!(self, Self::EmptyGrid)
    }
}

impl fmt::Display for ShVolumeRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyGrid => write!(f, "SH volume grid has a zero dimension"),
            Self::DegenerateLattice {
                grid_origin,
                cell_size,
            } => write!(
                f,
                "SH volume lattice is degenerate (grid origin {grid_origin:?}, cell size {cell_size:?})"
            ),
            Self::AtlasExceedsLimits {
                atlas_dimensions,
                layer_count,
                max_texture_dimension_2d,
                max_texture_array_layers,
            } => write!(
                f,
                "Octahedral SH atlas {}x{}x{} exceeds device limits (maxTextureDimension2D {}, maxTextureArrayLayers {})",
                atlas_dimensions[0],
                atlas_dimensions[1],
                layer_count,
                max_texture_dimension_2d,
                max_texture_array_layers,
            ),
            Self::DepthMomentExceedsLimits {
                grid_dimensions,
                max_texture_dimension_3d,
            } => write!(
                f,
                "SH depth-moment grid {}x{}x{} exceeds device maxTextureDimension3D {}",
                grid_dimensions[0],
                grid_dimensions[1],
                grid_dimensions[2],
                max_texture_dimension_3d,
            ),
            Self::ComposeGridExceedsLimits { affinity_dims } => write!(
                f,
                "SH compose grid of {}x{}x{} affinity rows exceeds the device's compose dispatch \
                 or buffer limits",
                affinity_dims[0], affinity_dims[1], affinity_dims[2],
            ),
        }
    }
}

/// The device-limit filter `ShVolumeResources::new` applies to a present SH
/// volume, in its order. `Ok` binds the volume.
pub fn check_sh_volume(
    section: &OctahedralShVolumeSection,
    limits: &DeviceLimits,
) -> Result<(), ShVolumeRejection> {
    if section.grid_dimensions.contains(&0) {
        return Err(ShVolumeRejection::EmptyGrid);
    }
    // A degenerate lattice would sample through `max(cell, 1e-6)` or NaN.
    let sane = section.grid_origin.iter().all(|v| v.is_finite())
        && section.cell_size.iter().all(|v| v.is_finite() && *v > 0.0);
    if !sane {
        return Err(ShVolumeRejection::DegenerateLattice {
            grid_origin: section.grid_origin,
            cell_size: section.cell_size,
        });
    }
    if !atlas_fits(section.atlas_dimensions, section.layer_count, limits) {
        return Err(ShVolumeRejection::AtlasExceedsLimits {
            atlas_dimensions: section.atlas_dimensions,
            layer_count: section.layer_count,
            max_texture_dimension_2d: limits.max_texture_dimension_2d,
            max_texture_array_layers: limits.max_texture_array_layers,
        });
    }
    if !volume_3d_fits(section.grid_dimensions, limits) {
        return Err(ShVolumeRejection::DepthMomentExceedsLimits {
            grid_dimensions: section.grid_dimensions,
            max_texture_dimension_3d: limits.max_texture_dimension_3d,
        });
    }
    // The loader pins ids 27 and 41's affinity grids to ceil(grid / 4) and
    // id 35's grid to id 34's, so this one check covers the legacy indirect
    // and direct compose grids alike.
    let affinity_dims = affinity_dims_for_grid(section.grid_dimensions);
    if !dynamic_compose_grid_fits(
        affinity_dims,
        limits.max_compute_workgroups_per_dimension,
        limits.min_uniform_buffer_offset_alignment,
        limits.max_buffer_size,
    ) {
        return Err(ShVolumeRejection::ComposeGridExceedsLimits { affinity_dims });
    }
    Ok(())
}

/// `section` when it passes [`check_sh_volume`], else `None`. Logs nothing:
/// the renderer's GPU half logs the refusal once.
pub fn usable_sh_volume<'a>(
    section: Option<&'a OctahedralShVolumeSection>,
    limits: &DeviceLimits,
) -> Option<&'a OctahedralShVolumeSection> {
    section.filter(|section| check_sh_volume(section, limits).is_ok())
}

pub fn atlas_fits(per_layer_dim: [u32; 2], layer_count: u32, limits: &DeviceLimits) -> bool {
    per_layer_dim[0] > 0
        && per_layer_dim[1] > 0
        && layer_count > 0
        && per_layer_dim[0] <= limits.max_texture_dimension_2d
        && per_layer_dim[1] <= limits.max_texture_dimension_2d
        && layer_count <= limits.max_texture_array_layers
}

pub fn volume_3d_fits(dimensions: [u32; 3], limits: &DeviceLimits) -> bool {
    dimensions
        .iter()
        .all(|&dimension| dimension > 0 && dimension <= limits.max_texture_dimension_3d)
}

/// Whether the renderer's dynamic compose gather can address every row of an
/// `affinity_dims` grid: the same arithmetic as its upload builder
/// (`build_dynamic_compose_grid_upload`), which a renderer test holds equal.
pub fn dynamic_compose_grid_fits(
    affinity_dims: [u32; 3],
    max_workgroups_x: u32,
    dynamic_offset_alignment: u32,
    max_buffer_size: u64,
) -> bool {
    let Some(rows) = affinity_dims
        .into_iter()
        .try_fold(1u32, |count, dimension| count.checked_mul(dimension))
    else {
        return false;
    };
    if dynamic_offset_alignment == 0 {
        return false;
    }
    if rows == 0 {
        return true;
    }
    let Ok(row_capacity) = u32::try_from(DYNAMIC_COMPOSE_ROW_CAPACITY) else {
        return false;
    };
    let chunk_capacity = row_capacity.min(max_workgroups_x);
    if chunk_capacity == 0 {
        return false;
    }
    let Ok(record_size) = u32::try_from(DYNAMIC_COMPOSE_GRID_DIMS_SIZE) else {
        return false;
    };
    let Some(stride) = record_size
        .checked_add(dynamic_offset_alignment - 1)
        .and_then(|bytes| bytes.checked_div(dynamic_offset_alignment))
        .and_then(|records| records.checked_mul(dynamic_offset_alignment))
    else {
        return false;
    };
    let last_chunk = rows.div_ceil(chunk_capacity) - 1;
    let Some(last_offset) = last_chunk.checked_mul(stride) else {
        return false;
    };
    u64::from(last_offset) + DYNAMIC_COMPOSE_GRID_DIMS_SIZE as u64 <= max_buffer_size
}

/// The animated descriptor count the SH volume binds: the usable legacy
/// section's, else a streamed manifest's base descriptors, else zero.
pub fn animated_light_count(
    usable: Option<&OctahedralShVolumeSection>,
    stream_animation_descriptors: Option<&[AnimationDescriptor]>,
) -> u32 {
    match (usable, stream_animation_descriptors) {
        (Some(section), _) => section.animation_descriptors.len() as u32,
        (None, Some(descriptors)) => descriptors.len() as u32,
        (None, None) => 0,
    }
}

/// The lattice the SH sampler reads, which fluid faces are cut along: a
/// streamed level's base metadata `(grid_origin, cell_size)`, else a usable
/// legacy volume's, else none.
pub fn sh_sampler_lattice(
    streamed: Option<([f32; 3], [f32; 3])>,
    legacy: Option<([f32; 3], [f32; 3])>,
) -> Option<FluidGridLattice> {
    let (origin, cell_size) = streamed.or(legacy)?;
    FluidGridLattice::new(origin, cell_size)
}
