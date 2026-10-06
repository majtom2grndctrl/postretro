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

/// Linear traversal: step the current node to the previous/next member of its
/// group in tree order, wrapping per `wrap`. `dir` maps Up/Left → previous,
/// Down/Right → next (a vstack navigates with Up/Down, an hstack with Left/Right;
/// either pair walks the same sequential member order).
pub(super) fn linear_step(
    rects: &FocusRectList,
    group_idx: usize,
    current_id: &str,
    dir: Dir,
    wrap: bool,
) -> Option<String> {
    let delta = match dir {
        Dir::Up | Dir::Left => -1,
        Dir::Down | Dir::Right => 1,
    };
    linear_index_step(rects, &rects.groups[group_idx], current_id, delta, wrap)
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

/// Spatial traversal: among the group's members lying in `dir`'s half-plane
/// relative to `current`, pick the one whose center is nearest (Euclidean on
/// device-pixel centers, with a perpendicular-offset penalty so a straight-ahead
/// neighbor beats a diagonal). Returns `None` when no member lies that way.
pub(super) fn spatial_step(
    rects: &FocusRectList,
    group_idx: usize,
    current: &FocusRect,
    dir: Dir,
) -> Option<String> {
    let group = &rects.groups[group_idx];
    let (cx, cy) = center(current.rect);

    let mut best: Option<(f32, &str)> = None;
    for &m in &group.members {
        let cand = &rects.rects[m];
        if cand.id == current.id {
            continue;
        }
        // Disabled members are excluded from the candidate set (M13 G2-T3).
        if cand.disabled {
            continue;
        }
        let (tx, ty) = center(cand.rect);
        let dx = tx - cx;
        let dy = ty - cy;
        // Primary axis must move the right way past a small epsilon; the
        // perpendicular offset is penalized so the most aligned neighbor wins.
        let (along, perp) = match dir {
            Dir::Up => (-dy, dx),
            Dir::Down => (dy, dx),
            Dir::Left => (-dx, dy),
            Dir::Right => (dx, dy),
        };
        // Guards floating-point ties; a candidate must advance by more than 0.5 dp
        // on the primary axis (exactly 0.5 is excluded).
        if along <= 0.5 {
            continue;
        }
        // Weight the perpendicular offset heavily so straight-ahead wins over a
        // diagonal at similar primary distance.
        let cost = along + perp.abs() * 2.0;
        if best.map(|(bc, _)| cost < bc).unwrap_or(true) {
            best = Some((cost, cand.id.as_str()));
        }
    }
    best.map(|(_, id)| id.to_string())
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
pub(super) fn hit_test_topmost(rects: &FocusRectList, pos: PointerPos) -> Option<&str> {
    let px = pos.x as f32;
    let py = pos.y as f32;
    rects
        .rects
        .iter()
        .filter(|r| !r.disabled && contains(r.rect, px, py))
        .max_by_key(|r| r.z)
        .map(|r| r.id.as_str())
}

/// Whether a device-pixel rect `[x, y, w, h]` contains the point `(px, py)`.
pub(super) fn contains(rect: [f32; 4], px: f32, py: f32) -> bool {
    px >= rect[0] && px < rect[0] + rect[2] && py >= rect[1] && py < rect[1] + rect[3]
}
