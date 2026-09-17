//! Pointer-free hierarchy projection over the base SH affinity grid.
//!
//! Phase 1 uses this module only for analysis: it changes no emitted bytes.
//! The merge topology is deliberately independent from tile math; callers
//! provide the exact node-level reconstruction gate used by their data window.

use postretro_level_format::sh_reconstruct::Level;

pub(crate) const MAX_NODE_SCALE: u8 = 3;

#[derive(Clone, Copy, Debug)]
pub(crate) struct BrickInput {
    pub level: Level,
    pub participates: bool,
    pub partial: bool,
    pub protected: bool,
    pub has_delta_entry: bool,
    /// Stored slots for L0/L1/L2 at scale zero.
    pub stored_tiles: [u32; 3],
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NodeEvaluation {
    pub passes: bool,
    pub darkness_bypass: bool,
    pub rel_p95: f32,
    pub rel_max: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Assignment {
    pub origin: [u32; 3],
    pub scale: u8,
    pub level: Level,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct NodeRecord {
    pub origin: [u32; 3],
    pub scale: u8,
    pub level: Level,
    pub brick_count: u32,
    pub stored_tiles: u32,
    pub evaluation: NodeEvaluation,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BlockCounts {
    pub delta: u64,
    pub protection: u64,
    pub partial: u64,
    pub member_shape: u64,
    pub gate: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct Projection {
    pub assignments: Vec<Assignment>,
    pub nodes: Vec<NodeRecord>,
    pub histogram: [[u64; 3]; (MAX_NODE_SCALE as usize) + 1],
    pub shipped_stored_tiles: u64,
    pub projected_stored_tiles: u64,
    pub saved_l1_tiles: u64,
    pub saved_l2_tiles: u64,
    pub merged_by_darkness_bypass: u64,
    pub blocks: BlockCounts,
    pub smoothing_demotions: u64,
}

pub(crate) fn project<F>(
    dimensions: [u32; 3],
    bricks: &[BrickInput],
    max_scale: u8,
    mut evaluate: F,
) -> Result<Projection, String>
where
    F: FnMut([u32; 3], u8, Level) -> NodeEvaluation,
{
    if max_scale > MAX_NODE_SCALE {
        return Err(format!(
            "SH hierarchy maximum scale {max_scale} exceeds {MAX_NODE_SCALE}"
        ));
    }
    let brick_count = checked_count(dimensions)?;
    if bricks.len() != brick_count {
        return Err(format!(
            "SH hierarchy has {} brick inputs for dimensions {dimensions:?}, expected {brick_count}",
            bricks.len()
        ));
    }

    let mut assignments = Vec::with_capacity(brick_count);
    for z in 0..dimensions[2] {
        for y in 0..dimensions[1] {
            for x in 0..dimensions[0] {
                let brick = &bricks[index([x, y, z], dimensions)];
                assignments.push(Assignment {
                    origin: [x, y, z],
                    scale: 0,
                    level: brick.level,
                });
            }
        }
    }

    let mut evaluations = vec![NodeEvaluation::default(); brick_count];
    let mut blocks = BlockCounts::default();
    let mut merged_by_darkness_bypass = 0u64;
    let mut smoothing_demotions = 0u64;

    for scale in 1..=max_scale {
        let edge = 1u32 << scale;
        let child_scale = scale - 1;
        let mut z = 0;
        while z + edge <= dimensions[2] {
            let mut y = 0;
            while y + edge <= dimensions[1] {
                let mut x = 0;
                while x + edge <= dimensions[0] {
                    let origin = [x, y, z];
                    debug_assert!(candidate_is_aligned(origin, scale));
                    let members = member_indices(origin, edge, dimensions);

                    if members.iter().any(|&member| bricks[member].has_delta_entry) {
                        blocks.delta += 1;
                        x += edge;
                        continue;
                    }
                    if members.iter().any(|&member| bricks[member].protected) {
                        blocks.protection += 1;
                        x += edge;
                        continue;
                    }
                    if members.iter().any(|&member| bricks[member].partial) {
                        blocks.partial += 1;
                        x += edge;
                        continue;
                    }
                    if members.iter().any(|&member| !bricks[member].participates) {
                        blocks.member_shape += 1;
                        x += edge;
                        continue;
                    }

                    let first = assignments[members[0]];
                    if first.level == Level::L0
                        || first.scale != child_scale
                        || members.iter().any(|&member| {
                            let assignment = assignments[member];
                            assignment.scale != child_scale || assignment.level != first.level
                        })
                    {
                        blocks.member_shape += 1;
                        x += edge;
                        continue;
                    }

                    let evaluation = evaluate(origin, scale, first.level);
                    if !evaluation.passes {
                        blocks.gate += 1;
                        x += edge;
                        continue;
                    }
                    if evaluation.darkness_bypass {
                        merged_by_darkness_bypass += 1;
                    }
                    for &member in &members {
                        assignments[member] = Assignment {
                            origin,
                            scale,
                            level: first.level,
                        };
                        evaluations[member] = evaluation;
                    }
                    x += edge;
                }
                y += edge;
            }
            z += edge;
        }

        smoothing_demotions += smooth_levels(
            dimensions,
            bricks,
            &mut assignments,
            &mut evaluations,
            &mut evaluate,
        )?;
    }

    let shipped_stored_tiles = bricks
        .iter()
        .filter(|brick| brick.participates)
        .map(|brick| u64::from(brick.stored_tiles[brick.level.to_u8() as usize]))
        .sum();
    let mut nodes = Vec::new();
    let mut histogram = [[0u64; 3]; (MAX_NODE_SCALE as usize) + 1];
    let mut projected_stored_tiles = 0u64;
    let mut saved_l1_tiles = 0u64;
    let mut saved_l2_tiles = 0u64;

    for z in 0..dimensions[2] {
        for y in 0..dimensions[1] {
            for x in 0..dimensions[0] {
                let coord = [x, y, z];
                let brick_index = index(coord, dimensions);
                if !bricks[brick_index].participates {
                    continue;
                }
                let assignment = assignments[brick_index];
                if assignment.origin != coord {
                    continue;
                }
                let edge = 1u32 << assignment.scale;
                let members = member_indices(assignment.origin, edge, dimensions);
                let stored_tiles = if assignment.scale == 0 {
                    bricks[brick_index].stored_tiles[assignment.level.to_u8() as usize]
                } else {
                    match assignment.level {
                        Level::L0 => {
                            return Err("SH hierarchy produced L0 at nonzero scale".to_string());
                        }
                        Level::L1 => 8,
                        Level::L2 => 1,
                    }
                };
                let before: u64 = members
                    .iter()
                    .map(|&member| {
                        u64::from(
                            bricks[member].stored_tiles[bricks[member].level.to_u8() as usize],
                        )
                    })
                    .sum();
                let saved = before.saturating_sub(u64::from(stored_tiles));
                match assignment.level {
                    Level::L1 => saved_l1_tiles += saved,
                    Level::L2 => saved_l2_tiles += saved,
                    Level::L0 => {}
                }
                histogram[assignment.scale as usize][assignment.level.to_u8() as usize] += 1;
                projected_stored_tiles += u64::from(stored_tiles);
                nodes.push(NodeRecord {
                    origin: assignment.origin,
                    scale: assignment.scale,
                    level: assignment.level,
                    brick_count: edge * edge * edge,
                    stored_tiles,
                    evaluation: evaluations[brick_index],
                });
            }
        }
    }

    Ok(Projection {
        assignments,
        nodes,
        histogram,
        shipped_stored_tiles,
        projected_stored_tiles,
        saved_l1_tiles,
        saved_l2_tiles,
        merged_by_darkness_bypass,
        blocks,
        smoothing_demotions,
    })
}

fn smooth_levels<F>(
    dimensions: [u32; 3],
    bricks: &[BrickInput],
    assignments: &mut [Assignment],
    evaluations: &mut [NodeEvaluation],
    evaluate: &mut F,
) -> Result<u64, String>
where
    F: FnMut([u32; 3], u8, Level) -> NodeEvaluation,
{
    let mut demotions = 0u64;
    loop {
        let mut changed = false;
        for z in 0..dimensions[2] {
            for y in 0..dimensions[1] {
                for x in 0..dimensions[0] {
                    let a_coord = [x, y, z];
                    let a_index = index(a_coord, dimensions);
                    if !bricks[a_index].participates {
                        continue;
                    }
                    for axis in 0..3 {
                        let mut b_coord = a_coord;
                        b_coord[axis] += 1;
                        if b_coord[axis] >= dimensions[axis] {
                            continue;
                        }
                        let b_index = index(b_coord, dimensions);
                        if !bricks[b_index].participates {
                            continue;
                        }
                        let a_level = assignments[a_index].level.to_u8();
                        let b_level = assignments[b_index].level.to_u8();
                        if a_level.abs_diff(b_level) <= 1 {
                            continue;
                        }
                        let coarse = if a_level > b_level { a_index } else { b_index };
                        demote_node(dimensions, assignments, evaluations, coarse, evaluate)?;
                        demotions += 1;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            return Ok(demotions);
        }
    }
}

fn demote_node<F>(
    dimensions: [u32; 3],
    assignments: &mut [Assignment],
    evaluations: &mut [NodeEvaluation],
    member: usize,
    evaluate: &mut F,
) -> Result<(), String>
where
    F: FnMut([u32; 3], u8, Level) -> NodeEvaluation,
{
    let assignment = assignments[member];
    let edge = 1u32 << assignment.scale;
    let members = member_indices(assignment.origin, edge, dimensions);
    let next_level = match assignment.level {
        Level::L2 => {
            let l1 = evaluate(assignment.origin, assignment.scale, Level::L1);
            if l1.passes {
                for &index in &members {
                    assignments[index].level = Level::L1;
                    evaluations[index] = l1;
                }
                return Ok(());
            }
            Level::L0
        }
        Level::L1 => Level::L0,
        Level::L0 => return Ok(()),
    };

    if next_level == Level::L0 && assignment.scale > 0 {
        for &member in &members {
            let coord = coord(member, dimensions);
            assignments[member] = Assignment {
                origin: coord,
                scale: 0,
                level: Level::L0,
            };
            evaluations[member] = NodeEvaluation::default();
        }
    } else {
        for &member in &members {
            assignments[member].level = next_level;
        }
    }
    Ok(())
}

pub(crate) fn candidate_is_aligned(origin: [u32; 3], scale: u8) -> bool {
    let edge = 1u32 << scale;
    origin.iter().all(|axis| axis % edge == 0)
}

fn member_indices(origin: [u32; 3], edge: u32, dimensions: [u32; 3]) -> Vec<usize> {
    let mut members = Vec::with_capacity((edge * edge * edge) as usize);
    for z in origin[2]..origin[2] + edge {
        for y in origin[1]..origin[1] + edge {
            for x in origin[0]..origin[0] + edge {
                members.push(index([x, y, z], dimensions));
            }
        }
    }
    members
}

fn checked_count(dimensions: [u32; 3]) -> Result<usize, String> {
    dimensions
        .iter()
        .try_fold(1usize, |count, &dimension| {
            count.checked_mul(dimension as usize)
        })
        .ok_or_else(|| format!("SH hierarchy dimensions {dimensions:?} overflow usize"))
}

fn index(coord: [u32; 3], dimensions: [u32; 3]) -> usize {
    coord[0] as usize
        + coord[1] as usize * dimensions[0] as usize
        + coord[2] as usize * dimensions[0] as usize * dimensions[1] as usize
}

fn coord(index: usize, dimensions: [u32; 3]) -> [u32; 3] {
    let x = index % dimensions[0] as usize;
    let y = (index / dimensions[0] as usize) % dimensions[1] as usize;
    let z = index / (dimensions[0] as usize * dimensions[1] as usize);
    [x as u32, y as u32, z as u32]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brick(level: Level) -> BrickInput {
        BrickInput {
            level,
            participates: true,
            partial: false,
            protected: false,
            has_delta_entry: false,
            stored_tiles: [64, 8, 1],
        }
    }

    fn passing(_: [u32; 3], _: u8, level: Level) -> NodeEvaluation {
        NodeEvaluation {
            passes: level != Level::L0,
            ..Default::default()
        }
    }

    #[test]
    fn aligned_same_level_bricks_merge_and_scale_zero_preserves_histogram() {
        let bricks = vec![brick(Level::L2); 8];
        let unchanged = project([2, 2, 2], &bricks, 0, passing).unwrap();
        assert_eq!(unchanged.histogram[0][Level::L2.to_u8() as usize], 8);
        assert_eq!(
            unchanged.projected_stored_tiles,
            unchanged.shipped_stored_tiles
        );

        let merged = project([2, 2, 2], &bricks, 1, passing).unwrap();
        assert_eq!(merged.histogram[1][Level::L2.to_u8() as usize], 1);
        assert_eq!(merged.projected_stored_tiles, 1);
        assert_eq!(merged.saved_l2_tiles, 7);
    }

    #[test]
    fn delta_protection_partial_failed_gate_and_misalignment_block_merge() {
        for blocker in 0..3 {
            let mut bricks = vec![brick(Level::L1); 8];
            match blocker {
                0 => bricks[0].has_delta_entry = true,
                1 => bricks[0].protected = true,
                2 => bricks[0].partial = true,
                _ => unreachable!(),
            }
            let projection = project([2, 2, 2], &bricks, 1, passing).unwrap();
            assert_eq!(projection.histogram[0][Level::L1.to_u8() as usize], 8);
        }
        let failed = project([2, 2, 2], &vec![brick(Level::L1); 8], 1, |_, _, _| {
            NodeEvaluation::default()
        })
        .unwrap();
        assert_eq!(failed.blocks.gate, 1);
        assert!(!candidate_is_aligned([1, 0, 0], 1));
    }

    #[test]
    fn recursive_scale_two_merge_requires_all_scale_one_children() {
        let bricks = vec![brick(Level::L2); 64];
        let merged = project([4, 4, 4], &bricks, 2, passing).unwrap();
        assert_eq!(merged.histogram[2][Level::L2.to_u8() as usize], 1);

        for blocker in 0..3 {
            let mut blocked = bricks.clone();
            match blocker {
                0 => blocked[0].has_delta_entry = true,
                1 => blocked[0].protected = true,
                2 => {
                    let mut calls = 0;
                    let projection = project([4, 4, 4], &blocked, 2, |origin, scale, level| {
                        calls += 1;
                        let pass = !(origin == [0, 0, 0] && scale == 1);
                        NodeEvaluation {
                            passes: pass && level != Level::L0,
                            ..Default::default()
                        }
                    })
                    .unwrap();
                    assert!(calls > 0);
                    assert_eq!(projection.histogram[2][Level::L2.to_u8() as usize], 0);
                    continue;
                }
                _ => unreachable!(),
            }
            let projection = project([4, 4, 4], &blocked, 2, passing).unwrap();
            assert_eq!(projection.histogram[2][Level::L2.to_u8() as usize], 0);
        }
    }

    #[test]
    fn recursive_no_merge_keeps_face_level_bound_without_raising_levels() {
        let mut bricks = vec![brick(Level::L1); 64];
        bricks[0].has_delta_entry = true;
        let before: Vec<u8> = bricks.iter().map(|brick| brick.level.to_u8()).collect();
        let projection = project([4, 4, 4], &bricks, 2, passing).unwrap();
        assert_eq!(projection.histogram[2][Level::L1.to_u8() as usize], 0);
        for (assignment, original) in projection.assignments.iter().zip(before) {
            assert!(assignment.level.to_u8() <= original);
        }
        assert_face_level_bound([4, 4, 4], &bricks, &projection.assignments);
    }

    fn assert_face_level_bound(
        dimensions: [u32; 3],
        bricks: &[BrickInput],
        assignments: &[Assignment],
    ) {
        for z in 0..dimensions[2] {
            for y in 0..dimensions[1] {
                for x in 0..dimensions[0] {
                    let a = index([x, y, z], dimensions);
                    if !bricks[a].participates {
                        continue;
                    }
                    for axis in 0..3 {
                        let mut neighbor = [x, y, z];
                        neighbor[axis] += 1;
                        if neighbor[axis] >= dimensions[axis] {
                            continue;
                        }
                        let b = index(neighbor, dimensions);
                        if bricks[b].participates {
                            assert!(
                                assignments[a]
                                    .level
                                    .to_u8()
                                    .abs_diff(assignments[b].level.to_u8())
                                    <= 1
                            );
                        }
                    }
                }
            }
        }
    }
}
