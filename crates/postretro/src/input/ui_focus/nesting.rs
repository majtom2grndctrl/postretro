// Nested focus groups: a directional move a group cannot answer continues in
// the enclosing group, where each nested group is one candidate by its bounds.
// See: context/lib/ui.md §4

use postretro_ui::tree::{FocusAxis, FocusKind, FocusRectList};

use super::traversal::{Dir, center};

/// One candidate inside a group: a member stop or a nested child group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Candidate {
    Rect(usize),
    Group(usize),
}

/// The axis a direction moves along.
pub(super) fn axis_of(dir: Dir) -> FocusAxis {
    match dir {
        Dir::Up | Dir::Down => FocusAxis::Vertical,
        Dir::Left | Dir::Right => FocusAxis::Horizontal,
    }
}

/// Whether a linear group answers `dir`: a stack answers its own axis only,
/// a grid both. A root group answers both, so a menu with no enclosing group
/// keeps stepping on either axis.
pub(super) fn linear_answers(rects: &FocusRectList, group: usize, dir: Dir) -> bool {
    let g = &rects.groups[group];
    g.parent.is_none() || g.axis.is_none_or(|axis| axis == axis_of(dir))
}

/// The first rect index in tree order a group contains, through nested
/// groups; `None` for a group with no stops.
fn first_rect(rects: &FocusRectList, group: usize) -> Option<usize> {
    let own = rects.groups[group].members.iter().copied().min();
    let nested = rects
        .groups
        .iter()
        .enumerate()
        .filter(|(_, g)| g.parent == Some(group))
        .filter_map(|(child, _)| first_rect(rects, child))
        .min();
    match (own, nested) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// A group's candidates — its member stops and direct child groups — in tree
/// order. A child group with no stops is not a candidate.
pub(super) fn candidates(rects: &FocusRectList, group: usize) -> Vec<Candidate> {
    let mut out: Vec<(usize, Candidate)> = rects.groups[group]
        .members
        .iter()
        .map(|&m| (m, Candidate::Rect(m)))
        .collect();
    for (child, g) in rects.groups.iter().enumerate() {
        if g.parent == Some(group)
            && let Some(first) = first_rect(rects, child)
        {
            out.push((first, Candidate::Group(child)));
        }
    }
    out.sort_by_key(|(position, _)| *position);
    out.into_iter().map(|(_, candidate)| candidate).collect()
}

/// Whether `rect` lies inside `group`, through nested groups.
pub(super) fn group_contains(rects: &FocusRectList, group: usize, rect: usize) -> bool {
    let mut current = rects.rects[rect].group;
    while let Some(g) = current {
        if g == group {
            return true;
        }
        current = rects.groups[g].parent;
    }
    false
}

/// Whether a candidate can take focus: an enabled stop, or a group holding one.
fn enabled(rects: &FocusRectList, candidate: Candidate) -> bool {
    match candidate {
        Candidate::Rect(r) => !rects.rects[r].disabled,
        Candidate::Group(g) => rects
            .rects
            .iter()
            .enumerate()
            .any(|(r, rect)| !rect.disabled && group_contains(rects, g, r)),
    }
}

fn bounds(rects: &FocusRectList, candidate: Candidate) -> [f32; 4] {
    match candidate {
        Candidate::Rect(r) => rects.rects[r].rect,
        Candidate::Group(g) => rects.groups[g].bounds,
    }
}

/// The candidate a move from `origin` lands on among `group`'s candidates,
/// or `None` when the group cannot answer (the move escapes outward).
pub(super) fn step_from(
    rects: &FocusRectList,
    group: usize,
    origin: Candidate,
    dir: Dir,
) -> Option<Candidate> {
    let parent = group;
    let list = candidates(rects, parent);
    let origin_index = list.iter().position(|c| *c == origin)?;
    let group = &rects.groups[parent];
    match group.kind {
        FocusKind::Linear => {
            if !linear_answers(rects, parent, dir) {
                return None;
            }
            let delta: i32 = match dir {
                Dir::Up | Dir::Left => -1,
                Dir::Down | Dir::Right => 1,
            };
            let len = list.len() as i32;
            let mut raw = origin_index as i32;
            for _ in 0..len {
                raw += delta;
                let idx = if group.wrap {
                    raw.rem_euclid(len)
                } else if raw < 0 || raw >= len {
                    return None;
                } else {
                    raw
                };
                let candidate = list[idx as usize];
                if candidate != origin && enabled(rects, candidate) {
                    return Some(candidate);
                }
            }
            None
        }
        FocusKind::Spatial => {
            let (cx, cy) = center(bounds(rects, origin));
            let mut best: Option<(f32, Candidate)> = None;
            for candidate in list {
                if candidate == origin || !enabled(rects, candidate) {
                    continue;
                }
                let (tx, ty) = center(bounds(rects, candidate));
                let (along, perp) = match dir {
                    Dir::Up => (cy - ty, tx - cx),
                    Dir::Down => (ty - cy, tx - cx),
                    Dir::Left => (cx - tx, ty - cy),
                    Dir::Right => (tx - cx, ty - cy),
                };
                if along <= 0.5 {
                    continue;
                }
                let cost = along + perp.abs() * 2.0;
                if best.is_none_or(|(b, _)| cost < b) {
                    best = Some((cost, candidate));
                }
            }
            best.map(|(_, candidate)| candidate)
        }
    }
}

/// The first enabled stop in a group, in tree order, through nested groups.
pub(super) fn first_enabled_stop(rects: &FocusRectList, group: usize) -> Option<usize> {
    candidates(rects, group)
        .into_iter()
        .find_map(|candidate| match candidate {
            Candidate::Rect(r) => (!rects.rects[r].disabled).then_some(r),
            Candidate::Group(g) => first_enabled_stop(rects, g),
        })
}
