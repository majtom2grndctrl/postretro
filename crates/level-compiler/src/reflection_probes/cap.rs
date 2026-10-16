// Reflection-probe cap: at most the probe array's layer limit, once on placement before capture and once after splitting.
// See: context/lib/resource_management.md §5.1

use super::placement::PlacedProbe;
use super::split::StageProbe;

/// Which of `count` probes stay under `cap`, or `None` when all fit. The
/// first `authored` always stay: placement refused the build past the cap.
/// The rest rank by `area`, largest first, ties to the earlier probe in wire
/// order.
fn largest_first(
    count: usize,
    authored: usize,
    cap: usize,
    area: impl Fn(usize) -> f64,
) -> Option<Vec<bool>> {
    let room = cap.saturating_sub(authored);
    let mut ranked: Vec<usize> = (authored..count).collect();
    if ranked.len() <= room {
        return None;
    }
    ranked.sort_by(|&a, &b| area(b).total_cmp(&area(a)).then(a.cmp(&b)));
    let mut keep = vec![false; count];
    keep[..authored].fill(true);
    for &i in &ranked[..room] {
        keep[i] = true;
    }
    Some(keep)
}

/// The pre-capture cap: derived probes past `cap`, ranked by the bound area
/// placement gave them, drop before any capture, so the stage never captures
/// more probes than the cap before splitting. A dropped derived probe's
/// faces bind nothing. Returns how many dropped.
pub(crate) fn cap_placed_probes(
    probes: &mut Vec<PlacedProbe>,
    authored: usize,
    cap: usize,
) -> usize {
    let Some(keep) = largest_first(probes.len(), authored, cap, |i| probes[i].bound_area) else {
        return 0;
    };
    let before = probes.len();
    let mut keep = keep.into_iter();
    probes.retain(|_| keep.next().expect("one flag per probe"));
    before - probes.len()
}

/// The post-split cap: keep at most `cap` probes. The first `authored` always
/// stay. The rest, derived and split, rank by bound surface area as
/// splitting left it, largest first, ties to the earlier probe in wire order.
/// A dropped split probe's water returns to its nearest kept ancestor; a
/// dropped derived probe's binds nothing. Returns the kept probes' indices,
/// ascending, and how many dropped.
pub(crate) fn apply_probe_cap(
    probes: &mut [StageProbe],
    authored: usize,
    cap: usize,
) -> (Vec<usize>, usize) {
    let Some(keep) = largest_first(probes.len(), authored, cap, |i| probes[i].placed.bound_area)
    else {
        return ((0..probes.len()).collect(), 0);
    };
    for i in 0..probes.len() {
        if keep[i] {
            continue;
        }
        let mut ancestor = probes[i].parent;
        while let Some(a) = ancestor
            && !keep[a]
        {
            ancestor = probes[a].parent;
        }
        let faces = std::mem::take(&mut probes[i].placed.faces);
        let area = std::mem::take(&mut probes[i].placed.bound_area);
        if let Some(a) = ancestor {
            let heir = &mut probes[a].placed;
            heir.faces.extend(faces);
            heir.faces.sort_unstable();
            heir.bound_area += area;
        }
    }
    let kept: Vec<usize> = (0..probes.len()).filter(|&i| keep[i]).collect();
    let dropped = probes.len() - kept.len();
    (kept, dropped)
}

/// The one cap warning, naming the total dropped count: before capture and
/// after splitting. A warm build replays it: placement reruns the first, and
/// the cache entry carries the second.
pub(crate) fn warn_capped(dropped: usize, cap: usize) {
    if dropped > 0 {
        log::warn!(
            "[ReflectionProbes] {dropped} derived or split probe(s) past the {cap}-probe cap \
             dropped, smallest bound water first; a split probe's water returns to the probe it \
             split from, and a derived probe's faces reflect nothing"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(area: f64, face: u32, parent: Option<usize>) -> StageProbe {
        StageProbe {
            placed: PlacedProbe {
                source_entity: 1,
                box_min: [0.0; 3],
                box_max: [1.0; 3],
                capture_point: [0.5; 3],
                bound_area: area,
                faces: vec![(0, face)],
            },
            parent,
        }
    }

    // S6: split probes count toward the cap and drop smallest first, their
    // water returning to the nearest kept ancestor; authored probes never
    // drop, and a dropped derived probe's water binds nothing.
    #[test]
    fn cap_drops_split_probes_smallest_first_and_returns_their_water() {
        // 0 authored; 1 derived, split into 3, which split into 4; 2 derived,
        // split into 5.
        let mut probes = vec![
            probe(1.0, 0, None),
            probe(5.0, 1, None),
            probe(0.5, 2, None),
            probe(2.0, 3, Some(1)),
            probe(0.25, 4, Some(3)),
            probe(3.0, 5, Some(2)),
        ];
        let (kept, dropped) = apply_probe_cap(&mut probes, 1, 4);
        assert_eq!(kept, vec![0, 1, 3, 5]);
        assert_eq!(dropped, 2);
        // 4's water goes to 3, its parent; 2 is derived and drops with its own.
        assert_eq!(probes[3].placed.faces, vec![(0, 3), (0, 4)]);
        assert!((probes[3].placed.bound_area - 2.25).abs() < 1e-12);
        assert_eq!(probes[5].placed.faces, vec![(0, 5)]);
        assert!(probes[2].placed.faces.is_empty());

        // With room for only the authored probe and one more, 3 drops too and
        // both its water and 4's return to 1.
        let mut probes = vec![
            probe(1.0, 0, None),
            probe(5.0, 1, None),
            probe(2.0, 3, Some(1)),
            probe(0.25, 4, Some(2)),
        ];
        let (kept, dropped) = apply_probe_cap(&mut probes, 1, 2);
        assert_eq!((kept, dropped), (vec![0, 1], 2));
        assert_eq!(probes[1].placed.faces, vec![(0, 1), (0, 3), (0, 4)]);
    }
}
