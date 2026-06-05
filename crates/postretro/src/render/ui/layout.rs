// UI layout/projection: maps 1280x720 logical-reference rects to device-pixel
// quad instances. Pure CPU-side — no wgpu call, holds no GPU handle — so the
// produced draw list is GPU-independent and asserted by the CPU draw-list tests.
// See: context/plans/in-progress/M13--descriptor-tree-layout

use super::{UiDrawList, UiInstance};

/// Logical-reference width. All UI is authored against this 1280x720 canvas;
/// the device scale derives from `device_size / reference` at encode time.
pub const REFERENCE_WIDTH: f32 = 1280.0;
/// Logical-reference height (see `REFERENCE_WIDTH`).
pub const REFERENCE_HEIGHT: f32 = 720.0;

/// Anchor point within the logical-reference canvas that an element's `offset`
/// is measured from, and that the element's own pivot aligns to. Keeping the
/// pivot and reference point the same makes a `Center`-anchored element with a
/// zero offset land dead-center regardless of its size — the common splash case.
///
/// All nine variants are live: they serialize to/from the descriptor wire as the
/// placement envelope's `anchor` field, drive the whole-tree placement transform
/// (`tree::anchor_fractions`), and are exercised by the layout tests. The splash
/// itself only uses `Center`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    /// Fractional position of the anchor in `[0,1]` along each axis: x grows
    /// right, y grows down (top-left origin, matching device-pixel rects).
    fn fractions(self) -> (f32, f32) {
        let (fx, fy) = match self {
            Anchor::TopLeft => (0.0, 0.0),
            Anchor::Top => (0.5, 0.0),
            Anchor::TopRight => (1.0, 0.0),
            Anchor::Left => (0.0, 0.5),
            Anchor::Center => (0.5, 0.5),
            Anchor::Right => (1.0, 0.5),
            Anchor::BottomLeft => (0.0, 1.0),
            Anchor::Bottom => (0.5, 1.0),
            Anchor::BottomRight => (1.0, 1.0),
        };
        (fx, fy)
    }
}

/// One element placed in logical-reference space, before projection to device
/// pixels. The element's `anchor` is both the reference point on the canvas and
/// the element's own pivot; `offset` nudges it from there (logical px, +x right,
/// +y down). `size` and `margin` are logical-reference px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiElement {
    pub anchor: Anchor,
    /// Offset from the anchor point, logical-reference px.
    pub offset: [f32; 2],
    /// Element size, logical-reference px.
    pub size: [f32; 2],
    /// UV rect into the bound texture: `[u0, v0, u_width, v_height]`.
    pub uv_rect: [f32; 4],
    /// Linear RGBA tint.
    pub color: [f32; 4],
    /// 9-slice margin, logical-reference px: `[left, top, right, bottom]`.
    pub margin: [f32; 4],
}

impl UiElement {
    /// Solid-color panel: full white texel, optional 9-slice margin.
    pub fn panel(anchor: Anchor, offset: [f32; 2], size: [f32; 2], color: [f32; 4]) -> Self {
        Self {
            anchor,
            offset,
            size,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            color,
            margin: [0.0; 4],
        }
    }

    /// Solid-color panel with a 9-slice margin (logical-reference px). Now only
    /// the layout tests exercise this directly — the splash builds 9-slice panels
    /// through the descriptor tree's `Border`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn panel_9slice(
        anchor: Anchor,
        offset: [f32; 2],
        size: [f32; 2],
        color: [f32; 4],
        margin: [f32; 4],
    ) -> Self {
        Self {
            margin,
            ..Self::panel(anchor, offset, size, color)
        }
    }

    /// Textured image: full texture, untinted (white). Splash images now flow
    /// through the descriptor tree's `image` nodes; the layout tests still use
    /// this for the projection-path assertions.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn image(anchor: Anchor, offset: [f32; 2], size: [f32; 2]) -> Self {
        Self {
            anchor,
            offset,
            size,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            color: [1.0, 1.0, 1.0, 1.0],
            margin: [0.0; 4],
        }
    }
}

/// Uniform device scale derived from the backbuffer size against the
/// 1280x720 logical reference.
///
/// **Aspect-ratio handling:** a single uniform scale is used for both axes —
/// `min(device.x / ref.x, device.y / ref.y)`. Picking the smaller ratio means
/// the logical canvas is letterboxed (never cropped) when the backbuffer aspect
/// differs from 16:9, and crucially an element keeps its authored aspect at
/// every resolution. The plan requires "only the uniform device scale applies,
/// never an independent x/y stretch" — the logo must not stretch — so an
/// independent x/y scale is deliberately rejected.
pub fn device_scale(device_size: [u32; 2]) -> f32 {
    let sx = device_size[0] as f32 / REFERENCE_WIDTH;
    let sy = device_size[1] as f32 / REFERENCE_HEIGHT;
    sx.min(sy)
}

/// Logical canvas origin in device pixels: the top-left of the scaled
/// 1280x720 canvas, centered in the backbuffer so the letterbox margin is
/// split evenly. Combined with `device_scale` this maps logical-reference
/// coordinates to device pixels: `device = origin + logical * scale`.
fn canvas_origin(device_size: [u32; 2], scale: f32) -> [f32; 2] {
    let scaled_w = REFERENCE_WIDTH * scale;
    let scaled_h = REFERENCE_HEIGHT * scale;
    [
        (device_size[0] as f32 - scaled_w) * 0.5,
        (device_size[1] as f32 - scaled_h) * 0.5,
    ]
}

/// Project one logical-reference element to a device-pixel `UiInstance`, with
/// the rect and 9-slice margin snapped to whole device pixels. Snapping the
/// rect edges (left/top and right/bottom independently, so width is preserved
/// as `snapped_right - snapped_left`) avoids subpixel edge blur on panels and
/// images. Margins scale and snap the same way so the shader's corner regions
/// stay on pixel boundaries. Text is NOT routed through here — glyphon keeps AA
/// sub-pixel positions.
fn project_element(elem: &UiElement, device_size: [u32; 2], scale: f32) -> UiInstance {
    let origin = canvas_origin(device_size, scale);
    let (afx, afy) = elem.anchor.fractions();

    // Anchor point on the canvas, in logical-reference px, then the element's
    // own pivot (same fractions) backs the top-left out from there.
    let anchor_x = REFERENCE_WIDTH * afx + elem.offset[0];
    let anchor_y = REFERENCE_HEIGHT * afy + elem.offset[1];
    let logical_left = anchor_x - elem.size[0] * afx;
    let logical_top = anchor_y - elem.size[1] * afy;

    // Logical -> device, then snap each edge to a whole device pixel. Snapping
    // edges (not pos+size) keeps abutting elements gap-free and preserves the
    // on-screen width/height as the difference of two rounded edges.
    let dev_left = origin[0] + logical_left * scale;
    let dev_top = origin[1] + logical_top * scale;
    let dev_right = dev_left + elem.size[0] * scale;
    let dev_bottom = dev_top + elem.size[1] * scale;

    let x = dev_left.round();
    let y = dev_top.round();
    let w = dev_right.round() - x;
    let h = dev_bottom.round() - y;

    // 9-slice margins scale and round to whole device pixels so the shader's
    // fixed corner regions land on pixel boundaries.
    let margin = [
        (elem.margin[0] * scale).round(),
        (elem.margin[1] * scale).round(),
        (elem.margin[2] * scale).round(),
        (elem.margin[3] * scale).round(),
    ];

    UiInstance {
        rect: [x, y, w, h],
        uv_rect: elem.uv_rect,
        color: elem.color,
        margin,
    }
}

/// Project a slice of logical-reference elements into a device-pixel draw list.
/// Pure: takes logical inputs + the device backbuffer size, returns a
/// `UiDrawList` with no GPU interaction. Element order is preserved (draw order).
/// Callable from tests with no GPU context.
pub fn project(elements: &[UiElement], device_size: [u32; 2]) -> UiDrawList {
    let scale = device_scale(device_size);
    let mut list = UiDrawList::new();
    for elem in elements {
        list.push(project_element(elem, device_size, scale));
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tolerance for device-pixel comparisons. Projected rects are snapped to
    /// whole pixels, but float rounding can leave a sub-ulp residue.
    const EPS: f32 = 1e-3;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() <= EPS
    }

    fn assert_rect_approx(got: [f32; 4], want: [f32; 4]) {
        for i in 0..4 {
            assert!(
                approx(got[i], want[i]),
                "rect[{i}] = {} != {} (rect {got:?} vs {want:?})",
                got[i],
                want[i],
            );
        }
    }

    #[test]
    fn scale_is_unity_at_reference_resolution() {
        assert!(approx(device_scale([1280, 720]), 1.0));
    }

    #[test]
    fn scale_is_three_at_4k() {
        // 3840x2160 is exactly 3x the 1280x720 reference on both axes.
        assert!(approx(device_scale([3840, 2160]), 3.0));
    }

    #[test]
    fn scale_is_uniform_and_letterboxes_on_mismatched_aspect() {
        // 1280x1440: x ratio 1.0, y ratio 2.0. Uniform scale takes the min, so
        // the logo never stretches — scale is 1.0 and the canvas letterboxes
        // vertically.
        assert!(approx(device_scale([1280, 1440]), 1.0));
        // 2560x720: x ratio 2.0, y ratio 1.0 -> min 1.0.
        assert!(approx(device_scale([2560, 720]), 1.0));
    }

    #[test]
    fn center_anchor_zero_offset_lands_centered_at_reference_res() {
        // A 200x100 panel centered with no offset sits at the canvas center
        // minus half its size: ((1280-200)/2, (720-100)/2) = (540, 310).
        let elem = UiElement::panel(Anchor::Center, [0.0, 0.0], [200.0, 100.0], [1.0; 4]);
        let list = project(&[elem], [1280, 720]);
        assert_eq!(list.len(), 1);
        assert_rect_approx(list.instances[0].rect, [540.0, 310.0, 200.0, 100.0]);
    }

    #[test]
    fn top_left_anchor_with_offset_places_from_origin() {
        // TopLeft pivot: top-left = anchor + offset = (0,0) + (32,16).
        let elem = UiElement::panel(Anchor::TopLeft, [32.0, 16.0], [64.0, 48.0], [1.0; 4]);
        let list = project(&[elem], [1280, 720]);
        assert_rect_approx(list.instances[0].rect, [32.0, 16.0, 64.0, 48.0]);
    }

    #[test]
    fn bottom_right_anchor_places_against_far_corner() {
        // BottomRight pivot at canvas corner (1280,720): top-left =
        // (1280 - 100, 720 - 40) = (1180, 680).
        let elem = UiElement::panel(Anchor::BottomRight, [0.0, 0.0], [100.0, 40.0], [1.0; 4]);
        let list = project(&[elem], [1280, 720]);
        assert_rect_approx(list.instances[0].rect, [1180.0, 680.0, 100.0, 40.0]);
    }

    #[test]
    fn center_panel_scales_uniformly_at_4k() {
        // At 3x, a centered 200x100 panel doubles position and size by 3.
        // Reference top-left (540,310) -> device (1620,930); size 600x300.
        let elem = UiElement::panel(Anchor::Center, [0.0, 0.0], [200.0, 100.0], [1.0; 4]);
        let list = project(&[elem], [3840, 2160]);
        assert_rect_approx(list.instances[0].rect, [1620.0, 930.0, 600.0, 300.0]);
    }

    #[test]
    fn rects_snap_to_integer_device_pixels() {
        // A scale that produces fractional edges must round to whole pixels.
        // Scale at 1281x721 is min(1281/1280, 721/720) = 1281/1280 ~ 1.00078.
        // We only assert the result is integer-valued, not the exact value.
        let elem = UiElement::panel(Anchor::TopLeft, [10.5, 20.3], [33.7, 50.9], [1.0; 4]);
        let list = project(&[elem], [1281, 721]);
        let r = list.instances[0].rect;
        for v in r {
            assert!(
                approx(v, v.round()),
                "rect component {v} is not snapped to a whole device pixel",
            );
        }
    }

    #[test]
    fn nine_slice_margins_scale_with_device() {
        // 8px logical margins at 3x device scale -> 24px device margins.
        let elem = UiElement::panel_9slice(
            Anchor::Center,
            [0.0, 0.0],
            [200.0, 100.0],
            [1.0; 4],
            [8.0, 8.0, 8.0, 8.0],
        );
        let list = project(&[elem], [3840, 2160]);
        assert_rect_approx(list.instances[0].margin, [24.0, 24.0, 24.0, 24.0]);
    }

    #[test]
    fn nine_slice_corner_rects_preserve_size_under_scale() {
        // Corners derive from the scaled rect + scaled margin; at 3x a 8px
        // corner becomes 24px and stays anchored to the rect corners. Exercises
        // the `corner_rects` derivation against scaled instances.
        let elem = UiElement::panel_9slice(
            Anchor::Center,
            [0.0, 0.0],
            [200.0, 100.0],
            [1.0; 4],
            [8.0, 8.0, 8.0, 8.0],
        );
        let list = project(&[elem], [3840, 2160]);
        let inst = list.instances[0];
        let [tl, tr, bl, br] = inst.corner_rects();
        let (x, y, w, h) = (inst.rect[0], inst.rect[1], inst.rect[2], inst.rect[3]);
        assert_rect_approx(tl, [x, y, 24.0, 24.0]);
        assert_rect_approx(tr, [x + w - 24.0, y, 24.0, 24.0]);
        assert_rect_approx(bl, [x, y + h - 24.0, 24.0, 24.0]);
        assert_rect_approx(br, [x + w - 24.0, y + h - 24.0, 24.0, 24.0]);
    }

    #[test]
    fn uv_and_color_pass_through_unchanged() {
        let mut elem = UiElement::image(Anchor::Center, [0.0, 0.0], [50.0, 50.0]);
        elem.uv_rect = [0.1, 0.2, 0.3, 0.4];
        elem.color = [0.5, 0.6, 0.7, 0.8];
        let list = project(&[elem], [1280, 720]);
        assert_rect_approx(list.instances[0].uv_rect, [0.1, 0.2, 0.3, 0.4]);
        assert_rect_approx(list.instances[0].color, [0.5, 0.6, 0.7, 0.8]);
    }

    #[test]
    fn draw_order_is_preserved() {
        let a = UiElement::panel(
            Anchor::TopLeft,
            [0.0, 0.0],
            [10.0, 10.0],
            [1.0, 0.0, 0.0, 1.0],
        );
        let b = UiElement::panel(
            Anchor::TopLeft,
            [0.0, 0.0],
            [10.0, 10.0],
            [0.0, 1.0, 0.0, 1.0],
        );
        let list = project(&[a, b], [1280, 720]);
        assert_eq!(list.len(), 2);
        assert_rect_approx(list.instances[0].color, [1.0, 0.0, 0.0, 1.0]);
        assert_rect_approx(list.instances[1].color, [0.0, 1.0, 0.0, 1.0]);
    }
}
