// Device-pixel projection, draw-data assembly, value→string/fill resolution, and
// the exported hit-test / focus rect-list types for the retained UI tree.
// See: context/lib/ui.md §1 (retained tree), §4 (interaction / focus)

use std::collections::HashMap;

use taffy::prelude::Layout;

use super::super::descriptor::{BarMax, BindSource, Border, PanelBind, SliderBind, TextBind};
use super::super::layout::{Anchor, REFERENCE_HEIGHT, REFERENCE_WIDTH};
use super::super::text::UiText;
use super::super::{UiDrawList, UiInstance};
use postretro_entities::SlotValue;

use super::CellValues;
use super::predicate::lookup_bound;
use super::style::TweenState;

// --- Hit-test / focus rect-list export ---------------------------------------

/// Focus-traversal kind exported with a focus group. The descriptor twin
/// (`descriptor::FocusKind`) is converted into this at export so the app-side
/// focus engine carries no descriptor dependency on the wire enum's serde derive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusKind {
    Linear,
    Spatial,
}

/// Hold-to-repeat timing exported with a focus group (milliseconds). Mirrors
/// `descriptor::RepeatPolicy`; carried on the focus rect list so the app-side
/// focus engine's dt-clocked repeat timer reads the container's declared cadence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RepeatPolicy {
    pub initial_delay_ms: f32,
    pub interval_ms: f32,
}

/// Per-direction id overrides exported with a focusable node. A set direction
/// wins over the container policy: nav that way jumps straight to the named node.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FocusNeighbors {
    pub up: Option<String>,
    pub down: Option<String>,
    pub left: Option<String>,
    pub right: Option<String>,
}

/// One focusable/interactive node in the exported hit-test/focus rect list: its
/// stable id (authored or auto-generated from tree position), device-pixel rect
/// `[x, y, w, h]`, painter z (tree order — later = higher), the index of the
/// focus group that governs its directional traversal (if any), and its neighbor
/// overrides. The app-side focus engine consumes this the FOLLOWING frame (the
/// reverse of the app→renderer snapshot's N→N+1 contract).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FocusRect {
    pub id: String,
    pub rect: [f32; 4],
    pub z: u32,
    /// Index into `FocusRectList::groups` of the nearest ancestor container that
    /// declares a focus policy, or `None` when no ancestor governs this node.
    pub group: Option<usize>,
    pub neighbors: FocusNeighbors,
    /// Interaction metadata for an interactive widget: a
    /// `button`'s activation reaction or a `slider`'s value-step parameters. `None`
    /// for passive focusables (an id-bearing text/panel/image). The app reads this
    /// off the focused node to fire activation (button `on_press`) or to apply a
    /// captured nav step (slider), keeping the focus engine widget-agnostic.
    pub interaction: Option<NodeInteraction>,
    /// Resolved a11y `aria-selected` state (M13 G2): a button's `selected`
    /// [`Predicate`] resolved against the frame snapshot to `1.0`/`0.0`. `None` when
    /// the widget declares no `selected`. This is a11y METADATA carried on the
    /// readback — the engine draws NO highlight from it (the author wires the visual
    /// highlight through `styleRanges`); the app surfaces it to the a11y layer.
    ///
    /// [`Predicate`]: super::super::descriptor::Predicate
    pub selected: Option<f32>,
    /// Resolved a11y `aria-checked` state (M13 G2): a button's `checked`
    /// [`Predicate`] resolved against the frame snapshot. `None` when undeclared.
    /// Like `selected`, a11y-only — no engine-drawn highlight.
    ///
    /// [`Predicate`]: super::super::descriptor::Predicate
    pub checked: Option<f32>,
    /// Disabled bit (M13 G2): a button/slider's `disabled` field. A disabled
    /// focusable is non-interactive and a11y-disabled. Populated here AND honored
    /// in nav/activation: `input/ui_focus.rs` skips disabled nodes during
    /// navigation and pointer focus, and the App activation gate blocks activation
    /// on a disabled focused node. Both sides are complete.
    pub disabled: bool,
}

/// Per-node interaction metadata exported with an interactive focusable node.
/// The app resolves the focused node's interaction to route a button's reserved
/// UI action or named reaction on confirm/click, or to step a slider's bound
/// value on a captured nav intent and emit the `setState` write.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NodeInteraction {
    /// A `button`: activation (confirm/click) routes `on_press` through the App's
    /// reserved-action-or-named-reaction seam.
    /// `repeat_on_hold`, when present, opts the button into activation-repeat: a
    /// HELD confirm re-fires `on_press` on the focus engine's hold-to-repeat clock
    /// (on-screen keyboard backspace pattern). Absent keeps the single-fire rule
    /// (one activation per press).
    Button {
        on_press: String,
        repeat_on_hold: Option<RepeatPolicy>,
    },
    /// A `slider`: a captured nav wire in `captures_nav` steps the bound slot's
    /// value by `step` within `[min, max]` and emits a `setState` write on the N+1
    /// frame. `"nav.left"`/`"nav.down"` decrease the value, `"nav.right"`/`"nav.up"`
    /// increase it; any other captured name is swallowed (focus does not move) but
    /// leaves the value unchanged.
    Slider {
        slot: String,
        min: f32,
        max: f32,
        step: f32,
        captures_nav: Vec<String>,
    },
}

/// A focus group exported from a container that declares a `focus` policy: its
/// traversal kind, wrap flag, optional repeat cadence, and the indices (into
/// `FocusRectList::rects`) of its directly-governed focusable members in tree
/// order. The focus engine moves focus within a group by its policy.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FocusGroup {
    pub kind: FocusKind,
    pub wrap: bool,
    pub repeat: Option<RepeatPolicy>,
    /// Indices into `FocusRectList::rects` of this group's members, tree order.
    pub members: Vec<usize>,
}

/// The flat hit-test / focus rect list exported once per draw-data build: every
/// focusable node's id+rect+z+group, plus the focus groups their containers
/// declared. The renderer publishes this back to the app (the reverse twin of the
/// app→renderer `UiReadSnapshot`); the focus engine reads it the next frame to
/// move focus, resolve pointer hits (topmost z), and drive the repeat timer.
///
/// "Focusable" today means a node that carries an authored `id` or sits under a
/// container that declares a focus policy (interactive widgets plug their
/// markers into this seam).
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct FocusRectList {
    pub rects: Vec<FocusRect>,
    pub groups: Vec<FocusGroup>,
    /// The tree's `initialFocus` (from the `AnchoredTree` envelope): the node id
    /// focus starts on when this tree becomes the active (top) stack tree. `None`
    /// selects the first focusable node in tree order.
    pub initial_focus: Option<String>,
    /// True when any container in the tree declared `restoreOnReturn`: on a pop
    /// that returns focus here, the focus engine restores this tree's last-focused
    /// node instead of resetting to `initial_focus`.
    pub restore_on_return: bool,
}

impl From<super::super::descriptor::FocusKind> for FocusKind {
    fn from(kind: super::super::descriptor::FocusKind) -> Self {
        match kind {
            super::super::descriptor::FocusKind::Linear => FocusKind::Linear,
            super::super::descriptor::FocusKind::Spatial => FocusKind::Spatial,
        }
    }
}

impl From<super::super::descriptor::RepeatPolicy> for RepeatPolicy {
    fn from(p: super::super::descriptor::RepeatPolicy) -> Self {
        Self {
            initial_delay_ms: p.initial_delay_ms,
            interval_ms: p.interval_ms,
        }
    }
}

impl From<&super::super::descriptor::FocusNeighbors> for FocusNeighbors {
    fn from(n: &super::super::descriptor::FocusNeighbors) -> Self {
        Self {
            up: n.up.clone(),
            down: n.down.clone(),
            left: n.left.clone(),
            right: n.right.clone(),
        }
    }
}

/// Computed draw entries from one tree: a device-pixel panel quad `UiDrawList`,
/// per-asset image quad lists, and device-positioned shaped-text lines. Panels
/// draw first (one batch, the pass's white texel), then each image group (one
/// batch per `asset`, its own bound texture), then text composites over them —
/// the order the UI pass records in.
///
/// Image quads are split out from panels because each `asset` key binds a
/// distinct texture: the renderer resolves the key through its image registry to
/// a bind group, so the tree groups image quads by key rather than folding them
/// into the panel list. `images` preserves first-seen key order so draw order is
/// deterministic.
#[derive(Debug, Default, Clone)]
pub(crate) struct UiDrawData {
    pub quads: UiDrawList,
    /// Image quad batches keyed by `asset`, in first-seen order. Each entry is
    /// `(asset_key, quads)`; the renderer binds the key's texture for its quads.
    pub images: Vec<(String, UiDrawList)>,
    pub texts: Vec<UiText>,
}

impl UiDrawData {
    /// `true` when this tree produced no drawable output: no panel quads, no
    /// image quads, and no text. The renderer early-outs at the composed
    /// `UiComposition` level; this per-layer predicate is test-only.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.quads.is_empty()
            && self.texts.is_empty()
            && self.images.iter().all(|(_, list)| list.is_empty())
    }

    /// Mutable handle to the quad list for `asset`, creating an empty list in
    /// first-seen order if the key is new. Keeps all quads sharing a texture in
    /// one batch so the renderer issues one draw per bound image.
    pub(crate) fn image_quad_for(&mut self, asset: &str) -> &mut UiDrawList {
        if let Some(idx) = self.images.iter().position(|(k, _)| k == asset) {
            return &mut self.images[idx].1;
        }
        self.images.push((asset.to_string(), UiDrawList::new()));
        &mut self.images.last_mut().expect("just pushed").1
    }
}

/// Project a node's reference-space rect (origin + taffy size) to a device-pixel
/// `[x, y, w, h]`, snapping each edge to a whole device pixel. Mirrors
/// `layout::project_element`'s edge-snap so abutting nodes stay gap-free.
pub(crate) fn project_rect(
    ref_origin: [f32; 2],
    layout: &Layout,
    scale: f32,
    canvas_origin: [f32; 2],
) -> [f32; 4] {
    let dev_left = canvas_origin[0] + ref_origin[0] * scale;
    let dev_top = canvas_origin[1] + ref_origin[1] * scale;
    let dev_right = dev_left + layout.size.width * scale;
    let dev_bottom = dev_top + layout.size.height * scale;

    let x = dev_left.round();
    let y = dev_top.round();
    [x, y, dev_right.round() - x, dev_bottom.round() - y]
}

/// Project a panel rect into a `UiInstance`, scaling/snapping any 9-slice border
/// margin the same way `layout::project_element` does so the shader's corner
/// regions land on whole device pixels.
pub(crate) fn project_quad(
    ref_origin: [f32; 2],
    layout: &Layout,
    scale: f32,
    canvas_origin: [f32; 2],
    fill: [f32; 4],
    border: Option<&Border>,
) -> UiInstance {
    let rect = project_rect(ref_origin, layout, scale, canvas_origin);
    let margin = match border {
        // 9-slice insets are `[left, top, right, bottom]` logical px; scale + snap.
        Some(b) => [
            (b.slice[0] * scale).round(),
            (b.slice[1] * scale).round(),
            (b.slice[2] * scale).round(),
            (b.slice[3] * scale).round(),
        ],
        None => [0.0; 4],
    };
    UiInstance::panel(rect, fill, margin)
}

/// Fractional anchor position in `[0,1]` per axis, x right / y down — the same
/// table `layout::Anchor::fractions` exposes (private there), reused here for the
/// whole-tree placement transform.
pub(crate) fn anchor_fractions(anchor: Anchor) -> (f32, f32) {
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

/// Top-left of the scaled 1280x720 canvas in device pixels, centered so the
/// letterbox margin splits evenly. Same rule as `layout::canvas_origin` (private
/// there) — reused so tree-laid rects share the splash's letterbox.
pub(crate) fn canvas_origin(device_size: [u32; 2], scale: f32) -> [f32; 2] {
    let scaled_w = REFERENCE_WIDTH * scale;
    let scaled_h = REFERENCE_HEIGHT * scale;
    [
        (device_size[0] as f32 - scaled_w) * 0.5,
        (device_size[1] as f32 - scaled_h) * 0.5,
    ]
}

/// Resolve a bound text node's drawn string from the frame's slot snapshot.
/// Unbound (`bind == None`) returns the literal `fallback`. Bound: look up
/// `bind.slot`; if the slot is absent from the snapshot, fall back to the literal
/// `fallback` (no panic, no warn — absence is the normal "slot not written this
/// frame" case). Present: format the value to a string and, if `bind.format` is
/// `Some(template)`, substitute its single `{}` with that string; with no format,
/// the value's bare string is drawn.
pub(crate) fn resolve_text(
    bind: Option<&TextBind>,
    bind_scope: Option<&str>,
    fallback: &str,
    slot_values: &HashMap<String, SlotValue>,
    cell_values: &CellValues,
) -> String {
    let Some(bind) = bind else {
        return fallback.to_string();
    };
    let Some(value) = lookup_bound(&bind.source, bind_scope, slot_values, cell_values) else {
        return fallback.to_string();
    };
    let rendered = slot_value_string(value);
    match &bind.format {
        // Single-placeholder substitution; multi-value templates are out of
        // scope, so only the first `{}` is replaced.
        Some(template) => template.replacen("{}", &rendered, 1),
        None => rendered,
    }
}

/// A `SlotValue`'s natural string form for text binding. `Number` formats
/// cleanly: an integral value prints with no decimals (`42`, not `42.0`), a
/// fractional value keeps its default float form (`12.5`). `Boolean`/`String`/
/// `Enum` print their natural representation. `Array` has no text rendering (it
/// is the panel-color shape), so it formats to an empty string — a text widget
/// should not bind an array slot.
fn slot_value_string(value: &SlotValue) -> String {
    match value {
        SlotValue::Number(n) => {
            if n.fract() == 0.0 && n.is_finite() {
                format!("{}", *n as i64)
            } else {
                n.to_string()
            }
        }
        SlotValue::Boolean(b) => b.to_string(),
        SlotValue::String(s) => s.clone(),
        SlotValue::Enum(e) => e.clone(),
        SlotValue::Array(_) => String::new(),
    }
}

/// Resolve a bound panel's fill from the frame's slot snapshot. Unbound returns
/// the literal `fallback`. Bound: look up `bind.slot`; a `SlotValue::Array` of
/// exactly 4 f32 is used as the linear `[r, g, b, a]` fill. An absent slot falls
/// back silently (the normal "not written this frame" case). A present-but-
/// malformed value (wrong variant, or an array whose length is not 4) falls back
/// to the literal `fallback` with a single `log::warn!` per call. The warn fires
/// once per frame for a genuinely mis-typed slot on BOTH paths: the fresh path
/// (one build per frame) and the retained tweened path (`drive_panel_binding`
/// snaps through here every frame the slot keeps the wrong shape). There is no
/// cross-frame dedup — an authoring error, not per-frame spam.
pub(crate) fn resolve_panel_fill(
    bind: Option<&PanelBind>,
    bind_scope: Option<&str>,
    fallback: [f32; 4],
    slot_values: &HashMap<String, SlotValue>,
    cell_values: &CellValues,
) -> [f32; 4] {
    let Some(bind) = bind else {
        return fallback;
    };
    match lookup_bound(&bind.source, bind_scope, slot_values, cell_values) {
        Some(SlotValue::Array(rgba)) if rgba.len() == 4 => [rgba[0], rgba[1], rgba[2], rgba[3]],
        // Absent slot/cell: silent fallback — the value simply was not written
        // this frame, which is expected for an optional binding.
        None => fallback,
        // Present but the wrong shape: an authoring error worth one warn.
        Some(_) => {
            log::warn!(
                "[Renderer] panel bind '{}' is not a length-4 array; using literal fill",
                bind_target_name(&bind.source),
            );
            fallback
        }
    }
}

/// A bind source's display name for diagnostics: the slot's dotted name or the
/// local cell name. Used in warn/skip messages where the bind kind is irrelevant.
pub(crate) fn bind_target_name(source: &BindSource) -> &str {
    match source {
        BindSource::Slot { slot } => slot,
        BindSource::Local { local } => local,
    }
}

/// The scalar value a `text` node's styleRanges maps: the eased tween *display*
/// value when a tween is active on the bind (the styleRanges contract — it
/// evaluates the value the widget renders, which mid-tween is the display value),
/// else the bound slot's `Number`. `None` when there is no bind, the slot is
/// absent, or it is not a `Number` (no value to map — the node keeps its base
/// color). The tween's display is the same eased value the rendered string shows,
/// so the color tracks the displayed number.
pub(crate) fn style_text_value(
    bind: Option<&TextBind>,
    bind_scope: Option<&str>,
    tween: Option<&TweenState<f32>>,
    slot_values: &HashMap<String, SlotValue>,
    cell_values: &CellValues,
) -> Option<f32> {
    if let Some(state) = tween {
        return Some(state.display);
    }
    let bind = bind?;
    match lookup_bound(&bind.source, bind_scope, slot_values, cell_values) {
        Some(SlotValue::Number(n)) => Some(*n),
        _ => None,
    }
}

/// The scalar value a `panel` node's styleRanges maps: the bound slot's `Number`.
/// `None` when there is no bind, the slot is absent, or it is not a `Number`. A
/// panel's RGBA fill-tween carries no scalar, so styleRanges on a panel reads the
/// raw numeric slot (a styleRanges panel binds a numeric slot, not the length-4
/// fill array) — the two bind uses are distinct.
/// Seam: unlike `style_text_value` (which returns the eased display value when a
/// tween is active), this path reads the raw slot. A panel's value-tween carries an
/// RGBA fill; a panel styleRanges bind carries a scalar — the two never coexist on
/// one panel, so there is no display value to prefer here.
pub(crate) fn style_value(
    bind: Option<&PanelBind>,
    bind_scope: Option<&str>,
    slot_values: &HashMap<String, SlotValue>,
    cell_values: &CellValues,
) -> Option<f32> {
    let bind = bind?;
    match lookup_bound(&bind.source, bind_scope, slot_values, cell_values) {
        Some(SlotValue::Number(n)) => Some(*n),
        _ => None,
    }
}

/// Resolve a `bar`'s bound numeric value from the frame's slot snapshot, or `0.0`
/// when the slot is absent or not a `Number` (a bar with no value reads empty).
/// The bar always binds, so there is no unbound fallback.
pub(crate) fn bar_slot_value(
    bind: &SliderBind,
    bind_scope: Option<&str>,
    slot_values: &HashMap<String, SlotValue>,
    cell_values: &CellValues,
) -> f32 {
    match lookup_bound(&bind.source, bind_scope, slot_values, cell_values) {
        Some(SlotValue::Number(n)) => *n,
        _ => 0.0,
    }
}

pub(crate) fn bar_max_value(max: &BarMax, slot_values: &HashMap<String, SlotValue>) -> f32 {
    match max {
        BarMax::Literal(value) => *value,
        BarMax::State(reference) => match slot_values.get(&reference.slot) {
            Some(SlotValue::Number(value)) => *value,
            _ => 0.0,
        },
    }
}

/// Convert a linear-RGBA `[f32; 4]` color to glyphon's sRGB-encoded `[u8; 4]`.
/// RGB channels go through the sRGB transfer function; alpha is linear (stays a
/// straight 0..1 → 0..255 scale). Matches the `UiText` color contract.
pub(crate) fn linear_rgba_to_srgb_u8(color: [f32; 4]) -> [u8; 4] {
    let encode = |c: f32| -> u8 {
        let c = c.clamp(0.0, 1.0);
        let srgb = if c <= 0.003_130_8 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        };
        (srgb * 255.0).round() as u8
    };
    [
        encode(color[0]),
        encode(color[1]),
        encode(color[2]),
        (color[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}
