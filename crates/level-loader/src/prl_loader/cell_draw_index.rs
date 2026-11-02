// CellDrawIndex (id 37): decode and cross-validation against BVH leaves and cells.
// See: context/lib/build_pipeline.md §PRL Compilation

use postretro_level_format::cell_draw_index::{CELL_DRAW_INDEX_VERSION, CellDrawIndexSection};
use postretro_render_data::geometry::{BvhLeaf, BvhTree};

use super::section_validation;
use crate::{CellData, CellDrawIndex, PrlLoadError};

/// Cross-validate a decoded `CellDrawIndex` section against the runtime BVH
/// leaf array and the loaded Cells section. `from_bytes` already enforced structural
/// CSR invariants (version, length, monotonic offsets, non-empty spans, no
/// `leaf_start + leaf_count` overflow); this layer enforces every invariant that
/// requires the *other* sections to be present.
///
/// Pure (no I/O, no logging) so each reject path is unit-testable without a
/// `.prl`. Returns `Err(reason)` describing the first failing invariant; the
/// loader wraps it as a `CellDrawIndex` section-validation load error.
///
/// Rejected cases:
/// - unsupported `version`,
/// - `cell_count != Cells.cell_count`,
/// - `cell_span_offset[0] != 0`, non-monotonic offsets, or
///   `cell_span_offset[cell_count] != span_count`,
/// - any span outside `[0, total_leaves)` (checked-add),
/// - any span whose leaves don't all carry `BvhLeaf.cell_id == cell`,
/// - any span covering a non-drawable leaf, or any span on a non-drawable cell,
/// - any span crossing a material-bucket boundary,
/// - spans out of ascending `leaf_start` order within a cell,
/// - adjacent same-cell/same-bucket spans that form a non-maximal run,
/// - overlapping / duplicate leaf coverage,
/// - any drawable leaf missing from the index,
/// - any non-drawable cell with a non-empty CSR row.
///
/// `version` is taken from the section header; `from_bytes` already rejects a
/// non-matching version, but the explicit guard keeps the rule local and lets a
/// future structurally-valid version bump be rejected here too.
pub(crate) fn validate_cell_draw_index(
    section: &CellDrawIndexSection,
    bvh_leaves: &[BvhLeaf],
    cells: &[CellData],
    version: u32,
) -> Result<(), String> {
    if version != CELL_DRAW_INDEX_VERSION {
        return Err(format!(
            "unsupported version {version}, expected {CELL_DRAW_INDEX_VERSION}"
        ));
    }

    let cell_count = section.cell_count as usize;
    if cell_count != cells.len() {
        return Err(format!(
            "cell_count {cell_count} != Cells cell_count {}",
            cells.len()
        ));
    }

    // Re-check the CSR offset invariants here too: a structurally-valid section
    // for a *future* version could reach this layer, and the validity of every
    // span lookup below depends on these. (from_bytes guards the current shape.)
    let offsets = &section.cell_span_offset;
    if offsets.len() != cell_count + 1 {
        return Err(format!(
            "offset table length {} != cell_count + 1 ({})",
            offsets.len(),
            cell_count + 1
        ));
    }
    if offsets[0] != 0 {
        return Err(format!("offset[0] {} != 0", offsets[0]));
    }
    for w in offsets.windows(2) {
        if w[1] < w[0] {
            return Err(format!("non-monotonic offsets: {} after {}", w[1], w[0]));
        }
    }
    if offsets[cell_count] != section.span_count {
        return Err(format!(
            "offset[cell_count] {} != span_count {}",
            offsets[cell_count], section.span_count
        ));
    }
    if section.spans.len() != section.span_count as usize {
        return Err(format!(
            "span array length {} != span_count {}",
            section.spans.len(),
            section.span_count
        ));
    }

    let total_leaves = bvh_leaves.len();

    // A BVH leaf is drawable iff it has indices AND its cell is drawable. Both
    // halves join through `cell_id == cell index`.
    let cell_is_drawable =
        |cell: usize| -> bool { cells.get(cell).is_some_and(|cell| cell.is_drawable) };
    let leaf_is_drawable = |leaf: &BvhLeaf| -> bool {
        leaf.index_count > 0 && cell_is_drawable(leaf.cell_id as usize)
    };

    // Coverage map over every BVH leaf: which leaves a cell row claims.
    let mut covered = vec![false; total_leaves];

    for cell in 0..cell_count {
        let start = offsets[cell] as usize;
        let end = offsets[cell + 1] as usize;
        let cell_spans = &section.spans[start..end];

        let mut prev_end: Option<u32> = None; // exclusive end of previous span
        let mut prev_bucket: Option<u32> = None;

        for span in cell_spans {
            let leaf_start = span.leaf_start;
            let leaf_count = span.leaf_count;
            // Structural guard re-asserted for non-current versions.
            if leaf_count == 0 {
                return Err(format!("cell {cell} has an empty span"));
            }
            let span_end = match leaf_start.checked_add(leaf_count) {
                Some(e) => e,
                None => {
                    return Err(format!(
                        "cell {cell} span leaf_start {leaf_start} + leaf_count {leaf_count} \
                         overflows u32"
                    ));
                }
            };
            if span_end as usize > total_leaves {
                return Err(format!(
                    "cell {cell} span [{leaf_start}, {span_end}) exceeds total BVH leaves \
                     {total_leaves}"
                ));
            }

            // Ascending, non-overlapping `leaf_start` within the cell.
            if let Some(pe) = prev_end
                && leaf_start < pe
            {
                return Err(format!(
                    "cell {cell} spans out of order / overlapping: span starting \
                         {leaf_start} follows a span ending {pe}"
                ));
            }

            let span_bucket = bvh_leaves[leaf_start as usize].material_bucket_id;

            // Every leaf in the span: belongs to this cell, is drawable, shares
            // one material bucket, and is not already covered.
            for idx in leaf_start..span_end {
                let leaf = &bvh_leaves[idx as usize];
                if leaf.cell_id as usize != cell {
                    return Err(format!(
                        "cell {cell} span covers BVH leaf {idx} whose cell_id is {} \
                         (wrong cell)",
                        leaf.cell_id
                    ));
                }
                if leaf.material_bucket_id != span_bucket {
                    return Err(format!(
                        "cell {cell} span [{leaf_start}, {span_end}) crosses material bucket \
                         boundary at leaf {idx} (bucket {} != {span_bucket})",
                        leaf.material_bucket_id
                    ));
                }
                if !leaf_is_drawable(leaf) {
                    return Err(format!(
                        "cell {cell} span covers non-drawable BVH leaf {idx} \
                         (index_count {} on a {} cell)",
                        leaf.index_count,
                        if cell_is_drawable(cell) {
                            "drawable"
                        } else {
                            "non-drawable"
                        }
                    ));
                }
                if covered[idx as usize] {
                    return Err(format!(
                        "cell {cell} span re-covers already-claimed BVH leaf {idx} \
                         (overlap / duplicate)"
                    ));
                }
                covered[idx as usize] = true;
            }

            // Non-maximal run: an adjacent same-bucket span that abuts the
            // previous one (prev_end == leaf_start) could have been one span.
            if let (Some(pe), Some(pb)) = (prev_end, prev_bucket)
                && pe == leaf_start
                && pb == span_bucket
            {
                return Err(format!(
                    "cell {cell} has a non-maximal run: spans abutting at leaf \
                         {leaf_start} in bucket {span_bucket} should be one span"
                ));
            }

            prev_end = Some(span_end);
            prev_bucket = Some(span_bucket);
        }
    }

    // Cross-check both directions:
    //   - every drawable BVH leaf must be covered by exactly one span,
    //   - non-drawable cells must have an empty CSR row.
    let mut cell_has_drawable_leaf = vec![false; cell_count];
    for (idx, leaf) in bvh_leaves.iter().enumerate() {
        if leaf_is_drawable(leaf) {
            // `leaf_is_drawable` already required `cell_id` to name a drawable
            // cell, so the cast is in range here.
            cell_has_drawable_leaf[leaf.cell_id as usize] = true;
            if !covered[idx] {
                return Err(format!(
                    "drawable BVH leaf {idx} (cell {}) is missing from the draw index",
                    leaf.cell_id
                ));
            }
        }
    }

    for cell in 0..cell_count {
        let row_empty = offsets[cell] == offsets[cell + 1];
        if !cell_has_drawable_leaf[cell] && !row_empty {
            return Err(format!("non-drawable cell {cell} has a non-empty CSR row"));
        }
    }

    Ok(())
}

pub(super) fn decode_cell_draw_index(
    cell_draw_index_data: Option<Vec<u8>>,
    bvh: &BvhTree,
    cells: &[CellData],
) -> Result<Option<CellDrawIndex>, PrlLoadError> {
    // Decode + cross-validate the CellDrawIndex (id 37) now that Cells and BVH
    // are both available. Non-empty BVHs require it; empty BVHs reject it.
    let cell_draw_index: Option<CellDrawIndex> = if bvh.leaves.is_empty() {
        if cell_draw_index_data.is_some() {
            return Err(section_validation(
                "CellDrawIndex",
                "section is present for a zero-leaf BVH; empty BVHs must omit CellDrawIndex",
            ));
        }
        None
    } else {
        let data = cell_draw_index_data.ok_or_else(|| {
            section_validation(
                "CellDrawIndex",
                format!(
                    "section is required when Bvh contains {} leaf/leaves",
                    bvh.leaves.len()
                ),
            )
        })?;
        {
            let header_version = data
                .get(0..4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            let section = CellDrawIndexSection::from_bytes(&data)
                .map_err(|err| section_validation("CellDrawIndex", err.to_string()))?;
            let version = header_version.unwrap_or(CELL_DRAW_INDEX_VERSION);
            validate_cell_draw_index(&section, &bvh.leaves, cells, version)
                .map_err(|reason| section_validation("CellDrawIndex", reason))?;
            log::info!(
                "[PRL] CellDrawIndex: {} cells, {} spans (candidate-cull index loaded)",
                section.cell_count,
                section.span_count,
            );
            Some(section)
        }
    };
    Ok(cell_draw_index)
}
