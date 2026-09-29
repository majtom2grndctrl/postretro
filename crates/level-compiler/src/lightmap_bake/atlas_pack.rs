// Group-cohesive multi-layer MaxRects packing: cell blocks into bake layers, charts for the dry run.
// See: context/lib/build_pipeline.md §Compiler pipeline

use super::MIN_ATLAS_DIMENSION;
#[cfg(test)]
use super::charts::Chart;
#[cfg(test)]
use super::{LightmapBakeError, MAX_ATLAS_LAYERS};
use crate::chart_raster::ChartPlacement;

/// Result of multi-bin atlas packing. All layers share one `(atlas_width,
/// atlas_height)` — a `texture_2d_array` has a single per-layer dimension across
/// every layer — and each placement carries the layer its chart landed on.
/// `placements` is parallel to the input `charts`.
#[cfg(test)]
#[derive(Debug)]
pub struct PackOutput {
    pub layer_count: u32,
    pub atlas_width: u32,
    pub atlas_height: u32,
    pub placements: Vec<ChartPlacement>,
}

/// Round a raw texel extent up to the packer's dimension contract: power-of-two,
/// a multiple of 4 (BC6H block alignment — a pow2 ≥ 4 is 4-aligned for free),
/// and within `[MIN_ATLAS_DIMENSION, max_dim]`.
fn round_atlas_dim(raw: u32, max_dim: u32) -> u32 {
    raw.max(MIN_ATLAS_DIMENSION)
        .next_power_of_two()
        .min(max_dim)
}

/// Pack charts into a multi-layer atlas with leaf-aware MaxRects binning.
///
/// `max_dim` bounds each layer's width and height (production passes
/// [`MAX_ATLAS_DIMENSION`]; tests pass a small value). Every chart's largest
/// side must be `≤ max_dim` or [`LightmapBakeError::ChartTooLarge`] is returned
/// (this is the single source of truth for that check). The number of layers is
/// capped at [`MAX_ATLAS_LAYERS`]; exceeding it yields
/// [`LightmapBakeError::LayerOverflow`].
///
/// Leaf cohesion is a hard invariant: all charts of one BVH leaf land on a
/// single layer (a leaf is the runtime draw/visibility unit, so straddling a
/// layer boundary would force a per-face layer switch in the hot path). A leaf
/// is packed as a unit into the current layer's free rectangles; if it doesn't
/// all fit, the WHOLE leaf rolls to a fresh layer — partial placements from the
/// failed attempt are discarded.
///
/// The shared `(atlas_width, atlas_height)` is sized to host the largest single
/// leaf in one layer (grown by doubling, capped at `max_dim`), so no leaf is
/// ever forced to split for want of room within a layer.
///
/// Production packs cell blocks instead (`block_layout::pack_cell_blocks`);
/// this chart-level packer remains for the residency dry run and tests.
#[cfg(test)]
pub(crate) fn pack_layers(
    charts: &[Chart],
    max_dim: u32,
    density_m_per_texel: f32,
) -> Result<PackOutput, LightmapBakeError> {
    pack_layers_with_layer_limit(charts, max_dim, MAX_ATLAS_LAYERS, density_m_per_texel)
}

/// [`pack_layers`] with an explicit layer ceiling in place of
/// [`MAX_ATLAS_LAYERS`]. Production always goes through `pack_layers`; the
/// lightmap residency dry run raises the ceiling so a small capped layer size
/// can be measured even when its layer count would exceed the runtime floor.
#[cfg(test)]
pub(crate) fn pack_layers_with_layer_limit(
    charts: &[Chart],
    max_dim: u32,
    max_layers: u32,
    density_m_per_texel: f32,
) -> Result<PackOutput, LightmapBakeError> {
    if charts.is_empty() {
        return Ok(PackOutput {
            layer_count: 1,
            atlas_width: MIN_ATLAS_DIMENSION,
            atlas_height: MIN_ATLAS_DIMENSION,
            placements: Vec::new(),
        });
    }

    // ChartTooLarge is now owned here: a single chart wider/taller than a layer
    // can never be placed, regardless of how many layers we open.
    for (face_index, chart) in charts.iter().enumerate() {
        if chart.width_texels > max_dim || chart.height_texels > max_dim {
            return Err(LightmapBakeError::ChartTooLarge {
                face_index,
                width_texels: chart.width_texels,
                height_texels: chart.height_texels,
                max: max_dim,
                u_extent_m: chart.uv_extent[0],
                v_extent_m: chart.uv_extent[1],
                density_m_per_texel,
            });
        }
    }

    // Group chart indices by leaf, preserving first-seen order so packing is
    // deterministic (no HashMap iteration leaking into placement). Each group is
    // a contiguous unit the packer places together.
    let leaves = group_charts_by_leaf(charts);
    let sizes: Vec<(u32, u32)> = charts
        .iter()
        .map(|chart| (chart.width_texels, chart.height_texels))
        .collect();
    let pack =
        pack_groups_into_layers(&sizes, &leaves, max_dim, max_layers).map_err(
            |error| match error {
                GroupPackError::LayerOverflow { layer_count, max } => {
                    LightmapBakeError::LayerOverflow { layer_count, max }
                }
                GroupPackError::GroupTooLarge { group } => LightmapBakeError::LeafTooLarge {
                    leaf_index: leaves[group]
                        .first()
                        .map(|&i| charts[i].leaf_index)
                        .unwrap_or(0),
                    chart_count: leaves[group].len(),
                    max_dim,
                },
            },
        )?;
    Ok(PackOutput {
        layer_count: pack.layer_count,
        atlas_width: pack.dim,
        atlas_height: pack.dim,
        placements: pack.placements,
    })
}

/// Square, uniform layers holding rects packed group by group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LayerPack {
    pub layer_count: u32,
    /// Edge of every layer: a power of two in `[MIN_ATLAS_DIMENSION, max_dim]`.
    pub dim: u32,
    /// Parallel to the input sizes.
    pub placements: Vec<ChartPlacement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GroupPackError {
    LayerOverflow {
        layer_count: u32,
        max: u32,
    },
    /// Group `group` cannot fit one empty `max_dim²` layer.
    GroupTooLarge {
        group: usize,
    },
}

/// Pack `sizes` into uniform square layers, one group at a time in the given
/// order. A group is cohesive: it packs into the current layer's free
/// rectangles, largest rect first, or rolls whole to a fresh layer; earlier
/// layers are never revisited. The layer edge is the smallest power of two
/// that hosts the largest group alone.
///
/// Every placement is a sum of rect extents and zero, so rects whose extents
/// are multiples of some `k` land on multiples of `k` (the layer edge is a
/// power of two at least 64).
pub(crate) fn pack_groups_into_layers(
    sizes: &[(u32, u32)],
    groups: &[Vec<usize>],
    max_dim: u32,
    max_layers: u32,
) -> Result<LayerPack, GroupPackError> {
    if sizes.is_empty() {
        return Ok(LayerPack {
            layer_count: 1,
            dim: MIN_ATLAS_DIMENSION,
            placements: Vec::new(),
        });
    }

    // Size the shared per-layer dimension to fit the largest group in a single
    // layer. Start from a square that covers each group's total area and
    // largest side, then grow (doubling) until every group packs alone.
    let atlas_dim = choose_layer_dim(sizes, groups, max_dim);

    // Pack groups into layers. Each group tries the current layer's MaxRects
    // free list; a group that doesn't fit rolls whole to a new layer.
    let mut placements = vec![
        ChartPlacement {
            x: 0,
            y: 0,
            layer: 0
        };
        sizes.len()
    ];
    let mut layer: u32 = 0;
    let mut packer = MaxRects::new(atlas_dim, atlas_dim);

    for (group_index, group) in groups.iter().enumerate() {
        if !place_group(&mut packer, sizes, group, layer, &mut placements) {
            // The group didn't fit in the current layer — open a fresh one and
            // place the whole group there. Sizing guarantees a group fits an
            // empty layer, so this single retry always succeeds.
            layer += 1;
            if layer >= max_layers {
                return Err(GroupPackError::LayerOverflow {
                    layer_count: layer + 1,
                    max: max_layers,
                });
            }
            packer = MaxRects::new(atlas_dim, atlas_dim);
            if !place_group(&mut packer, sizes, group, layer, &mut placements) {
                // A single group too large to fit even an empty `max_dim²`
                // layer can never be placed, and cohesion forbids splitting it.
                // Error rather than `debug_assert!` so release builds reject it
                // instead of silently leaving its rects at `{0,0,N}`.
                return Err(GroupPackError::GroupTooLarge { group: group_index });
            }
        }
    }

    Ok(LayerPack {
        layer_count: layer + 1,
        dim: atlas_dim,
        placements,
    })
}

/// Group chart indices by `leaf_index`, preserving first-seen leaf order. Charts
/// arrive leaf-ordered from `extract_geometry`, so this is usually contiguous,
/// but the grouping does not rely on that.
#[cfg(test)]
fn group_charts_by_leaf(charts: &[Chart]) -> Vec<Vec<usize>> {
    let mut order: Vec<u32> = Vec::new();
    let mut groups: std::collections::HashMap<u32, Vec<usize>> = std::collections::HashMap::new();
    for (i, c) in charts.iter().enumerate() {
        groups.entry(c.leaf_index).or_insert_with(|| {
            order.push(c.leaf_index);
            Vec::new()
        });
        groups.get_mut(&c.leaf_index).unwrap().push(i);
    }
    order
        .into_iter()
        .map(|leaf| groups.remove(&leaf).unwrap())
        .collect()
}

/// Choose the shared per-layer square dimension. It must host the largest single
/// group in one layer, so we grow (doubling, 4-aligned pow2, capped at `max_dim`)
/// until each group packs alone via MaxRects.
fn choose_layer_dim(sizes: &[(u32, u32)], groups: &[Vec<usize>], max_dim: u32) -> u32 {
    // Lower bound from the densest group: its total area and its widest/tallest
    // rect both have to fit one layer.
    let mut min_side = MIN_ATLAS_DIMENSION;
    for group in groups {
        let area: u64 = group
            .iter()
            .map(|&i| sizes[i].0 as u64 * sizes[i].1 as u64)
            .sum();
        let side_from_area = (area as f64).sqrt().ceil() as u32;
        let max_side = group
            .iter()
            .map(|&i| sizes[i].0.max(sizes[i].1))
            .max()
            .unwrap_or(0);
        min_side = min_side.max(side_from_area).max(max_side);
    }

    let mut dim = round_atlas_dim(min_side, max_dim);
    loop {
        // A group fits this dimension if MaxRects places all its rects in one
        // empty layer. The area lower bound is optimistic (ignores
        // fragmentation), so confirm with a real pack and grow on overflow.
        let all_fit = groups.iter().all(|group| {
            let mut packer = MaxRects::new(dim, dim);
            group
                .iter()
                .all(|&i| packer.insert(sizes[i].0, sizes[i].1).is_some())
        });
        if all_fit || dim >= max_dim {
            return dim;
        }
        dim = round_atlas_dim(dim + 1, max_dim);
    }
}

/// Try to place all of `group`'s rects into `packer` (the current layer). On
/// success, writes each placement at `layer` and returns `true`. On the first
/// rect that doesn't fit, returns `false` WITHOUT mutating `placements` for the
/// rects it did place — the caller rolls the whole group to a fresh layer, so
/// any partial work in `packer` is discarded with the packer itself.
fn place_group(
    packer: &mut MaxRects,
    sizes: &[(u32, u32)],
    group: &[usize],
    layer: u32,
    placements: &mut [ChartPlacement],
) -> bool {
    // Largest-first within the group packs big rects before the free list
    // fragments — the standard MaxRects ordering for density.
    let mut order: Vec<usize> = group.to_vec();
    order.sort_by(|&a, &b| {
        let area_a = sizes[a].0 as u64 * sizes[a].1 as u64;
        let area_b = sizes[b].0 as u64 * sizes[b].1 as u64;
        area_b
            .cmp(&area_a)
            // Tie-break on input index so the order is fully deterministic.
            .then(a.cmp(&b))
    });

    let mut staged: Vec<(usize, u32, u32)> = Vec::with_capacity(order.len());
    for &i in &order {
        match packer.insert(sizes[i].0, sizes[i].1) {
            Some((x, y)) => staged.push((i, x, y)),
            None => return false,
        }
    }
    for (i, x, y) in staged {
        placements[i] = ChartPlacement { x, y, layer };
    }
    true
}

/// MaxRects free-rectangle bin packer for one atlas layer. Maintains a list of
/// maximal free rectangles; each insert picks the best-short-side-fit free rect,
/// splits it, and prunes any free rects now contained in another. This is the
/// genuine MaxRects algorithm (Jylänki 2010), not a shelf fallback, so it reaches
/// the 85–95% density target on mixed chart sets.
pub(crate) struct MaxRects {
    free: Vec<Rect>,
}

#[derive(Clone, Copy)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

impl MaxRects {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        MaxRects {
            free: vec![Rect {
                x: 0,
                y: 0,
                w: width,
                h: height,
            }],
        }
    }

    /// Place a `w × h` rectangle, returning its top-left `(x, y)` on success.
    /// Best-Short-Side-Fit: among free rects that can host it, pick the one
    /// leaving the smallest leftover short side (ties broken by long side, then
    /// position) for a deterministic, dense placement.
    pub(crate) fn insert(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let mut best: Option<(usize, u32, u32)> = None; // (free idx, short leftover, long leftover)
        for (idx, r) in self.free.iter().enumerate() {
            if r.w >= w && r.h >= h {
                let leftover_x = r.w - w;
                let leftover_y = r.h - h;
                let short = leftover_x.min(leftover_y);
                let long = leftover_x.max(leftover_y);
                let better = match best {
                    None => true,
                    Some((_, bs, bl)) => short < bs || (short == bs && long < bl),
                };
                if better {
                    best = Some((idx, short, long));
                }
            }
        }

        let (idx, _, _) = best?;
        let placed = Rect {
            x: self.free[idx].x,
            y: self.free[idx].y,
            w,
            h,
        };

        // Split every free rect that overlaps the placed rect into the maximal
        // sub-rects that don't, then prune any free rect contained in another.
        let mut i = 0;
        while i < self.free.len() {
            if let Some(splits) = split_free(&self.free[i], &placed) {
                self.free.swap_remove(i);
                self.free.extend(splits);
            } else {
                i += 1;
            }
        }
        self.prune();

        Some((placed.x, placed.y))
    }

    /// Drop any free rect fully contained in another — the maximality invariant
    /// MaxRects relies on (without it the free list grows unbounded with
    /// redundant rects).
    fn prune(&mut self) {
        let mut i = 0;
        while i < self.free.len() {
            let mut removed = false;
            let mut j = 0;
            while j < self.free.len() {
                if i != j && contains(&self.free[j], &self.free[i]) {
                    self.free.swap_remove(i);
                    removed = true;
                    break;
                }
                j += 1;
            }
            if !removed {
                i += 1;
            }
        }
    }
}

/// True if `outer` fully contains `inner`.
fn contains(outer: &Rect, inner: &Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x + inner.w <= outer.x + outer.w
        && inner.y + inner.h <= outer.y + outer.h
}

/// If `placed` overlaps `free`, return the maximal sub-rects of `free` that lie
/// outside `placed` (up to four: left, right, top, bottom slabs). Returns `None`
/// when they don't overlap — `free` is left untouched.
fn split_free(free: &Rect, placed: &Rect) -> Option<Vec<Rect>> {
    let no_overlap = placed.x >= free.x + free.w
        || placed.x + placed.w <= free.x
        || placed.y >= free.y + free.h
        || placed.y + placed.h <= free.y;
    if no_overlap {
        return None;
    }

    let mut out = Vec::with_capacity(4);
    // Left slab.
    if placed.x > free.x {
        out.push(Rect {
            x: free.x,
            y: free.y,
            w: placed.x - free.x,
            h: free.h,
        });
    }
    // Right slab.
    if placed.x + placed.w < free.x + free.w {
        out.push(Rect {
            x: placed.x + placed.w,
            y: free.y,
            w: (free.x + free.w) - (placed.x + placed.w),
            h: free.h,
        });
    }
    // Bottom slab.
    if placed.y > free.y {
        out.push(Rect {
            x: free.x,
            y: free.y,
            w: free.w,
            h: placed.y - free.y,
        });
    }
    // Top slab.
    if placed.y + placed.h < free.y + free.h {
        out.push(Rect {
            x: free.x,
            y: placed.y + placed.h,
            w: free.w,
            h: (free.y + free.h) - (placed.y + placed.h),
        });
    }
    Some(out)
}
