//! The array-backed stored-node layout against the tree-keyed derivation it
//! replaced. `oracle_derive` is that derivation, kept verbatim (including its
//! per-probe `l0_local_slot` search and divide-based node origin).

use std::collections::BTreeMap;

use postretro_level_format::sh_volume::OctahedralShProbe;
use postretro_level_loader::ShStreamBaseMetadata;

use super::*;

pub(in crate::render::sh_streaming) struct Rng(pub(in crate::render::sh_streaming) u64);

impl Rng {
    pub(in crate::render::sh_streaming) fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub(in crate::render::sh_streaming) fn below(&mut self, bound: u32) -> u32 {
        ((self.next() >> 33) % u64::from(bound)) as u32
    }

    pub(in crate::render::sh_streaming) fn chance(&mut self, percent: u32) -> bool {
        self.below(100) < percent
    }
}

pub(in crate::render::sh_streaming) struct OracleLayout {
    pub(in crate::render::sh_streaming) nodes: Vec<Option<StoredNode>>,
    pub(in crate::render::sh_streaming) local_slots: Vec<Option<u32>>,
    pub(in crate::render::sh_streaming) layouts: BTreeMap<StoredNode, StoredNodeLayout>,
}

pub(in crate::render::sh_streaming) fn oracle_derive(
    base: &ShStreamBaseMetadata,
) -> Result<OracleLayout, ShResidencyDrainError> {
    let affinity_dims = base.grid_dimensions.map(|axis| axis.div_ceil(4));
    let brick_count = affinity_dims.into_iter().try_fold(1usize, |count, axis| {
        count
            .checked_mul(axis as usize)
            .ok_or(ShResidencyDrainError::SlotOverflow)
    })?;
    let mut levels = Vec::with_capacity(brick_count);
    let mut scales = Vec::with_capacity(brick_count);
    for brick_z in 0..affinity_dims[2] {
        for brick_y in 0..affinity_dims[1] {
            for brick_x in 0..affinity_dims[0] {
                let dense = dense_index(
                    [brick_x * 4, brick_y * 4, brick_z * 4],
                    base.grid_dimensions,
                )?;
                let probe = base.probes.get(dense as usize).ok_or(
                    ShResidencyDrainError::MalformedChunk {
                        cluster_id: 0,
                        reason: "id-34 projection has incomplete brick metadata",
                    },
                )?;
                levels.push(Level::from_u8(probe.density_level).ok_or(
                    ShResidencyDrainError::MalformedChunk {
                        cluster_id: 0,
                        reason: "id-34 projection has an invalid density level",
                    },
                )?);
                scales.push(probe.node_scale);
            }
        }
    }
    let validity: Vec<bool> = base
        .probes
        .iter()
        .map(|probe| probe.validity != 0)
        .collect();
    let prefix = stored_node_prefix_sum(base.grid_dimensions, &levels, &scales, &validity).ok_or(
        ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "id-34 projection has an invalid stored-node prefix",
        },
    )?;

    let mut nodes = Vec::with_capacity(base.probes.len());
    let mut local_slots = Vec::with_capacity(base.probes.len());
    let mut layouts = BTreeMap::new();
    for (dense_usize, probe) in base.probes.iter().enumerate() {
        let dense = u32::try_from(dense_usize).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        let node = oracle_stored_node_for_dense(
            dense,
            base.grid_dimensions,
            probe.validity,
            probe.density_level,
            probe.node_scale,
        )?;
        let Some(node) = node else {
            nodes.push(None);
            local_slots.push(None);
            continue;
        };
        let node_index = affinity_index(node.brick_origin, affinity_dims)?;
        let range =
            *prefix
                .bricks
                .get(node_index)
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id: 0,
                    reason: "id-34 stored-node origin is outside the prefix",
                })?;
        if range.stored_tile_count == 0 {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "valid probe resolves to an empty stored node",
            });
        }
        let local_slot = match node.level {
            0 => oracle_l0_local_slot(dense, base.grid_dimensions, &validity)?,
            1 | 2 => 0,
            _ => unreachable!("stored_node_for_dense validated level"),
        };
        if local_slot >= range.stored_tile_count {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "id-34 local stored-node rank exceeds its prefix range",
            });
        }
        layouts.entry(node).or_insert(StoredNodeLayout {
            global_base_slot: range.base_slot,
            tile_count: range.stored_tile_count,
        });
        nodes.push(Some(node));
        local_slots.push(Some(local_slot));
    }
    Ok(OracleLayout {
        nodes,
        local_slots,
        layouts,
    })
}

fn oracle_stored_node_for_dense(
    dense: u32,
    dimensions: [u32; 3],
    validity: u8,
    level: u8,
    scale: u8,
) -> Result<Option<StoredNode>, ShResidencyDrainError> {
    if validity == 0 {
        return Ok(None);
    }
    if level > 2 || scale > 31 || dimensions.contains(&0) {
        return Err(ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "id-34 node metadata is invalid",
        });
    }
    let xy = dimensions[0]
        .checked_mul(dimensions[1])
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    if dense
        >= xy
            .checked_mul(dimensions[2])
            .ok_or(ShResidencyDrainError::SlotOverflow)?
    {
        return Err(ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "dense index exceeds id-34 grid",
        });
    }
    let xyz = [
        dense % dimensions[0],
        (dense / dimensions[0]) % dimensions[1],
        dense / xy,
    ];
    let brick = xyz.map(|coordinate| coordinate / 4);
    let span = 1u32
        .checked_shl(u32::from(scale))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    Ok(Some(StoredNode {
        brick_origin: brick.map(|coordinate| coordinate / span * span),
        scale,
        level,
    }))
}

fn oracle_l0_local_slot(
    dense: u32,
    dimensions: [u32; 3],
    validity: &[bool],
) -> Result<u32, ShResidencyDrainError> {
    let xy = dimensions[0]
        .checked_mul(dimensions[1])
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let xyz = [
        dense % dimensions[0],
        (dense / dimensions[0]) % dimensions[1],
        dense / xy,
    ];
    let brick = xyz.map(|axis| axis / 4);
    let mut count = 0u32;
    for local_z in 0..4 {
        for local_y in 0..4 {
            for local_x in 0..4 {
                let coord = [
                    brick[0] * 4 + local_x,
                    brick[1] * 4 + local_y,
                    brick[2] * 4 + local_z,
                ];
                if coord[0] >= dimensions[0]
                    || coord[1] >= dimensions[1]
                    || coord[2] >= dimensions[2]
                {
                    continue;
                }
                let index = dense_index(coord, dimensions)?;
                if index == dense {
                    return Ok(count);
                }
                if validity[index as usize] {
                    count = count
                        .checked_add(1)
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                }
            }
        }
    }
    Err(ShResidencyDrainError::MalformedChunk {
        cluster_id: 0,
        reason: "valid L0 probe is absent from its brick",
    })
}

pub(in crate::render::sh_streaming) fn base_with_probes(
    dims: [u32; 3],
    probes: Vec<OctahedralShProbe>,
) -> ShStreamBaseMetadata {
    ShStreamBaseMetadata {
        grid_origin: [0.0; 3],
        cell_size: [1.0; 3],
        grid_dimensions: dims,
        probe_stride: 8,
        tile_dimension: 4,
        tile_border: 1,
        atlas_dimensions: [8, 8],
        layer_count: 1,
        tiles_per_layer: 1,
        atlas_tiles_per_row: 1,
        irradiance_format: 0,
        probes,
        animation_descriptors: Vec::new(),
        slot_for_map_light: Vec::new(),
    }
}

/// A well-formed id-34 probe set: every brick has a level, scaled nodes sit on
/// aligned origins that fit the grid, and each scaled node's origin brick
/// holds at least one valid probe. `scaled` allows scale 1..=3 nodes;
/// `invalid_percent` is the share of probes marked invalid.
pub(in crate::render::sh_streaming) fn synthetic_probes(
    rng: &mut Rng,
    dims: [u32; 3],
    scaled: bool,
    invalid_percent: u32,
) -> Vec<OctahedralShProbe> {
    let bricks = dims.map(|axis| axis.div_ceil(4) as usize);
    let brick_count = bricks[0] * bricks[1] * bricks[2];
    let mut level = vec![0u8; brick_count];
    let mut scale = vec![0u8; brick_count];
    let mut assigned = vec![false; brick_count];
    let brick_at = |x: usize, y: usize, z: usize| x + bricks[0] * (y + bricks[1] * z);
    let mut scaled_origins = Vec::new();
    for bz in 0..bricks[2] {
        for by in 0..bricks[1] {
            for bx in 0..bricks[0] {
                if assigned[brick_at(bx, by, bz)] {
                    continue;
                }
                let mut placed = false;
                if scaled && rng.chance(40) {
                    let node_scale = 1 + rng.below(3) as u8;
                    let edge = 1usize << node_scale;
                    let fits = bx % edge == 0
                        && by % edge == 0
                        && bz % edge == 0
                        && (bx + edge) * 4 <= dims[0] as usize
                        && (by + edge) * 4 <= dims[1] as usize
                        && (bz + edge) * 4 <= dims[2] as usize
                        && (bz..bz + edge).all(|z| {
                            (by..by + edge)
                                .all(|y| (bx..bx + edge).all(|x| !assigned[brick_at(x, y, z)]))
                        });
                    if fits {
                        let node_level = 1 + rng.below(2) as u8;
                        for z in bz..bz + edge {
                            for y in by..by + edge {
                                for x in bx..bx + edge {
                                    let index = brick_at(x, y, z);
                                    assigned[index] = true;
                                    level[index] = node_level;
                                    scale[index] = node_scale;
                                }
                            }
                        }
                        scaled_origins.push([bx, by, bz]);
                        placed = true;
                    }
                }
                if !placed {
                    let index = brick_at(bx, by, bz);
                    assigned[index] = true;
                    level[index] = rng.below(3) as u8;
                }
            }
        }
    }
    let [width, height, depth] = dims.map(|axis| axis as usize);
    let mut probes = Vec::with_capacity(width * height * depth);
    for z in 0..depth {
        for y in 0..height {
            for x in 0..width {
                let index = brick_at(x / 4, y / 4, z / 4);
                probes.push(OctahedralShProbe {
                    validity: u8::from(!rng.chance(invalid_percent)),
                    density_level: level[index],
                    node_scale: scale[index],
                    ..Default::default()
                });
            }
        }
    }
    for [bx, by, bz] in scaled_origins {
        probes[bx * 4 + width * (by * 4 + height * bz * 4)].validity = 1;
    }
    probes
}

pub(in crate::render::sh_streaming) const GRID_SHAPES: [[u32; 3]; 9] = [
    [4, 4, 4],
    [8, 8, 8],
    [16, 16, 16],
    [5, 7, 9],
    [11, 6, 13],
    [23, 17, 14],
    [32, 8, 4],
    [1, 1, 1],
    [30, 30, 30],
];

fn assert_layouts_match(base: &ShStreamBaseMetadata, label: &str) -> bool {
    let new = derive_dense_node_layout(base);
    let old = oracle_derive(base);
    match (new, old) {
        (Err(new), Err(old)) => {
            assert_eq!(new, old, "{label}: both reject, with different errors");
            false
        }
        (Ok(new), Ok(old)) => {
            assert_eq!(new.nodes, old.nodes, "{label}: dense nodes");
            assert_eq!(new.local_slots, old.local_slots, "{label}: local slots");
            assert_eq!(
                new.layouts.len(),
                old.layouts.len(),
                "{label}: layout count"
            );
            let mut queried = 0usize;
            for (node, layout) in &old.layouts {
                let found = new
                    .layouts
                    .get(node)
                    .unwrap_or_else(|| panic!("{label}: missing layout for {node:?}"));
                assert_eq!(found.global_base_slot, layout.global_base_slot, "{label}");
                assert_eq!(found.tile_count, layout.tile_count, "{label}");
                // Absent keys: neighbours of a present node must agree on
                // every axis the array indexes by.
                for variant in key_variants(*node) {
                    assert_eq!(
                        new.layouts
                            .get(&variant)
                            .map(|l| (l.global_base_slot, l.tile_count)),
                        old.layouts
                            .get(&variant)
                            .map(|l| (l.global_base_slot, l.tile_count)),
                        "{label}: query {variant:?}"
                    );
                    queried += 1;
                }
            }
            for (node, layout) in new.layouts.iter() {
                let expected = old
                    .layouts
                    .get(&node)
                    .unwrap_or_else(|| panic!("{label}: extra layout {node:?}"));
                assert_eq!(layout.tile_count, expected.tile_count, "{label}");
            }
            assert_eq!(
                new.layouts.values().count(),
                old.layouts.len(),
                "{label}: value iteration"
            );
            let _ = queried;
            true
        }
        (new, old) => panic!(
            "{label}: new {:?} vs oracle {:?}",
            new.map(|_| "Ok"),
            old.map(|_| "Ok")
        ),
    }
}

/// Keys near `node`: other scale and level at the same origin, and the
/// origin stepped one brick along each axis (or out of the grid).
pub(in crate::render::sh_streaming) fn key_variants(node: StoredNode) -> Vec<StoredNode> {
    let mut variants = Vec::new();
    for scale in 0..4u8 {
        for level in 0..3u8 {
            variants.push(StoredNode {
                brick_origin: node.brick_origin,
                scale,
                level,
            });
        }
    }
    for axis in 0..3 {
        for delta in [1u32, u32::MAX] {
            let mut origin = node.brick_origin;
            origin[axis] = origin[axis].wrapping_add(delta);
            variants.push(StoredNode {
                brick_origin: origin,
                ..node
            });
        }
    }
    variants
}

#[test]
fn fixed_fixture_layout_matches_the_oracle() {
    let probes = vec![
        OctahedralShProbe {
            validity: 1,
            ..Default::default()
        };
        64
    ];
    let base = base_with_probes([4, 4, 4], probes);
    assert!(assert_layouts_match(&base, "one L0 brick"));
}

#[test]
fn well_formed_grids_of_every_shape_and_density_match_the_oracle() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut accepted = 0;
    let mut scaled_seen = false;
    let mut levels_seen = [false; 3];
    for round in 0..6 {
        for dims in GRID_SHAPES {
            for (scaled, invalid_percent) in [(false, 0), (false, 40), (true, 0), (true, 60)] {
                let probes = synthetic_probes(&mut rng, dims, scaled, invalid_percent);
                for probe in &probes {
                    scaled_seen |= probe.node_scale > 0;
                    levels_seen[probe.density_level as usize] = true;
                }
                let base = base_with_probes(dims, probes);
                let label =
                    format!("round {round} dims {dims:?} scaled {scaled} inv {invalid_percent}");
                accepted += usize::from(assert_layouts_match(&base, &label));
            }
        }
    }
    assert!(scaled_seen, "generator never produced a scaled node");
    assert_eq!(levels_seen, [true; 3], "generator missed a density level");
    assert!(
        accepted > 100,
        "only {accepted} synthetic manifests were accepted"
    );
}

#[test]
fn corrupted_metadata_is_rejected_or_accepted_exactly_as_the_oracle_does() {
    let mut rng = Rng(0xd1b5_4a32_d192_ed03);
    let (mut accepted, mut rejected) = (0, 0);
    for round in 0..40 {
        for dims in GRID_SHAPES {
            let mut probes = synthetic_probes(&mut rng, dims, true, 25);
            for _ in 0..1 + rng.below(3) {
                let index = rng.below(probes.len() as u32) as usize;
                match rng.below(4) {
                    0 => probes[index].density_level = rng.below(5) as u8,
                    1 => probes[index].node_scale = rng.below(34) as u8,
                    2 => probes[index].validity ^= 1,
                    _ => probes[index].density_level = 0,
                }
            }
            let base = base_with_probes(dims, probes);
            let label = format!("corrupt round {round} dims {dims:?}");
            if assert_layouts_match(&base, &label) {
                accepted += 1;
            } else {
                rejected += 1;
            }
        }
    }
    assert!(
        accepted > 20 && rejected > 20,
        "accepted {accepted}, rejected {rejected}"
    );
}

#[test]
fn probe_count_that_disagrees_with_the_grid_is_rejected_like_the_oracle() {
    let mut rng = Rng(7);
    let dims = [8, 8, 8];
    let mut probes = synthetic_probes(&mut rng, dims, false, 10);
    probes.pop();
    assert!(!assert_layouts_match(
        &base_with_probes(dims, probes.clone()),
        "short"
    ));
    probes.push(OctahedralShProbe::default());
    probes.push(OctahedralShProbe::default());
    assert!(!assert_layouts_match(
        &base_with_probes(dims, probes),
        "long"
    ));
}
