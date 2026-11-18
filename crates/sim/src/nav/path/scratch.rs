// Reusable A* search state: per-node and per-link arrays sized to the largest
// graph seen, reset lazily by a generation stamp, so a route query touches only
// the nodes it visits instead of zeroing graph-sized arrays first.
// See: context/lib/build_pipeline.md §Navigation bake

use std::collections::BinaryHeap;

use super::Frontier;

/// Search scratch for route queries. One query runs at a time per scratch; a
/// caller keeps one per pass (or per worker) and lends it to each query. Its
/// contents never carry from one query to the next: every per-node and
/// per-link field reads as unvisited until the current query writes it.
#[derive(Default)]
pub struct NavScratch {
    /// The current query's stamp. A field whose stamp differs is unvisited.
    generation: u32,
    node_stamps: Vec<u32>,
    g_score: Vec<f32>,
    /// `Some(previous_node)`, or `None` when reached from the virtual start.
    came_from: Vec<Option<usize>>,
    link_stamps: Vec<u32>,
    /// The dock a mover carries the body from before this link's exit node.
    rode_from: Vec<Option<u32>>,
    pub(super) open: BinaryHeap<Frontier>,
    /// Per-node and per-link fields reset by the current query (first touch).
    #[cfg(test)]
    pub(super) resets: usize,
}

impl std::fmt::Debug for NavScratch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NavScratch")
            .field("generation", &self.generation)
            .field("nodes", &self.node_stamps.len())
            .field("links", &self.link_stamps.len())
            .finish_non_exhaustive()
    }
}

impl NavScratch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a query over `node_count` nodes and `link_count` links. Growth
    /// allocates once per larger graph; nothing is cleared per query except
    /// when the stamp wraps.
    pub(super) fn begin(&mut self, node_count: usize, link_count: usize) {
        if self.node_stamps.len() < node_count {
            self.node_stamps.resize(node_count, 0);
            self.g_score.resize(node_count, f32::INFINITY);
            self.came_from.resize(node_count, None);
        }
        if self.link_stamps.len() < link_count {
            self.link_stamps.resize(link_count, 0);
            self.rode_from.resize(link_count, None);
        }
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            // A stamp left from 2^32 queries ago would read as current.
            self.node_stamps.fill(0);
            self.link_stamps.fill(0);
            self.generation = 1;
        }
        self.open.clear();
        #[cfg(test)]
        {
            self.resets = 0;
        }
    }

    pub(super) fn g(&self, node: usize) -> f32 {
        if self.node_stamps[node] == self.generation {
            self.g_score[node]
        } else {
            f32::INFINITY
        }
    }

    pub(super) fn came_from(&self, node: usize) -> Option<usize> {
        if self.node_stamps[node] == self.generation {
            self.came_from[node]
        } else {
            None
        }
    }

    pub(super) fn rode_from(&self, link: usize) -> Option<u32> {
        if self.link_stamps[link] == self.generation {
            self.rode_from[link]
        } else {
            None
        }
    }

    fn touch_node(&mut self, node: usize) {
        if self.node_stamps[node] != self.generation {
            self.node_stamps[node] = self.generation;
            self.g_score[node] = f32::INFINITY;
            self.came_from[node] = None;
            #[cfg(test)]
            {
                self.resets += 1;
            }
        }
    }

    pub(super) fn set_g(&mut self, node: usize, g: f32) {
        self.touch_node(node);
        self.g_score[node] = g;
    }

    pub(super) fn set_came_from(&mut self, node: usize, previous: Option<usize>) {
        self.touch_node(node);
        self.came_from[node] = previous;
    }

    pub(super) fn set_rode_from(&mut self, link: usize, dock: Option<u32>) {
        if self.link_stamps[link] != self.generation {
            self.link_stamps[link] = self.generation;
            #[cfg(test)]
            {
                self.resets += 1;
            }
        }
        self.rode_from[link] = dock;
    }

    #[cfg(test)]
    pub(super) fn generation_for_test(&self) -> u32 {
        self.generation
    }

    #[cfg(test)]
    pub(super) fn set_generation_for_test(&mut self, generation: u32) {
        self.generation = generation;
    }
}
