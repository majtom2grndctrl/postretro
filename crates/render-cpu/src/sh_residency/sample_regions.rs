// GPU-free world-region to affinity-row helpers the residency plan's
// sample-region tables and the renderer's per-frame gate share.
// See: context/lib/rendering_pipeline.md §4 "Sampled-row compose"

use glam::Vec3;

use super::ShResidencyDrainError;
use super::stored_nodes::StoredNode;

/// How far a sample region dilates, in probe spacings, so the trilinear
/// footprint of any point inside it is covered.
pub const SAMPLE_REGION_DILATION_CELLS: f32 = 1.1;

/// One world-space AABB whose drawn pixels may sample the SH probe volume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShSampleRegion {
    pub min: Vec3,
    pub max: Vec3,
}

impl ShSampleRegion {
    pub const fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }
}

/// The streamed base grid and its dense stored-node layout.
pub struct SampleRegionGrid<'a> {
    pub origin: [f32; 3],
    pub cell_size: [f32; 3],
    pub dimensions: [u32; 3],
    pub dense_nodes: &'a [Option<StoredNode>],
}

/// Inclusive affinity-brick bounds of the probes whose trilinear footprint
/// `region` can sample: the region dilated by `SAMPLE_REGION_DILATION_CELLS`
/// probe spacings and clamped to the grid. `None` when the grid or region
/// is degenerate.
pub fn region_brick_bounds(
    origin: [f32; 3],
    cell_size: [f32; 3],
    dimensions: [u32; 3],
    region: ShSampleRegion,
) -> Option<([u32; 3], [u32; 3])> {
    if dimensions.contains(&0)
        || cell_size
            .iter()
            .any(|spacing| !spacing.is_finite() || *spacing <= 0.0)
        || !region.min.is_finite()
        || !region.max.is_finite()
    {
        return None;
    }

    let min = region.min.min(region.max);
    let max = region.min.max(region.max);
    let origin = glam::Vec3::from_array(origin);
    let spacing = glam::Vec3::from_array(cell_size);
    let dilation = spacing * SAMPLE_REGION_DILATION_CELLS;
    let normalized_min = (min - dilation - origin) / spacing;
    let normalized_max = (max + dilation - origin) / spacing;
    let grid_max = dimensions.map(|dimension| dimension.saturating_sub(1));
    let first = [
        clamp_probe(normalized_min.x.floor(), grid_max[0]),
        clamp_probe(normalized_min.y.floor(), grid_max[1]),
        clamp_probe(normalized_min.z.floor(), grid_max[2]),
    ];
    // `floor + 1` includes the high trilinear corner on a probe plane.
    let last = [
        clamp_probe(normalized_max.x.floor() + 1.0, grid_max[0]),
        clamp_probe(normalized_max.y.floor() + 1.0, grid_max[1]),
        clamp_probe(normalized_max.z.floor() + 1.0, grid_max[2]),
    ];
    Some((first.map(|probe| probe / 4), last.map(|probe| probe / 4)))
}

/// Every brick in inclusive `bounds`, x fastest.
pub fn brick_rows_in((first, last): ([u32; 3], [u32; 3])) -> impl Iterator<Item = [u32; 3]> {
    (first[2]..=last[2]).flat_map(move |z| {
        (first[1]..=last[1]).flat_map(move |y| (first[0]..=last[0]).map(move |x| [x, y, z]))
    })
}

pub fn clamp_probe(value: f32, maximum: u32) -> u32 {
    value.max(0.0).min(maximum as f32) as u32
}

pub fn scaled_writer_row(
    grid: &SampleRegionGrid<'_>,
    sampled_row: u32,
) -> Result<Option<u32>, ShResidencyDrainError> {
    let affinity_dims = grid.dimensions.map(|dimension| dimension.div_ceil(4));
    let brick = affinity_coordinates(sampled_row, affinity_dims)?;
    let first_probe = brick.map(|coordinate| coordinate * 4);
    let last_probe = [
        (first_probe[0] + 4).min(grid.dimensions[0]),
        (first_probe[1] + 4).min(grid.dimensions[1]),
        (first_probe[2] + 4).min(grid.dimensions[2]),
    ];
    let mut writer = None;
    for z in first_probe[2]..last_probe[2] {
        for y in first_probe[1]..last_probe[1] {
            for x in first_probe[0]..last_probe[0] {
                let dense = dense_index([x, y, z], grid.dimensions)?;
                let Some(node) = grid.dense_nodes.get(dense as usize).copied().flatten() else {
                    continue;
                };
                if node.scale == 0 {
                    continue;
                }
                let row = affinity_row(node.brick_origin, grid.dimensions)?;
                match writer {
                    None => writer = Some(row),
                    Some(prior) if prior == row => {}
                    Some(_) => {
                        return Err(ShResidencyDrainError::GpuCapacity {
                            reason: "one affinity row resolves to multiple scaled-node writers",
                        });
                    }
                }
            }
        }
    }
    Ok(writer)
}

pub fn affinity_row(
    brick: [u32; 3],
    grid_dimensions: [u32; 3],
) -> Result<u32, ShResidencyDrainError> {
    let dims = grid_dimensions.map(|dimension| dimension.div_ceil(4));
    if brick[0] >= dims[0] || brick[1] >= dims[1] || brick[2] >= dims[2] {
        return Err(ShResidencyDrainError::SlotOverflow);
    }
    brick[0]
        .checked_add(
            brick[1]
                .checked_mul(dims[0])
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
        )
        .and_then(|row| row.checked_add(brick[2].checked_mul(dims[0].checked_mul(dims[1])?)?))
        .ok_or(ShResidencyDrainError::SlotOverflow)
}

fn affinity_coordinates(row: u32, dimensions: [u32; 3]) -> Result<[u32; 3], ShResidencyDrainError> {
    let xy = dimensions[0]
        .checked_mul(dimensions[1])
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let count = xy
        .checked_mul(dimensions[2])
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    if xy == 0 || row >= count {
        return Err(ShResidencyDrainError::SlotOverflow);
    }
    Ok([
        row % dimensions[0],
        (row / dimensions[0]) % dimensions[1],
        row / xy,
    ])
}

fn dense_index(xyz: [u32; 3], dimensions: [u32; 3]) -> Result<u32, ShResidencyDrainError> {
    let xy = dimensions[0]
        .checked_mul(dimensions[1])
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    xyz[0]
        .checked_add(
            xyz[1]
                .checked_mul(dimensions[0])
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
        )
        .and_then(|dense| dense.checked_add(xyz[2].checked_mul(xy)?))
        .ok_or(ShResidencyDrainError::SlotOverflow)
}
