// The planar mirror's cell-set helpers: the seed domain and ascending set unions.
// See: context/lib/rendering_pipeline.md §2, §7.3a

use std::cmp::Ordering;

use postretro_level_loader::LevelWorld;
use postretro_visibility::{VisibilityPath, VisibleCells};

use crate::render_preparation::VisibleRenderPreparation;

/// The camera's reach the seeds come from: the walk's fog reach on a walk
/// path, the drawn set on a fallback (every cell when it draws all).
/// Ascending either way, as seed collection requires.
pub(super) fn seed_domain<'a>(
    main: &'a VisibleRenderPreparation,
    world: &LevelWorld,
    all_cells: &'a mut Vec<u32>,
) -> &'a [u32] {
    if matches!(main.stats.path, VisibilityPath::PrlPortal { .. }) {
        return &main.fog_reachable;
    }
    match &main.visible_cells {
        VisibleCells::Culled(cells) => cells,
        VisibleCells::DrawAll => {
            all_cells.clear();
            all_cells.extend(0..world.cell_count() as u32);
            all_cells
        }
    }
}

/// `cells` as an explicit list. A `DrawAll` becomes an empty list first,
/// which allocates nothing; the mirror's own sets never draw all.
pub(super) fn culled_list(cells: &mut VisibleCells) -> &mut Vec<u32> {
    if matches!(cells, VisibleCells::DrawAll) {
        *cells = VisibleCells::Culled(Vec::new());
    }
    match cells {
        VisibleCells::Culled(list) => list,
        VisibleCells::DrawAll => unreachable!("replaced by an empty list above"),
    }
}

/// `a ∪ b` into `out`: a linear merge of two ascending, duplicate-free sets
/// that keeps `out` ascending and drops cells both hold.
pub(super) fn merge_ascending(a: &[u32], b: &[u32], out: &mut Vec<u32>) {
    debug_assert!(a.windows(2).all(|pair| pair[0] < pair[1]), "ascending");
    debug_assert!(b.windows(2).all(|pair| pair[0] < pair[1]), "ascending");
    out.clear();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            Ordering::Less => {
                out.push(a[i]);
                i += 1;
            }
            Ordering::Greater => {
                out.push(b[j]);
                j += 1;
            }
            Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out.extend_from_slice(&a[i..]);
    out.extend_from_slice(&b[j..]);
}
