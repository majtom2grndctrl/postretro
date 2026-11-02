// Coverage proof matching: one piece's edges paired one-for-one across directions, seam overlaps excused.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

/// More seam overlaps than this on one piece are not tried: the piece stays
/// unproven, which costs a false hole, never a missed one.
const SEAM_CAP: usize = 12;

/// One piece's candidate pairs, `pairs[left][right]`. Each side lists the
/// edges running over the piece first (`required`), then the stand-ins,
/// which may partner but need none.
pub(super) struct PieceGraph {
    pub(super) pairs: Vec<Vec<bool>>,
    pub(super) required: [usize; 2],
    pub(super) total: [usize; 2],
}

/// Nodes of one side, grouped: the first `required` groups need a partner.
struct Side {
    groups: Vec<Vec<usize>>,
    required: usize,
}

impl PieceGraph {
    /// Per side, the required edges one maximum one-for-one matching leaves
    /// uncovered; both empty exactly when some matching covers them all.
    pub(super) fn uncovered(&self) -> [Vec<usize>; 2] {
        let sides = [0, 1].map(|s| Side {
            groups: (0..self.total[s]).map(|k| vec![k]).collect(),
            required: self.required[s],
        });
        self.cover(&sides)
            .map(|covered| (0..covered.len()).filter(|&k| !covered[k]).collect())
    }

    /// Whether some choice of disjoint seam overlaps, per side, lets every
    /// required edge be covered. A chosen overlap joins its two edges into
    /// one node: one partner covers it, and pairs with whichever edge it
    /// pairs with, so the other is the one excused, and an excused edge never
    /// excuses another. Every choice is tried, so the verdict does not depend
    /// on order. `seams` hold pairs of required slots.
    pub(super) fn proven_with_seams(&self, seams: [&[(usize, usize)]; 2]) -> bool {
        if seams[0].len() + seams[1].len() > SEAM_CAP {
            return false;
        }
        let [left, right] = seams.map(disjoint_choices);
        left.iter().any(|l| {
            right.iter().any(|r| {
                if l.is_empty() && r.is_empty() {
                    return false;
                }
                let sides = [(0, l), (1, r)]
                    .map(|(s, chosen)| joined(self.required[s], self.total[s], chosen));
                self.cover(&sides).iter().flatten().all(|&c| c)
            })
        })
    }

    /// Per side, which required groups a maximum matching against every
    /// group of the other side covers. A matching covering each side's
    /// required groups exists exactly when one covering both does
    /// (Mendelsohn–Dulmage), so the sides are matched apart.
    fn cover(&self, sides: &[Side; 2]) -> [Vec<bool>; 2] {
        let linked =
            |l: &[usize], r: &[usize]| l.iter().any(|&a| r.iter().any(|&b| self.pairs[a][b]));
        let adjacency = |from: usize| -> Vec<Vec<usize>> {
            let (own, other) = (&sides[from], &sides[1 - from]);
            own.groups[..own.required]
                .iter()
                .map(|g| {
                    (0..other.groups.len())
                        .filter(|&o| {
                            if from == 0 {
                                linked(g, &other.groups[o])
                            } else {
                                linked(&other.groups[o], g)
                            }
                        })
                        .collect()
                })
                .collect()
        };
        [0, 1].map(|s| covered_by_matching(&adjacency(s), sides[1 - s].groups.len()))
    }
}

/// Every set of pairwise disjoint `seams`, the empty set first.
fn disjoint_choices(seams: &[(usize, usize)]) -> Vec<Vec<(usize, usize)>> {
    (0..1u32 << seams.len())
        .map(|mask| {
            (0..seams.len())
                .filter(|&k| mask & (1 << k) != 0)
                .map(|k| seams[k])
                .collect::<Vec<_>>()
        })
        .filter(|chosen| {
            let mut slots: Vec<usize> = chosen.iter().flat_map(|&(a, b)| [a, b]).collect();
            let all = slots.len();
            slots.sort_unstable();
            slots.dedup();
            slots.len() == all
        })
        .collect()
}

/// One side's groups with each `chosen` pair joined: the required edges
/// left single, then the joined pairs, then the stand-ins.
fn joined(required: usize, total: usize, chosen: &[(usize, usize)]) -> Side {
    let in_pair = |k: usize| chosen.iter().any(|&(a, b)| k == a || k == b);
    let mut groups: Vec<Vec<usize>> = (0..required)
        .filter(|&k| !in_pair(k))
        .map(|k| vec![k])
        .collect();
    groups.extend(chosen.iter().map(|&(a, b)| vec![a, b]));
    let required_groups = groups.len();
    groups.extend((required..total).map(|k| vec![k]));
    Side {
        groups,
        required: required_groups,
    }
}

/// Which left vertices a maximum matching covers, by Kuhn's augmenting
/// paths over `adjacency` (left → right slots, in order). The same input
/// covers the same vertices; an augmenting path never uncovers a left
/// vertex, so the count is the maximum.
fn covered_by_matching(adjacency: &[Vec<usize>], right_count: usize) -> Vec<bool> {
    fn augment(
        left: usize,
        adjacency: &[Vec<usize>],
        visited: &mut [bool],
        left_of: &mut [Option<usize>],
    ) -> bool {
        for &right in &adjacency[left] {
            if visited[right] {
                continue;
            }
            visited[right] = true;
            let free = match left_of[right] {
                None => true,
                Some(other) => augment(other, adjacency, visited, left_of),
            };
            if free {
                left_of[right] = Some(left);
                return true;
            }
        }
        false
    }

    let mut left_of = vec![None; right_count];
    (0..adjacency.len())
        .map(|left| {
            let mut visited = vec![false; right_count];
            augment(left, adjacency, &mut visited, &mut left_of)
        })
        .collect()
}
