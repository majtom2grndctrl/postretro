// Coverage proof matching: one side of a piece covered one-for-one by the other side's edges.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use std::collections::HashMap;

/// One maximum matching from a piece's left slots into its right slots.
pub(super) struct Cover {
    /// Left slots past the leading ones that it leaves uncovered.
    pub(super) uncovered: Vec<usize>,
    /// Each left slot's right partner.
    pub(super) partner: Vec<Option<usize>>,
}

/// Match left slots into `right` slots, where `candidates[l]` lists the
/// right slots left slot `l` may pair with and `pairs(l, r)` says whether
/// they do. The first `lead` left slots augment first, then each `start`
/// pair that pairs is taken where both slots are still free, then every
/// other free left slot augments, in order. An augmenting path never
/// uncovers a left slot, and a slot with no augmenting path never gains
/// one, so the matching is maximum whatever the start, and a leading slot
/// that is covered once stays covered. `pairs` is read lazily, at most once
/// per pair.
pub(super) fn cover(
    candidates: &[Vec<usize>],
    right: usize,
    lead: usize,
    start: &[(usize, usize)],
    pairs: impl FnMut(usize, usize) -> bool,
) -> Cover {
    let left = candidates.len();
    let mut graph = Lazy {
        pairs,
        known: HashMap::new(),
        candidates,
    };
    let mut left_of: Vec<Option<usize>> = vec![None; right];
    let lead = lead.min(left);
    for l in 0..lead {
        graph.augment(l, &mut vec![false; right], &mut left_of);
    }
    let mut taken = vec![false; left];
    for l in left_of.iter().flatten() {
        taken[*l] = true;
    }
    for &(l, r) in start {
        if !taken[l] && left_of[r].is_none() && graph.pair(l, r) {
            taken[l] = true;
            left_of[r] = Some(l);
        }
    }
    let free: Vec<usize> = (lead..left).filter(|&l| !taken[l]).collect();
    for l in free {
        graph.augment(l, &mut vec![false; right], &mut left_of);
    }
    let mut partner = vec![None; left];
    for (r, l) in left_of.iter().enumerate() {
        if let Some(l) = *l {
            partner[l] = Some(r);
        }
    }
    // A leading slot left free fails every other slot: never expected,
    // and failing toward unproven costs a false hole, never a missed one.
    let lead_held = (0..lead).all(|l| partner[l].is_some());
    Cover {
        uncovered: (lead..left)
            .filter(|&l| !lead_held || partner[l].is_none())
            .collect(),
        partner,
    }
}

struct Lazy<'c, F> {
    pairs: F,
    known: HashMap<(usize, usize), bool>,
    candidates: &'c [Vec<usize>],
}

impl<F: FnMut(usize, usize) -> bool> Lazy<'_, F> {
    fn pair(&mut self, l: usize, r: usize) -> bool {
        let pairs = &mut self.pairs;
        *self.known.entry((l, r)).or_insert_with(|| pairs(l, r))
    }

    fn augment(&mut self, l: usize, visited: &mut [bool], left_of: &mut [Option<usize>]) -> bool {
        for k in 0..self.candidates[l].len() {
            let r = self.candidates[l][k];
            if visited[r] || !self.pair(l, r) {
                continue;
            }
            visited[r] = true;
            let free = match left_of[r] {
                None => true,
                Some(other) => self.augment(other, visited, left_of),
            };
            if free {
                left_of[r] = Some(l);
                return true;
            }
        }
        false
    }
}
