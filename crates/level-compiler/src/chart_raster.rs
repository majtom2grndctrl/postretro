// Shared chart-rasterization math: a chart texel's world position and soft-visibility seed.
// See: context/lib/build_pipeline.md §Compiler pipeline (atlas preparation)

use glam::Vec3;

use crate::lightmap_bake::Chart;

/// Padding inserted around each chart in atlas texels. One texel of padding
/// plus the post-bake edge-dilation pass keeps bilinear sampling from dragging
/// black into chart interiors.
///
/// Public because the animated weight-map baker needs to resolve a chunk's
/// atlas-texel rectangle from its owning face's placement, and the placement's
/// interior offset is `(placement + CHART_PADDING_TEXELS, ...)`.
pub const CHART_PADDING_TEXELS: u32 = 2;

/// Where a chart landed in the atlas, in texel coordinates.
///
/// The chart keeps its own texel grid; only where its texels sit changes. An
/// untransposed chart's padded-local texel `(x, y)` (chart row-major, padding
/// included) sits at atlas `(self.x + x, self.y + y)`; a transposed chart's at
/// `(self.x + y, self.y + x)`, so its placed rect is `height × width`. Every
/// chart-to-atlas mapping goes through the methods below, and every walk over
/// a chart's texels stays in the chart's own row-major order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChartPlacement {
    pub x: u32,
    pub y: u32,
    /// Internal bake layer this chart landed on: a compiler working plane that
    /// never ships. Charts pack per cell block, and blocks pack into layers.
    pub layer: u32,
    /// The chart's rows run down the atlas's columns.
    pub transposed: bool,
}

impl ChartPlacement {
    /// An untransposed placement.
    pub const fn at(x: u32, y: u32, layer: u32) -> Self {
        Self {
            x,
            y,
            layer,
            transposed: false,
        }
    }

    /// A chart-oriented pair `(along the chart's rows, along its columns)` in
    /// atlas orientation; applied twice it is the identity.
    pub fn orient<T>(&self, a: T, b: T) -> (T, T) {
        if self.transposed { (b, a) } else { (a, b) }
    }

    /// The atlas extent of a `width × height` padded chart.
    pub fn placed_extent(&self, width: u32, height: u32) -> (u32, u32) {
        self.orient(width, height)
    }

    /// Atlas texel of padded-local chart texel `(x, y)`.
    pub fn padded_texel(&self, x: u32, y: u32) -> (u32, u32) {
        let (dx, dy) = self.orient(x, y);
        (self.x + dx, self.y + dy)
    }

    /// Atlas texel of chart-interior texel `(tx, ty)`, one padding in.
    pub fn interior_texel(&self, tx: i32, ty: i32) -> (i32, i32) {
        let padding = CHART_PADDING_TEXELS as i32;
        let (dx, dy) = self.orient(padding + tx, padding + ty);
        (self.x as i32 + dx, self.y as i32 + dy)
    }

    /// Chart-interior texel of atlas texel `(x, y)`: the inverse of
    /// [`Self::interior_texel`].
    pub fn interior_of(&self, x: u32, y: u32) -> (i32, i32) {
        let padding = CHART_PADDING_TEXELS as i32;
        let (dx, dy) = (x as i32 - self.x as i32, y as i32 - self.y as i32);
        let (cx, cy) = self.orient(dx, dy);
        (cx - padding, cy - padding)
    }
}

/// Interior (non-padded) dimensions of a chart in atlas texels. Clamped to at
/// least 1 so a degenerate chart still maps to one texel.
pub fn chart_interior_dims(chart: &Chart) -> (i32, i32) {
    let padding = CHART_PADDING_TEXELS as i32;
    let iw = (chart.width_texels as i32 - 2 * padding).max(1);
    let ih = (chart.height_texels as i32 - 2 * padding).max(1);
    (iw, ih)
}

/// The grid `chart`'s texels sit on: its UV rect, its interior texel extent,
/// and the grid texel of its first interior texel. A chart is its own grid
/// unless it is a sub-chart window onto its parent's.
pub(crate) struct ChartGrid {
    pub uv_min: [f32; 2],
    pub uv_extent: [f32; 2],
    pub interior: [i32; 2],
    pub origin: [i32; 2],
}

pub(crate) fn chart_grid(chart: &Chart) -> ChartGrid {
    match chart.window {
        None => {
            let (iw, ih) = chart_interior_dims(chart);
            ChartGrid {
                uv_min: chart.uv_min,
                uv_extent: chart.uv_extent,
                interior: [iw, ih],
                origin: [0, 0],
            }
        }
        Some(window) => ChartGrid {
            uv_min: window.grid_uv_min,
            uv_extent: window.grid_uv_extent,
            interior: [
                window.grid_interior[0] as i32,
                window.grid_interior[1] as i32,
            ],
            origin: [window.origin[0] as i32, window.origin[1] as i32],
        },
    }
}

/// World-space position of the texel at interior coordinates `(tx, ty)` of
/// `chart`, in `[0, interior_w) × [0, interior_h)`, computed from its grid
/// index so a sub-chart's texel matches its parent's bit for bit.
///
/// Matches the frozen lightmap reference's per-texel derivation exactly —
/// every baker routes its world-position lookups through this function so
/// they agree on texel centres at chunk and cut boundaries.
pub fn chart_texel_world_position(chart: &Chart, tx: i32, ty: i32) -> Vec3 {
    let grid = chart_grid(chart);
    let u_frac = ((grid.origin[0] + tx) as f32 + 0.5) / grid.interior[0] as f32;
    let v_frac = ((grid.origin[1] + ty) as f32 + 0.5) / grid.interior[1] as f32;
    let local_u = grid.uv_min[0] + u_frac * grid.uv_extent[0];
    let local_v = grid.uv_min[1] + v_frac * grid.uv_extent[1];
    chart.origin + chart.u_axis * local_u + chart.v_axis * local_v
}

/// Soft-visibility sample-lattice seed of interior texel `(tx, ty)` of
/// `chart`'s grid, shared by the static, shadowmask and animated bakes.
///
/// Keyed on the chart's world-space frame (origin and axes, exact `f32`
/// bits, a sub-chart sharing its parent's) and the grid texel, so both sides
/// of a cut draw the same samples, and never on its bake-layer placement or its
/// face index: a chart that moves in the atlas, or keeps its surface while an
/// unrelated edit renumbers faces, draws the same samples and bakes the same
/// texels. A fixed integer mix (FNV-1a over the words, then the SplitMix64
/// finalizer), never a per-process hash, so separate runs agree byte for byte.
pub fn chart_texel_seed(chart: &Chart, tx: i32, ty: i32) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let [window_x, window_y] = chart.window.map_or([0, 0], |window| {
        [window.origin[0] as i32, window.origin[1] as i32]
    });
    let frame = [chart.origin, chart.u_axis, chart.v_axis];
    let words = frame
        .iter()
        .flat_map(|v| v.to_array())
        .map(f32::to_bits)
        .chain([(window_x + tx) as u32, (window_y + ty) as u32]);
    let mut z = words.fold(FNV_OFFSET, |h, word| {
        (h ^ u64::from(word)).wrapping_mul(FNV_PRIME)
    });
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}
