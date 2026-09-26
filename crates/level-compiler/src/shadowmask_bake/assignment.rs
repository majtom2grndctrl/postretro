// Shadowmask overlap graph construction and deterministic four-channel assignment.
// See: context/lib/build_pipeline.md §PRL section IDs

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use postretro_level_format::shadowmask_atlas::SHADOWMASK_CHANNEL_DROPPED;

use crate::map_data::MapLight;

pub(super) const SHADOWMASK_COLOR_SEARCH_NODE_BUDGET: usize = 100_000;

const SHADOWMASK_ASSIGNMENT_CHECKPOINT_OPERATIONS: usize = 1024;

/// Full symmetric overlap matrix. One atomic byte per ordered light pair lets
/// chart workers union edges directly without per-chart graph copies.
pub(super) struct OverlapGraph {
    light_count: usize,
    edges: Vec<AtomicU8>,
    /// Most selected lights covering any one texel, as the analytic walk saw
    /// it. Reported, never acted on: it is the mask-capacity question's input.
    peak_texel_overlap: AtomicUsize,
}

const _: () = assert!(std::mem::size_of::<AtomicU8>() == 1);

impl OverlapGraph {
    pub(super) fn new(light_count: usize) -> Self {
        let edge_count = light_count
            .checked_mul(light_count)
            .expect("shadowmask overlap graph dimensions exceed addressable memory");
        Self {
            light_count,
            edges: (0..edge_count).map(|_| AtomicU8::new(0)).collect(),
            peak_texel_overlap: AtomicUsize::new(0),
        }
    }

    pub(super) fn record_texel_overlap(&self, covering_lights: usize) {
        self.peak_texel_overlap
            .fetch_max(covering_lights, Ordering::Relaxed);
    }

    pub(super) fn peak_texel_overlap(&self) -> u32 {
        u32::try_from(self.peak_texel_overlap.load(Ordering::Relaxed))
            .expect("selected light count fits the u32 selection table")
    }

    pub(super) fn light_count(&self) -> usize {
        self.light_count
    }

    pub(super) fn storage_bytes(&self) -> usize {
        self.edges.len() * std::mem::size_of::<AtomicU8>()
    }

    pub(super) fn mark_overlap(&self, a: usize, b: usize) {
        if a == b {
            return;
        }
        self.edges[a * self.light_count + b].store(1, Ordering::Relaxed);
        self.edges[b * self.light_count + a].store(1, Ordering::Relaxed);
    }

    pub(super) fn overlaps(&self, a: usize, b: usize) -> bool {
        self.edges[a * self.light_count + b].load(Ordering::Relaxed) != 0
    }

    #[cfg(test)]
    pub(super) fn snapshot(&self) -> Vec<u8> {
        self.edges
            .iter()
            .map(|edge| edge.load(Ordering::Relaxed))
            .collect()
    }

    #[cfg(test)]
    pub(super) fn from_edges(light_count: usize, edges: &[(usize, usize)]) -> Self {
        let graph = Self::new(light_count);
        for &(a, b) in edges {
            graph.mark_overlap(a, b);
        }
        graph
    }
}

fn record_assignment_operation(operation_count: &mut usize, checkpoint: &mut impl FnMut()) {
    *operation_count = (*operation_count).saturating_add(1);
    if *operation_count % SHADOWMASK_ASSIGNMENT_CHECKPOINT_OPERATIONS == 0 {
        checkpoint();
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ChannelAssignment {
    pub(super) channels: Vec<u8>,
    pub(super) exact_nodes_visited: usize,
    pub(super) fallback_lights_considered: usize,
    pub(super) fallback_operations: usize,
    pub(super) used_fallback: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExactColorResult {
    Colored,
    Uncolorable,
    BudgetExhausted,
}

pub(super) struct ExactColorBudget {
    remaining: usize,
    pub(super) visited: usize,
}

impl ExactColorBudget {
    pub(super) fn new(nodes: usize) -> Self {
        Self {
            remaining: nodes,
            visited: 0,
        }
    }

    fn visit(&mut self, operation_count: &mut usize, checkpoint: &mut impl FnMut()) -> bool {
        if self.remaining == 0 {
            return false;
        }
        self.remaining -= 1;
        self.visited += 1;
        record_assignment_operation(operation_count, checkpoint);
        true
    }
}

pub(super) fn assign_channels_with_drops_controlled(
    graph: &OverlapGraph,
    selected: &[(usize, u32, &MapLight)],
    exact_node_budget: usize,
    mut checkpoint: impl FnMut(),
) -> ChannelAssignment {
    let mut active = vec![true; graph.light_count()];
    let mut budget = ExactColorBudget::new(exact_node_budget);
    let mut operation_count = 0;

    if exact_node_budget != 0 {
        loop {
            checkpoint();
            let (result, channels) = color_graph_exact_bounded(
                graph,
                &active,
                &mut budget,
                &mut operation_count,
                &mut checkpoint,
            );
            match result {
                ExactColorResult::Colored => {
                    return finish_channel_assignment(
                        channels,
                        &active,
                        budget.visited,
                        0,
                        0,
                        false,
                    );
                }
                ExactColorResult::Uncolorable => {
                    let Some(drop_index) = lowest_intensity_active(selected, &active) else {
                        return finish_channel_assignment(
                            vec![SHADOWMASK_CHANNEL_DROPPED; graph.light_count()],
                            &active,
                            budget.visited,
                            0,
                            0,
                            false,
                        );
                    };
                    active[drop_index] = false;
                }
                ExactColorResult::BudgetExhausted => {
                    log::warn!(
                        "[ShadowmaskAtlas] exact channel search exhausted its deterministic {exact_node_budget}-node budget; using conservative fallback"
                    );
                    break;
                }
            }
        }
    } else {
        log::warn!(
            "[ShadowmaskAtlas] exact channel search exhausted its deterministic 0-node budget; using conservative fallback"
        );
    }

    checkpoint();
    let fallback_start_operations = operation_count;
    let (channels, fallback_lights_considered) = color_graph_priority_greedy(
        graph,
        selected,
        &mut active,
        &mut operation_count,
        &mut checkpoint,
    );
    finish_channel_assignment(
        channels,
        &active,
        budget.visited,
        fallback_lights_considered,
        operation_count.saturating_sub(fallback_start_operations),
        true,
    )
}

fn finish_channel_assignment(
    channels: Vec<u8>,
    active: &[bool],
    exact_nodes_visited: usize,
    fallback_lights_considered: usize,
    fallback_operations: usize,
    used_fallback: bool,
) -> ChannelAssignment {
    let dropped: Vec<usize> = active
        .iter()
        .enumerate()
        .filter_map(|(i, &is_active)| (!is_active).then_some(i))
        .collect();
    if !dropped.is_empty() {
        log::warn!(
            "[ShadowmaskAtlas] dropped {} selected light mask(s) to obtain a four-channel assignment: {:?}",
            dropped.len(),
            dropped
        );
    }
    ChannelAssignment {
        channels,
        exact_nodes_visited,
        fallback_lights_considered,
        fallback_operations,
        used_fallback,
    }
}

fn lowest_intensity_active(selected: &[(usize, u32, &MapLight)], active: &[bool]) -> Option<usize> {
    active
        .iter()
        .enumerate()
        .filter(|&(_, is_active)| *is_active)
        .min_by(|&(a, _), &(b, _)| light_drop_priority_cmp(selected, a, b))
        .map(|(i, _)| i)
}

fn light_drop_priority_cmp(
    selected: &[(usize, u32, &MapLight)],
    a: usize,
    b: usize,
) -> std::cmp::Ordering {
    light_intensity_score(selected[a].2)
        .total_cmp(&light_intensity_score(selected[b].2))
        .then(selected[a].0.cmp(&selected[b].0))
        .then(selected[a].1.cmp(&selected[b].1))
}

fn light_intensity_score(light: &MapLight) -> f32 {
    light.intensity * light.color[0].max(light.color[1]).max(light.color[2])
}

pub(super) fn color_graph_exact_bounded(
    graph: &OverlapGraph,
    active: &[bool],
    budget: &mut ExactColorBudget,
    operation_count: &mut usize,
    checkpoint: &mut impl FnMut(),
) -> (ExactColorResult, Vec<u8>) {
    let degrees = active_degrees(graph, active, operation_count, checkpoint);
    let mut order: Vec<usize> = active
        .iter()
        .enumerate()
        .filter_map(|(i, &is_active)| is_active.then_some(i))
        .collect();
    order.sort_by(|&a, &b| degrees[b].cmp(&degrees[a]).then(a.cmp(&b)));

    let mut channels = vec![SHADOWMASK_CHANNEL_DROPPED; graph.light_count()];
    let result = color_order_exact_bounded_iterative(
        graph,
        active,
        &order,
        &mut channels,
        budget,
        operation_count,
        checkpoint,
    );
    (result, channels)
}

#[derive(Clone, Copy)]
struct ExactColorFrame {
    cursor: usize,
    next_channel: u8,
    entered: bool,
}

fn color_order_exact_bounded_iterative(
    graph: &OverlapGraph,
    active: &[bool],
    order: &[usize],
    channels: &mut [u8],
    budget: &mut ExactColorBudget,
    operation_count: &mut usize,
    checkpoint: &mut impl FnMut(),
) -> ExactColorResult {
    let capacity = order
        .len()
        .saturating_add(1)
        .min(budget.remaining.saturating_add(1));
    let mut stack = Vec::with_capacity(capacity);
    stack.push(ExactColorFrame {
        cursor: 0,
        next_channel: 0,
        entered: false,
    });

    loop {
        record_assignment_operation(operation_count, checkpoint);
        let Some(frame) = stack.last_mut() else {
            return ExactColorResult::Uncolorable;
        };
        if !frame.entered {
            if !budget.visit(operation_count, checkpoint) {
                return ExactColorResult::BudgetExhausted;
            }
            frame.entered = true;
            if frame.cursor == order.len() {
                return ExactColorResult::Colored;
            }
        }

        let cursor = frame.cursor;
        let light = order[cursor];
        let mut selected_channel = None;
        while frame.next_channel < 4 {
            let channel = frame.next_channel;
            frame.next_channel += 1;
            let mut used_by_neighbor = false;
            for other in 0..graph.light_count() {
                record_assignment_operation(operation_count, checkpoint);
                if other != light
                    && active[other]
                    && graph.overlaps(light, other)
                    && channels[other] == channel
                {
                    used_by_neighbor = true;
                    break;
                }
            }
            if !used_by_neighbor {
                selected_channel = Some(channel);
                break;
            }
        }

        if let Some(channel) = selected_channel {
            channels[light] = channel;
            stack.push(ExactColorFrame {
                cursor: cursor + 1,
                next_channel: 0,
                entered: false,
            });
            continue;
        }

        channels[light] = SHADOWMASK_CHANNEL_DROPPED;
        stack.pop();
        let Some(parent) = stack.last() else {
            return ExactColorResult::Uncolorable;
        };
        if parent.cursor < order.len() {
            channels[order[parent.cursor]] = SHADOWMASK_CHANNEL_DROPPED;
        }
    }
}

pub(super) fn color_graph_priority_greedy(
    graph: &OverlapGraph,
    selected: &[(usize, u32, &MapLight)],
    active: &mut [bool],
    operation_count: &mut usize,
    checkpoint: &mut impl FnMut(),
) -> (Vec<u8>, usize) {
    let mut order: Vec<usize> = active
        .iter()
        .enumerate()
        .filter_map(|(index, &is_active)| is_active.then_some(index))
        .collect();
    order.sort_by(|&a, &b| light_drop_priority_cmp(selected, b, a));

    let mut channels = vec![SHADOWMASK_CHANNEL_DROPPED; graph.light_count()];
    for &light in &order {
        record_assignment_operation(operation_count, checkpoint);
        let mut used = [false; 4];
        for (other, &channel) in channels.iter().enumerate() {
            record_assignment_operation(operation_count, checkpoint);
            if other != light && graph.overlaps(light, other) {
                let channel = channel as usize;
                if channel < used.len() {
                    used[channel] = true;
                }
            }
        }
        if let Some(channel) = used.iter().position(|&is_used| !is_used) {
            channels[light] = channel as u8;
        } else {
            active[light] = false;
        }
    }

    (channels, order.len())
}

fn active_degrees(
    graph: &OverlapGraph,
    active: &[bool],
    operation_count: &mut usize,
    checkpoint: &mut impl FnMut(),
) -> Vec<usize> {
    let mut degrees = vec![0; graph.light_count()];
    for light in 0..graph.light_count() {
        if !active[light] {
            continue;
        }
        for (other, &other_is_active) in active.iter().enumerate() {
            record_assignment_operation(operation_count, checkpoint);
            if other != light && other_is_active && graph.overlaps(light, other) {
                degrees[light] += 1;
            }
        }
    }
    degrees
}
