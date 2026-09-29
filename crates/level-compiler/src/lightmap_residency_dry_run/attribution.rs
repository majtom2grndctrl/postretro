//! Byte attribution of the stored id-22 and id-42 payloads to receiver cells.
//!
//! Every stored grid unit (a BC 4×4 block, or a raw texel) goes to exactly one
//! owner: the cell whose charts cover most of the irradiance texels under it,
//! ties to the lowest cell id. A unit no chart touches is unattributed, so
//! attributed plus unattributed equals each payload exactly.

use super::{AtlasFormats, ChartRect, EncodingUnit};

const NO_OWNER: u32 = u32::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SectionAttribution {
    pub per_cell: Vec<u64>,
    pub unattributed: u64,
    /// Payload bytes the header declares, for the exactness check.
    pub payload: u64,
}

impl SectionAttribution {
    fn new(cell_count: usize, payload: u64) -> Self {
        Self {
            per_cell: vec![0; cell_count],
            unattributed: 0,
            payload,
        }
    }

    pub(crate) fn attributed(&self) -> u64 {
        self.per_cell.iter().sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Attribution {
    pub irradiance: SectionAttribution,
    pub direction: SectionAttribution,
    pub shadowmask: Option<SectionAttribution>,
    /// Irradiance texels claimed by two charts. Nonzero means the chart
    /// reconstruction disagrees with the stored packing.
    pub overlap_texels: u64,
}

pub(crate) fn attribute(
    formats: &AtlasFormats,
    charts: &[ChartRect],
    cell_count: usize,
) -> Attribution {
    let mut result = Attribution {
        irradiance: SectionAttribution::new(cell_count, formats.irr_payload_bytes),
        direction: SectionAttribution::new(cell_count, formats.dir_payload_bytes),
        shadowmask: formats
            .stored_shadowmask()
            .map(|sm| SectionAttribution::new(cell_count, sm.payload_bytes)),
        overlap_texels: 0,
    };
    let width = formats.irr_width as usize;
    let height = formats.irr_height as usize;
    let mut by_layer: Vec<Vec<&ChartRect>> = vec![Vec::new(); formats.layer_count as usize];
    for chart in charts {
        by_layer[chart.layer as usize].push(chart);
    }

    let irr_unit = formats.irradiance_unit();
    let dir_unit = EncodingUnit {
        width: 1,
        height: 1,
        bytes: formats.direction_texel_bytes(),
    };
    let mut owners = vec![NO_OWNER; width * height];
    let mut tally = Vec::with_capacity(16);
    for layer_charts in &by_layer {
        owners.fill(NO_OWNER);
        for chart in layer_charts {
            for y in chart.y..chart.y + chart.height {
                let row = y as usize * width;
                for x in chart.x..chart.x + chart.width {
                    let slot = &mut owners[row + x as usize];
                    if *slot != NO_OWNER {
                        result.overlap_texels += 1;
                    }
                    // Last writer wins on overlap; overlap is reported, not resolved.
                    *slot = chart.cell;
                }
            }
        }
        let grid = OwnerGrid {
            owners: &owners,
            width,
            height,
        };
        let irr_cols = formats.irr_width.div_ceil(irr_unit.width);
        let irr_rows = formats.irr_height.div_ceil(irr_unit.height);
        grid.attribute_units(
            irr_cols,
            irr_rows,
            irr_unit.bytes,
            &mut result.irradiance,
            &mut tally,
        );
        grid.attribute_units(
            formats.dir_width,
            formats.dir_height,
            dir_unit.bytes,
            &mut result.direction,
            &mut tally,
        );
        if let (Some(sm), Some(out)) = (formats.stored_shadowmask(), result.shadowmask.as_mut()) {
            let unit = AtlasFormats::shadowmask_unit();
            grid.attribute_units(
                sm.width.div_ceil(unit.width),
                sm.height.div_ceil(unit.height),
                unit.bytes,
                out,
                &mut tally,
            );
        }
    }
    result
}

struct OwnerGrid<'a> {
    owners: &'a [u32],
    width: usize,
    height: usize,
}

impl OwnerGrid<'_> {
    /// Split the irradiance grid into `cols × rows` equal units and credit
    /// each unit's `bytes` to its majority owner.
    fn attribute_units(
        &self,
        cols: u32,
        rows: u32,
        bytes: u64,
        out: &mut SectionAttribution,
        tally: &mut Vec<(u32, u32)>,
    ) {
        for row in 0..rows as usize {
            let (y0, y1) = span(row, rows as usize, self.height);
            for col in 0..cols as usize {
                let (x0, x1) = span(col, cols as usize, self.width);
                tally.clear();
                for y in y0..y1 {
                    for &owner in &self.owners[y * self.width + x0..y * self.width + x1] {
                        if owner == NO_OWNER {
                            continue;
                        }
                        match tally.iter_mut().find(|(cell, _)| *cell == owner) {
                            Some((_, count)) => *count += 1,
                            None => tally.push((owner, 1)),
                        }
                    }
                }
                let majority = tally
                    .iter()
                    .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
                    .map(|&(cell, _)| cell);
                match majority {
                    Some(cell) => out.per_cell[cell as usize] += bytes,
                    None => out.unattributed += bytes,
                }
            }
        }
    }
}

/// Irradiance texel range `[start, end)` covered by unit `index` of `units`
/// across `extent` texels; never empty.
fn span(index: usize, units: usize, extent: usize) -> (usize, usize) {
    let start = index * extent / units;
    (start, ((index + 1) * extent / units).max(start + 1))
}
