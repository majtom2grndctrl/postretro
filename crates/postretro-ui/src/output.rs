//! CPU-owned UI output and app->renderer snapshot payloads.
//!
//! These types are intentionally free of GPU handles. The renderer pass uploads
//! or consumes these payloads at its own boundary.

use bytemuck::{Pod, Zeroable};

use super::{descriptor, tree};

/// Per-instance draw record. Layout mirrors `UiInstance` in `ui_quad.wgsl`:
/// four `vec4<f32>` attributes, tightly packed, no padding. Byte-for-byte
/// stable so `bytemuck` can cast a slice straight into the instance buffer.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct UiInstance {
    /// Device-pixel rect: `[x, y, width, height]`, top-left origin.
    pub rect: [f32; 4],
    /// UV rect into the bound texture: `[u0, v0, u_width, v_height]`.
    pub uv_rect: [f32; 4],
    /// Linear RGBA tint multiplied into the sampled texel.
    pub color: [f32; 4],
    /// 9-slice margin in device pixels: `[left, top, right, bottom]`. All zero
    /// renders a plain stretched quad (the degenerate case).
    pub margin: [f32; 4],
}

impl UiInstance {
    /// Solid-color panel: full UV slice over the bound 1x1 white texel, with an
    /// optional 9-slice margin. Color is linear RGBA. Production paths build
    /// instances via `layout::project`; this ctor backs the corner-rect tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn panel(rect: [f32; 4], color: [f32; 4], margin: [f32; 4]) -> Self {
        Self {
            rect,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            color,
            margin,
        }
    }

    /// Textured image: samples the full bound texture, untinted (white).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn image(rect: [f32; 4]) -> Self {
        Self {
            rect,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            color: [1.0, 1.0, 1.0, 1.0],
            margin: [0.0; 4],
        }
    }

    /// CPU-side derivation of the 9-slice corner rects (device pixels) for this
    /// instance - the four fixed-size corners as `[x, y, w, h]` in order
    /// top-left, top-right, bottom-left, bottom-right. Mirrors the shader's
    /// `axis` margin clamp so layout assertions match what the GPU draws.
    /// Used by tests (and future layout assertions); not consumed by the draw.
    #[cfg(test)]
    pub fn corner_rects(&self) -> [[f32; 4]; 4] {
        let (x, y, w, h) = (self.rect[0], self.rect[1], self.rect[2], self.rect[3]);
        let (ml, mt, mr, mb) = (
            self.margin[0],
            self.margin[1],
            self.margin[2],
            self.margin[3],
        );
        // Clamp margins so opposing corners never overrun the rect - matches
        // `axis` in ui_quad.wgsl.
        let clamp_axis = |full: f32, lo: f32, hi: f32| -> (f32, f32) {
            let avail = full.max(0.0);
            let lo_c = lo.clamp(0.0, avail);
            let hi_c = hi.clamp(0.0, (avail - lo_c).max(0.0));
            (lo_c, hi_c)
        };
        let (cl, cr) = clamp_axis(w, ml, mr);
        let (ct, cb) = clamp_axis(h, mt, mb);
        [
            [x, y, cl, ct],                   // top-left
            [x + w - cr, y, cr, ct],          // top-right
            [x, y + h - cb, cl, cb],          // bottom-left
            [x + w - cr, y + h - cb, cr, cb], // bottom-right
        ]
    }
}

/// Pure CPU draw list - a flat batch of instances sharing one bound texture.
/// Built with no wgpu call so layout/scaling logic stays GPU-independent: the
/// `layout` projection path populates it and the CPU layout tests assert against
/// it. The pass uploads it to the instance buffer at encode time.
#[derive(Debug, Default, Clone)]
pub struct UiDrawList {
    pub instances: Vec<UiInstance>,
}

impl UiDrawList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, instance: UiInstance) {
        self.instances.push(instance);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn clear(&mut self) {
        self.instances.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }
}

/// UI uniform: device viewport in pixels. 16 bytes (vec2 + vec2 pad) to match
/// `UiUniform` in `ui_quad.wgsl` and satisfy uniform alignment.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct UiUniform {
    pub viewport: [f32; 2],
    pub _pad: [f32; 2],
}

/// One shaped text line for the renderer text backend. Positions and font size
/// arrive already in **device pixels** (device-scaled by the caller, not in
/// logical-reference units), so text and quad output share one
/// coordinate space and text tracks resolution the same way panels do. The
/// position is NOT integer-snapped - the text backend keeps sub-pixel AA.
#[derive(Debug, Clone)]
pub struct UiText {
    /// The string to shape and render.
    pub content: String,
    /// Top-left baseline-box position in device pixels (`[left, top]`). Not
    /// snapped - text is positioned with sub-pixel precision.
    pub position: [f32; 2],
    /// Font size in device pixels (already device-scaled by the caller).
    pub font_size: f32,
    /// Glyph color, sRGB 0..=255 per channel + alpha.
    pub color: [u8; 4],
    /// Registered font family name to shape this line with. It must match a
    /// family registered in the text renderer's font system; an unregistered name
    /// falls back to a system face.
    pub family: String,
}

impl UiText {
    /// Convenience constructor for a single device-positioned line.
    pub fn new(
        content: impl Into<String>,
        position: [f32; 2],
        font_size: f32,
        color: [u8; 4],
        family: impl Into<String>,
    ) -> Self {
        Self {
            content: content.into(),
            position,
            font_size,
            color,
            family: family.into(),
        }
    }
}

/// One entry in the gameplay UI modal stack as published on the read snapshot:
/// a named descriptor tree plus its resolved input behavior and the optional
/// `onCommit` reaction carried from the `PushTree` that opened it. The renderer
/// draws the stack bottom->top; the app reads the TOP entry's `capture_mode` to
/// drive the input seam and focus. The App fires `on_commit` from the text-entry
/// commit seam; the renderer never reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct UiTreeEntry {
    /// Registry name the tree was registered/pushed under. Identifies the entry
    /// in the stack (e.g. for diagnostics); the renderer keys retained state by
    /// stack position, not by name.
    pub name: String,
    /// The descriptor tree to lay out and draw this frame.
    pub descriptor: descriptor::AnchoredTree,
    /// Resolved capture behavior (from the descriptor's `capture_mode` envelope).
    /// Only the TOP entry's mode is acted on by the app's input seam.
    pub capture_mode: descriptor::CaptureMode,
    /// Optional named reaction fired by the App when this tree commits (carried
    /// from `PushTree { on_commit }`).
    pub on_commit: Option<String>,
}

/// Once-per-frame published read-only snapshot the UI pass reads when it records.
/// Stored on the `Renderer` via a setter the `App` calls just before each render
/// call - NOT threaded as a render parameter, so both render signatures stay
/// stable.
///
/// Carries the gameplay UI modal stack (a Vec of trees drawn bottom->top) and the
/// frame's resolved slot values. The renderer reads `trees`/`slot_values` on the
/// gameplay path. The boot splash does NOT use this - it is a renderer-owned
/// pass independent of the UI system. Descriptors are carried here - never
/// laid-out rects; CPU layout and text shaping are resolved from this snapshot.
/// Slot values are cloned out of the live `SlotTable` once per frame so the
/// renderer never borrows the live store - preserving the renderer/game-logic
/// boundary. Value-less slots are omitted; a present key always carries a
/// resolved value.
#[derive(Debug, Clone, Default)]
pub struct UiReadSnapshot {
    /// The gameplay UI modal stack for this frame, drawn bottom->top (`trees[0]`
    /// is the bottom, the last entry is the top/active tree). Empty (the default)
    /// on the splash path and whenever gameplay publishes no UI - the renderer's
    /// UI pass then early-outs each empty/absent layer. The bottom-of-stack layer
    /// is the HUD (`content/base/ui/hud.json`), resolved by name from the registry
    /// and published by `main.rs`; modal trees pushed via the named-tree registry
    /// stack above it.
    pub trees: Vec<UiTreeEntry>,
    /// Resolved state-store values for this frame, keyed by dotted slot name.
    /// Cloned out of the live `SlotTable` once per frame (see the type doc).
    /// Only slots that currently hold a value appear; value-less slots are
    /// skipped. Empty on the splash path.
    pub slot_values: std::collections::HashMap<String, postretro_entities::SlotValue>,
    /// Resolved presentation-cell values for this frame, keyed by
    /// `(scopeId, cellName)`. Published from the app-side cell
    /// store the same way `slot_values` flows from the slot table - so a `{ local }`
    /// bind resolves against the live cell value without the descriptor (compared
    /// by the retained reuse gate) ever changing. Empty on the splash path and
    /// whenever no `localState` scope composes.
    pub cell_values: tree::CellValues,
    /// Deterministic frame time in seconds, accumulated from per-frame `dt`
    /// (`App::script_time`) - NEVER wall-clock. Stays `f64` end-to-end to match
    /// the App's accumulator. The retained gameplay build threads it down so the
    /// tween runtime can ease bound display values over time. `0.0` (the default)
    /// on the splash/fresh path, where inertness is structural - that path takes
    /// no time at all.
    pub time_seconds: f64,
    /// The focused node id in the active (top) stack tree, resolved app-side by
    /// the focus engine the previous frame. The UI pass draws the focus ring around
    /// this node's rect on the top layer. `None` (the default) when nothing is
    /// focused; the ring may trail a focus change by one frame (the same N->N+1
    /// latency every UI event carries).
    pub focused_id: Option<String>,
}

impl UiReadSnapshot {
    /// Snapshot carrying the gameplay UI modal stack (the content side) plus the
    /// frame's resolved slot-value snapshot and the deterministic frame time. The
    /// renderer lays each tree out bottom->top into the UI draw list, resolves
    /// `bind` slots against `slot_values`, and threads `time_seconds` into the
    /// retained build so the tween runtime can ease bound display values over time.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_trees(
        trees: Vec<UiTreeEntry>,
        slot_values: std::collections::HashMap<String, postretro_entities::SlotValue>,
        cell_values: tree::CellValues,
        time_seconds: f64,
        focused_id: Option<String>,
    ) -> Self {
        Self {
            trees,
            slot_values,
            cell_values,
            time_seconds,
            focused_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_instance_byte_layout_is_64_bytes_no_padding() {
        // The shader's instance vertex layout (four Float32x4 at offsets
        // 0/16/32/48, stride 64) depends on this exact packing.
        assert_eq!(std::mem::size_of::<UiInstance>(), 64);
        assert_eq!(std::mem::align_of::<UiInstance>(), 4);
        // Field offsets the VertexAttribute table hardcodes.
        let probe = UiInstance {
            rect: [1.0, 2.0, 3.0, 4.0],
            uv_rect: [5.0, 6.0, 7.0, 8.0],
            color: [9.0, 10.0, 11.0, 12.0],
            margin: [13.0, 14.0, 15.0, 16.0],
        };
        let bytes: &[u8] = bytemuck::bytes_of(&probe);
        assert_eq!(bytes.len(), 64);
        // First field starts at offset 0, last vec4 at offset 48.
        assert_eq!(&bytes[0..4], &1.0f32.to_le_bytes());
        assert_eq!(&bytes[48..52], &13.0f32.to_le_bytes());
    }

    #[test]
    fn uniform_is_16_bytes() {
        assert_eq!(std::mem::size_of::<UiUniform>(), 16);
    }

    #[test]
    fn zero_margin_corner_rects_collapse() {
        // A plain quad (zero margin) has zero-size corners - the whole rect is
        // the stretched center region.
        let inst = UiInstance::panel([10.0, 20.0, 100.0, 60.0], [1.0; 4], [0.0; 4]);
        for c in inst.corner_rects() {
            assert_eq!(c[2], 0.0, "corner width collapses with zero margin");
            assert_eq!(c[3], 0.0, "corner height collapses with zero margin");
        }
    }

    #[test]
    fn nine_slice_corner_rects_are_fixed_size_and_anchored() {
        // 8px corners on a 100x60 rect at (10,20). Corners keep their 8px size
        // and sit at the four rect corners regardless of center stretch.
        let inst = UiInstance::panel([10.0, 20.0, 100.0, 60.0], [1.0; 4], [8.0, 8.0, 8.0, 8.0]);
        let [tl, tr, bl, br] = inst.corner_rects();
        assert_eq!(tl, [10.0, 20.0, 8.0, 8.0]);
        assert_eq!(tr, [10.0 + 100.0 - 8.0, 20.0, 8.0, 8.0]);
        assert_eq!(bl, [10.0, 20.0 + 60.0 - 8.0, 8.0, 8.0]);
        assert_eq!(br, [10.0 + 100.0 - 8.0, 20.0 + 60.0 - 8.0, 8.0, 8.0]);
    }

    #[test]
    fn corner_rects_clamp_when_margins_exceed_rect() {
        // Margins larger than the rect must not produce overlapping/negative
        // corners - they clamp to the available space (mirrors axis).
        let inst = UiInstance::panel([0.0, 0.0, 10.0, 10.0], [1.0; 4], [8.0, 8.0, 8.0, 8.0]);
        let [tl, tr, _bl, _br] = inst.corner_rects();
        // Left corner gets 8, right corner gets the remaining 2.
        assert_eq!(tl[2], 8.0);
        assert_eq!(tr[2], 2.0);
        assert!(tr[0] >= tl[0] + tl[2] - 1e-6, "corners do not overlap");
    }

    #[test]
    fn draw_list_push_and_clear() {
        let mut list = UiDrawList::new();
        assert!(list.is_empty());
        list.push(UiInstance::image([0.0, 0.0, 5.0, 5.0]));
        assert_eq!(list.len(), 1);
        list.clear();
        assert!(list.is_empty());
    }
}
