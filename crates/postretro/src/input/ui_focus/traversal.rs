// Focus traversal over the exported rect list: initial focus, linear and
// spatial steps, neighbor overrides, and pointer hit-testing.
// See: context/lib/ui.md §4

use crate::input::ui_dispatch::PointerPos;
use crate::input::ui_nav::NavIntent;
use postretro_ui::tree::{FocusRect, FocusRectList};

/// A directional nav step the focus engine resolves against the rect list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    /// Map a directional `NavIntent` to a `Dir`, or `None` for non-directional
    /// intents (confirm/cancel/menu/options/next/prev — handled separately).
    pub(super) fn from_nav(nav: NavIntent) -> Option<Dir> {
        match nav {
            NavIntent::Up => Some(Dir::Up),
            NavIntent::Down => Some(Dir::Down),
            NavIntent::Left => Some(Dir::Left),
            NavIntent::Right => Some(Dir::Right),
            _ => None,
        }
    }

    pub(super) fn to_nav(self) -> NavIntent {
        match self {
            Dir::Up => NavIntent::Up,
            Dir::Down => NavIntent::Down,
            Dir::Left => NavIntent::Left,
            Dir::Right => NavIntent::Right,
        }
    }
}

/// The id a node's `focusNeighbors` names for `dir`, if any.
pub(super) fn neighbor_override(rect: &FocusRect, dir: Dir) -> Option<&str> {
    match dir {
        Dir::Up => rect.neighbors.up.as_deref(),
        Dir::Down => rect.neighbors.down.as_deref(),
        Dir::Left => rect.neighbors.left.as_deref(),
        Dir::Right => rect.neighbors.right.as_deref(),
    }
}

/// The id focus should start on: the tree's `initialFocus` when it names an
/// existing, non-disabled focusable node, else the first non-disabled focusable
/// node in tree order. A `disabled` node is never selected as initial focus
/// (M13 G2-T3) — even when explicitly named by `initialFocus`.
pub(super) fn initial_focus_id(rects: &FocusRectList) -> Option<String> {
    if let Some(initial) = &rects.initial_focus
        && rects.rects.iter().any(|r| &r.id == initial && !r.disabled)
    {
        return Some(initial.clone());
    }
    rects
        .rects
        .iter()
        .find(|r| !r.disabled)
        .map(|r| r.id.clone())
}

/// Shared index walk for linear `move_focus` and next/prev: find `current_id` in
/// the group's member list and step by `delta`, wrapping or clamping per `wrap`.
///
/// Disabled members are skipped (M13 G2-T3): the walk advances by `delta` repeatedly
/// until it lands on a non-disabled member or exhausts the group, so a run of
/// consecutive disabled members is stepped over in one move (not a single ±1 step
/// that could settle on a disabled node). Wrap and clamp semantics are unchanged —
/// a wrapping group keeps walking around the ring (bounded to its length so an
/// all-disabled group terminates), a non-wrapping group stops at the edge. Returns
/// `None` when no non-disabled member lies that way.
pub(super) fn linear_index_step(
    rects: &FocusRectList,
    group: &postretro_ui::tree::FocusGroup,
    current_id: &str,
    delta: i32,
    wrap: bool,
) -> Option<String> {
    let pos = group
        .members
        .iter()
        .position(|&m| rects.rects[m].id == current_id)?;
    let len = group.members.len() as i32;
    if len == 0 {
        return None;
    }
    // Walk by `delta` past consecutive disabled members. Bounded to `len` steps so
    // a fully-disabled (or wrapping) group can't loop forever.
    let mut raw = pos as i32;
    for _ in 0..len {
        raw += delta;
        let idx = if wrap {
            ((raw % len) + len) % len
        } else if raw < 0 || raw >= len {
            return None;
        } else {
            raw
        };
        let member = group.members[idx as usize];
        if !rects.rects[member].disabled {
            return Some(rects.rects[member].id.clone());
        }
    }
    None
}

/// Center `[cx, cy]` of a device-pixel rect `[x, y, w, h]`.
pub(super) fn center(rect: [f32; 4]) -> (f32, f32) {
    (rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5)
}

/// Resolve a pointer position to the topmost focusable node under it: among all
/// rects containing the point, the one with the highest z (later in tree order
/// draws on top). `None` when the point is over no focusable node.
///
/// Disabled nodes are excluded (M13 G2-T3): a click or hover over a disabled node
/// falls through as if it were not focusable — it neither focuses nor activates it.
/// A disabled node on top does NOT mask a non-disabled node beneath it; the hit
/// resolves to the topmost *non-disabled* rect under the point.
///
/// A stop with a scroll `clip` is hit only inside that clip, so a fully
/// clipped stop is not clickable (nav still reaches it and scrolls it in).
pub(super) fn hit_test_topmost(rects: &FocusRectList, pos: PointerPos) -> Option<&str> {
    let px = pos.x as f32;
    let py = pos.y as f32;
    rects
        .rects
        .iter()
        .filter(|r| {
            // A stop inside a scroll viewport is hit only where it shows: a
            // click on the clipped area reaches nothing hidden there (P19).
            !r.disabled
                && contains(r.rect, px, py)
                && r.clip.is_none_or(|clip| contains(clip, px, py))
        })
        .max_by_key(|r| r.z)
        .map(|r| r.id.as_str())
}

/// Whether a device-pixel rect `[x, y, w, h]` contains the point `(px, py)`.
pub(super) fn contains(rect: [f32; 4], px: f32, py: f32) -> bool {
    px >= rect[0] && px < rect[0] + rect[2] && py >= rect[1] && py < rect[1] + rect[3]
}

#[cfg(test)]
mod scroll_clip_tests {
    use super::*;
    use postretro_ui::tree::FocusNeighbors;

    fn stop(id: &str, rect: [f32; 4], z: u32, clip: Option<[f32; 4]>) -> FocusRect {
        FocusRect {
            id: id.to_string(),
            rect,
            z,
            group: None,
            neighbors: FocusNeighbors::default(),
            interaction: None,
            selected: None,
            checked: None,
            disabled: false,
            tablist: None,
            clip,
        }
    }

    // P19: a click on a scroll container's clipped area activates nothing
    // hidden there; inside the viewport the scrolled stop is hit as drawn.
    #[test]
    fn a_click_on_a_scroll_containers_clipped_area_hits_nothing_hidden() {
        let viewport = Some([0.0, 100.0, 200.0, 100.0]);
        let rects = FocusRectList {
            rects: vec![
                // Scrolled up out of the viewport (drawn above its top edge).
                stop("hidden", [0.0, 60.0, 200.0, 30.0], 0, viewport),
                // Straddling the top edge: only its lower part shows.
                stop("partial", [0.0, 90.0, 200.0, 30.0], 1, viewport),
                stop("shown", [0.0, 150.0, 200.0, 30.0], 2, viewport),
                // Outside any scroll container: not clipped.
                stop("title", [0.0, 0.0, 200.0, 40.0], 3, None),
            ],
            groups: Vec::new(),
            initial_focus: None,
            restore_on_return: true,
            owner: None,
        };
        let at = |x: f64, y: f64| hit_test_topmost(&rects, PointerPos { x, y });
        assert_eq!(at(10.0, 70.0), None, "the hidden stop is not clickable");
        assert_eq!(at(10.0, 95.0), None, "the clipped part of a stop is not hit");
        assert_eq!(at(10.0, 110.0), Some("partial"));
        assert_eq!(at(10.0, 160.0), Some("shown"));
        assert_eq!(at(10.0, 20.0), Some("title"));
    }
}
