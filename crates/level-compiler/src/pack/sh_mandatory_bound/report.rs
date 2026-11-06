//! The SH mandatory bound as an author reads it: priced as the pool prices
//! it, with a warning past what the other pools leave of the memory target.
//! See: context/lib/build_pipeline.md §PRL section IDs (SH mandatory bound)

use std::time::Duration;

use postretro_level_format::cell_visibility::CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;
use postretro_level_format::cells::CellsSection;
use postretro_level_format::cluster_directory::{ShMandatoryBoundEntry, WHOLE_MAP_WORST_CELL};
use postretro_level_format::sh_pool_cost::{
    ShDenseSlotTextures, sh_dense_slot_bytes, sh_sparse_capacity_bytes,
};

/// What the other pools leave of the 2 GB target: 2048 MiB minus the lightmap
/// pool (about 490 MiB), the shadow pools (about 427 MiB) and everything else
/// (about 650 MiB) is about 480 MiB, rounded to 512 MiB. Recompute from those
/// figures when any of them moves. Fixed, not an author key, like occluder
/// classification.
const SH_MANDATORY_WARN_BYTES: u64 = 512 * MIB;
const MIB: u64 = 1024 * 1024;

/// One stored lead, in the units an author reads.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct PricedLead {
    lead: u32,
    pub(super) bytes: u64,
    dense_slots: u32,
    /// `None` when the figure is the whole map.
    worst: Option<WorstCellPlace>,
}

#[derive(Debug, Clone, PartialEq)]
struct WorstCellPlace {
    cell: u32,
    /// Engine space, metres: the shared stages hold no editor coordinates, and
    /// the inverse of the source adapter's axis swizzle and scale lives with
    /// the adapter, so this does not claim to be a map coordinate.
    center: [f32; 3],
}

/// What the Build Summary says about the bound. No stored leads means the
/// level has no streamed SH to bound.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ShMandatoryReport {
    pub(super) leads: Vec<PricedLead>,
    /// The bound loop alone, inside the Packing stage.
    pub(crate) elapsed: Duration,
}

impl ShMandatoryReport {
    pub(super) fn price(
        entries: &[ShMandatoryBoundEntry],
        textures: ShDenseSlotTextures,
        cells: &CellsSection,
        elapsed: Duration,
    ) -> anyhow::Result<Self> {
        let slot_bytes = sh_dense_slot_bytes(textures).ok_or_else(|| {
            anyhow::anyhow!(
                "SH mandatory bound: the pool cannot allocate atlas formats {textures:?}"
            )
        })?;
        let leads = entries
            .iter()
            .map(|entry| {
                let dense_bytes = u64::from(entry.worst_dense_slots) * slot_bytes;
                let sparse_bytes: u64 = entry
                    .families
                    .iter()
                    .map(|family| {
                        sh_sparse_capacity_bytes(family.worst_entries, family.worst_tile_f16)
                    })
                    .sum();
                PricedLead {
                    lead: entry.lead,
                    bytes: dense_bytes + sparse_bytes,
                    dense_slots: entry.worst_dense_slots,
                    worst: (entry.worst_cell != WHOLE_MAP_WORST_CELL)
                        .then(|| place_of(entry.worst_cell, cells)),
                }
            })
            .collect();
        Ok(Self { leads, elapsed })
    }

    /// The Build Summary block. The first stored lead is the one the pool is
    /// sized from, so it carries the headline and the warning.
    pub(crate) fn lines(&self) -> Vec<String> {
        let Some((sized, wider)) = self.leads.split_first() else {
            return vec!["SH mandatory memory: none (this level streams no SH probes)".to_string()];
        };
        let mut lines = vec![match &sized.worst {
            Some(worst) => format!(
                "SH mandatory memory: {} pool demand at {} (how far ahead of the camera \
                 probes load), {} probe slots; dense demand peaks {}",
                mib(sized.bytes),
                metres(sized.lead),
                sized.dense_slots,
                worst.describe(),
            ),
            None => format!(
                "SH mandatory memory: {} pool demand, {} probe slots, the whole map: \
                 without usable portals every probe stays loaded",
                mib(sized.bytes),
                sized.dense_slots,
            ),
        }];
        lines.extend(wider.iter().filter_map(|lead| {
            lead.worst.as_ref().map(|worst| {
                format!(
                    "  at the maximum {}: {} pool demand, {} probe slots; dense demand peaks {}",
                    metres(lead.lead),
                    mib(lead.bytes),
                    lead.dense_slots,
                    worst.describe(),
                )
            })
        }));
        lines.extend(
            self.warning()
                .map(|warning| format!("  warning: {warning}")),
        );
        lines
    }

    pub(super) fn log(&self) {
        if let Some(sized) = self.leads.first() {
            log::info!(
                "[Compiler] SH mandatory bound: {} stored lead(s), {} at the first, {:.3} ms",
                self.leads.len(),
                mib(sized.bytes),
                self.elapsed.as_secs_f64() * 1000.0,
            );
        }
        if let Some(warning) = self.warning() {
            log::warn!("[Compiler] {warning}");
        }
    }

    /// Fires strictly above the threshold, on the lead the pool is sized from.
    fn warning(&self) -> Option<String> {
        let sized = self.leads.first()?;
        if sized.bytes <= SH_MANDATORY_WARN_BYTES {
            return None;
        }
        let over = format!(
            "SH mandatory memory {} is above the {} budget, so this level may not fit a 2 GB GPU.",
            mib(sized.bytes),
            mib(SH_MANDATORY_WARN_BYTES),
        );
        Some(match &sized.worst {
            Some(worst) => format!(
                "{over} Add walls or doors near {}, where probe demand peaks, so less of the map can be seen and reached from there.",
                worst.position(),
            ),
            None => format!(
                "{over} Seal the level so it has usable portals, which let probes load by area."
            ),
        })
    }
}

fn place_of(cell: u32, cells: &CellsSection) -> WorstCellPlace {
    let record = &cells.cells[cell as usize];
    WorstCellPlace {
        cell,
        center: std::array::from_fn(|axis| {
            (record.bounds_min[axis] + record.bounds_max[axis]) * 0.5
        }),
    }
}

impl WorstCellPlace {
    fn describe(&self) -> String {
        format!("in cell {}, near {}", self.cell, self.position())
    }

    fn position(&self) -> String {
        let [x, y, z] = self.center;
        format!("({x:.1}, {y:.1}, {z:.1}) engine metres")
    }
}

fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / MIB as f64)
}

fn metres(lead: u32) -> String {
    format!(
        "{} m lead",
        lead as f32 / CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE as f32
    )
}
