// UI layout/projection: maps logical-reference rects (1280x720 scale basis,
// widened to the backbuffer aspect) to device-pixel quad instances. Pure CPU-side — no wgpu call, holds no GPU handle — so the
// produced draw list is GPU-independent and asserted by the CPU draw-list tests.
// See: context/lib/ui.md

use super::{UiDrawList, UiInstance};

pub use postretro_scripting_core::ui::layout::Anchor;

/// Logical-reference width. UI is authored in logical px whose scale basis is
/// this 1280x720 reference: the device scale derives from `device_size /
/// reference` at encode time, and the logical canvas widens past it on the
/// axis the backbuffer has spare (see [`logical_canvas_size`]).
pub const REFERENCE_WIDTH: f32 = 1280.0;
/// Logical-reference height (see `REFERENCE_WIDTH`).
pub const REFERENCE_HEIGHT: f32 = 720.0;

fn anchor_fractions(anchor: Anchor) -> (f32, f32) {
    match anchor {
        Anchor::TopLeft => (0.0, 0.0),
        Anchor::Top => (0.5, 0.0),
        Anchor::TopRight => (1.0, 0.0),
        Anchor::Left => (0.0, 0.5),
        Anchor::Center => (0.5, 0.5),
        Anchor::Right => (1.0, 0.5),
        Anchor::BottomLeft => (0.0, 1.0),
        Anchor::Bottom => (0.5, 1.0),
        Anchor::BottomRight => (1.0, 1.0),
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
    /// layout tests exercise this directly; runtime UI projects descriptor
    /// borders through the retained tree path.
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
/// `min(device.x / ref.x, device.y / ref.y)` — so an element keeps its authored
/// aspect at every resolution and never stretches. The axis that bounds the
/// scale spans exactly the reference size; the other axis widens the logical
/// canvas (see [`logical_canvas_size`]) instead of leaving a letterbox margin.
pub fn device_scale(device_size: [u32; 2]) -> f32 {
    let sx = device_size[0] as f32 / REFERENCE_WIDTH;
    let sy = device_size[1] as f32 / REFERENCE_HEIGHT;
    sx.min(sy)
}

/// Logical size of the whole device backbuffer, in logical-reference px:
/// `device_size / device_scale`. The axis that bounds the scale is exactly the
/// reference size; the other is at least its reference size (wider on a wide
/// window, taller on a tall one). At 16:9 this is exactly 1280x720.
///
/// The canvas starts at the backbuffer's top-left, so `device = logical *
/// scale`. Anchors resolve against this size, which puts a corner anchor on
/// the backbuffer's corner at every aspect. A degenerate (zero-area) backbuffer
/// reports the reference size so layout never sees an infinite extent.
pub fn logical_canvas_size(device_size: [u32; 2]) -> [f32; 2] {
    let (w, h) = (device_size[0] as f32, device_size[1] as f32);
    let sx = w / REFERENCE_WIDTH;
    let sy = h / REFERENCE_HEIGHT;
    if !(sx > 0.0 && sy > 0.0 && sx.is_finite() && sy.is_finite()) {
        return [REFERENCE_WIDTH, REFERENCE_HEIGHT];
    }
    // The bounding axis is pinned to the reference value rather than divided
    // back out, so an exact 16:9 backbuffer yields exactly 1280x720.
    if sx > sy {
        [(w / sy).max(REFERENCE_WIDTH), REFERENCE_HEIGHT]
    } else if sy > sx {
        [REFERENCE_WIDTH, (h / sx).max(REFERENCE_HEIGHT)]
    } else {
        [REFERENCE_WIDTH, REFERENCE_HEIGHT]
    }
}

/// Project one logical-reference element to a device-pixel `UiInstance`, with
/// the rect and 9-slice margin snapped to whole device pixels. Snapping the
/// rect edges (left/top and right/bottom independently, so width is preserved
/// as `snapped_right - snapped_left`) avoids subpixel edge blur on panels and
/// images. Margins scale and snap the same way so the shader's corner regions
/// stay on pixel boundaries. Text is NOT routed through here — text rendering
/// keeps sub-pixel positions. Test-only since the boot splash stopped projecting
/// through `layout`.
fn project_element(elem: &UiElement, device_size: [u32; 2], scale: f32) -> UiInstance {
    let canvas = logical_canvas_size(device_size);
    let (afx, afy) = anchor_fractions(elem.anchor);

    // Anchor point on the canvas, in logical-reference px, then the element's
    // own pivot (same fractions) backs the top-left out from there.
    let anchor_x = canvas[0] * afx + elem.offset[0];
    let anchor_y = canvas[1] * afy + elem.offset[1];
    let logical_left = anchor_x - elem.size[0] * afx;
    let logical_top = anchor_y - elem.size[1] * afy;

    // Logical -> device, then snap each edge to a whole device pixel. Snapping
    // edges (not pos+size) keeps abutting elements gap-free and preserves the
    // on-screen width/height as the difference of two rounded edges.
    let dev_left = logical_left * scale;
    let dev_top = logical_top * scale;
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
/// Callable from tests with no GPU context. Test-only now: the boot splash no
/// longer projects through this path.
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
    fn scale_is_uniform_on_mismatched_aspect() {
        // 1280x1440: x ratio 1.0, y ratio 2.0. Uniform scale takes the min, so
        // nothing stretches — scale is 1.0.
        assert!(approx(device_scale([1280, 1440]), 1.0));
        // 2560x720: x ratio 2.0, y ratio 1.0 -> min 1.0.
        assert!(approx(device_scale([2560, 720]), 1.0));
    }

    #[test]
    fn logical_canvas_is_exactly_the_reference_at_16_9() {
        for size in [
            [1280, 720],
            [1920, 1080],
            [2560, 1440],
            [3840, 2160],
            [1600, 900],
            [16, 9],
        ] {
            assert_eq!(
                logical_canvas_size(size),
                [REFERENCE_WIDTH, REFERENCE_HEIGHT],
                "{size:?}"
            );
        }
    }

    #[test]
    fn logical_canvas_widens_the_spare_axis() {
        // Wide: height bounds the scale, width grows.
        let [w, h] = logical_canvas_size([2560, 1080]);
        assert!(approx(h, 720.0) && approx(w, 2560.0 / 1.5), "{w}x{h}");
        let [w, h] = logical_canvas_size([3440, 1440]);
        assert!(approx(h, 720.0) && approx(w, 1720.0), "{w}x{h}");
        // Tall / narrow: width bounds the scale, height grows.
        let [w, h] = logical_canvas_size([1080, 1920]);
        assert!(
            approx(w, 1280.0) && approx(h, 1920.0 * 1280.0 / 1080.0),
            "{w}x{h}"
        );
        let [w, h] = logical_canvas_size([1000, 1000]);
        assert!(approx(w, 1280.0) && approx(h, 1280.0), "{w}x{h}");
        // Degenerate backbuffer: the reference, never an infinite extent.
        assert_eq!(logical_canvas_size([0, 720]), [1280.0, 720.0]);
        assert_eq!(logical_canvas_size([0, 0]), [1280.0, 720.0]);
    }

    #[test]
    fn corner_anchors_land_on_backbuffer_corners_on_wide_and_tall_windows() {
        for device in [[2560, 1080], [3440, 1440], [1080, 1920], [1000, 1000]] {
            let scale = device_scale(device);
            let (dw, dh) = (device[0] as f32, device[1] as f32);
            let size = [100.0, 40.0];
            let (sw, sh) = ((size[0] * scale).round(), (size[1] * scale).round());
            let tl = project(
                &[UiElement::panel(Anchor::TopLeft, [0.0; 2], size, [1.0; 4])],
                device,
            );
            let br = project(
                &[UiElement::panel(
                    Anchor::BottomRight,
                    [0.0; 2],
                    size,
                    [1.0; 4],
                )],
                device,
            );
            let r = tl.instances[0].rect;
            assert!(
                approx(r[0], 0.0) && approx(r[1], 0.0),
                "{device:?} TL {r:?}"
            );
            let r = br.instances[0].rect;
            assert!(
                (r[0] + r[2] - dw).abs() <= 1.0 && (r[1] + r[3] - dh).abs() <= 1.0,
                "{device:?} BR {r:?}"
            );
            assert!(
                (r[2] - sw).abs() <= 1.0 && (r[3] - sh).abs() <= 1.0,
                "{device:?} size"
            );
        }
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
