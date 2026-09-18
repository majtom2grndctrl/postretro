//! Shared base/delta reconstruction: base L1 is scale-aware and node-local; delta L1/L2 are brick-local.
//! See: context/lib/rendering_pipeline.md §4 "Adaptive base-probe spacing".

use glam::Vec3;

use crate::delta_sh_volumes::{AFFINITY_FACTOR, PROBES_PER_CELL};

/// Affinity factor as `usize` — a 4×4×4 brick edge (= one affinity cell).
const AF: usize = AFFINITY_FACTOR as usize; // 4

/// Largest power-of-two brick-node scale represented by the v11 base wire.
pub const MAX_NODE_SCALE: u8 = 3;

/// An octahedral interior tile: `interior*interior` RGB texels.
pub type Tile = Vec<Vec3>;

/// A zero-filled tile of `texels` RGB texels.
pub fn zero_tile(texels: usize) -> Tile {
    vec![Vec3::ZERO; texels]
}

/// The 8 corner local indices of a 4×4×4 brick: local ∈ {0,3}³, x-fastest.
pub fn corner_locals() -> [usize; 8] {
    let mut out = [0usize; 8];
    let mut k = 0;
    for &cz in &[0usize, AF - 1] {
        for &cy in &[0usize, AF - 1] {
            for &cx in &[0usize, AF - 1] {
                out[k] = cx + cy * AF + cz * AF * AF;
                k += 1;
            }
        }
    }
    out
}

/// Decompose a local probe index (0..63) into `(x, y, z)` within the brick.
pub fn local_xyz(local: usize) -> (usize, usize, usize) {
    (local % AF, (local / AF) % AF, local / (AF * AF))
}

/// Trilinear weight of corner `(cx,cy,cz)∈{0,3}³` for a target at local
/// `(tx,ty,tz)`, per-axis weight = position along the 0..3 span.
pub fn trilinear_weight(target: (usize, usize, usize), corner: (usize, usize, usize)) -> f32 {
    let axis = |t: usize, c: usize| -> f32 {
        let f = t as f32 / (AF - 1) as f32; // 0..1 along the brick span
        if c == AF - 1 { f } else { 1.0 - f }
    };
    axis(target.0, corner.0) * axis(target.1, corner.1) * axis(target.2, corner.2)
}

/// Probe edge length of an aligned hierarchy node at `scale`.
///
/// Scale zero is one 4-probe affinity brick; every additional scale doubles
/// the brick edge while retaining the same eight-corner L1 stored set.
pub fn node_probe_edge(scale: u8) -> Option<u32> {
    (scale <= MAX_NODE_SCALE).then(|| u32::from(AFFINITY_FACTOR) << scale)
}

/// Probe-local coordinate inside the aligned node containing `probe`.
pub fn node_local_coord(probe: [u32; 3], scale: u8) -> Option<[u32; 3]> {
    let edge = node_probe_edge(scale)?;
    Some(probe.map(|axis| axis % edge))
}

/// Absolute probe coordinate of one L1 node corner in x-fastest corner order.
pub fn node_corner_coord(probe_origin: [u32; 3], scale: u8, corner: u8) -> Option<[u32; 3]> {
    if corner >= 8 {
        return None;
    }
    let span = node_probe_edge(scale)? - 1;
    Some([
        probe_origin[0] + if corner & 1 == 0 { 0 } else { span },
        probe_origin[1] + if corner & 2 == 0 { 0 } else { span },
        probe_origin[2] + if corner & 4 == 0 { 0 } else { span },
    ])
}

/// Scale-aware L1 trilinear weight for one node-local target and stored corner.
///
/// This is the hierarchy reconstruction definition mirrored by
/// `sh_sample.wgsl`: the fraction spans the complete `4 * 2^scale` probe edge,
/// not the target probe's containing brick.
pub fn node_l1_corner_weight(local: [u32; 3], corner: u8, scale: u8) -> Option<f32> {
    if corner >= 8 {
        return None;
    }
    let edge = node_probe_edge(scale)?;
    if local.into_iter().any(|axis| axis >= edge) {
        return None;
    }
    let span = (edge - 1) as f32;
    let fraction = local.map(|axis| axis as f32 / span);
    Some(
        (if corner & 1 == 0 {
            1.0 - fraction[0]
        } else {
            fraction[0]
        }) * (if corner & 2 == 0 {
            1.0 - fraction[1]
        } else {
            fraction[1]
        }) * (if corner & 4 == 0 {
            1.0 - fraction[2]
        } else {
            fraction[2]
        }),
    )
}

/// L1 reconstruction of the tile at `target_local` from the brick's valid corner
/// tiles. Corners that are absent/invalid are dropped and the surviving weights
/// renormalized. Returns `None` when no valid corner exists.
pub fn reconstruct_l1_tile(
    tiles: &[Option<Tile>; PROBES_PER_CELL],
    target_local: usize,
    texels: usize,
) -> Option<Tile> {
    let target = local_xyz(target_local);
    let mut acc = zero_tile(texels);
    let mut wsum = 0.0f32;
    for (corner, corner_local) in corner_locals().into_iter().enumerate() {
        if let Some(tile) = &tiles[corner_local] {
            let w = node_l1_corner_weight(
                [target.0 as u32, target.1 as u32, target.2 as u32],
                corner as u8,
                0,
            )
            .expect("brick-local L1 coordinates use scale zero");
            if w <= 0.0 {
                continue;
            }
            for (a, t) in acc.iter_mut().zip(tile.iter()) {
                *a += *t * w;
            }
            wsum += w;
        }
    }
    if wsum <= 0.0 {
        return None;
    }
    for a in acc.iter_mut() {
        *a /= wsum;
    }
    Some(acc)
}

/// L2 brick-mean tile over valid probes. `None` when the brick has no valid
/// probe.
pub fn reconstruct_l2_tile(tiles: &[Option<Tile>; PROBES_PER_CELL], texels: usize) -> Option<Tile> {
    let mut acc = zero_tile(texels);
    let mut n = 0u32;
    for tile in tiles.iter().flatten() {
        for (a, t) in acc.iter_mut().zip(tile.iter()) {
            *a += *t;
        }
        n += 1;
    }
    if n == 0 {
        return None;
    }
    for a in acc.iter_mut() {
        *a /= n as f32;
    }
    Some(acc)
}

/// Per-brick, per-section probe-density level. The `#[repr(u8)]` discriminants
/// (0/1/2) are the on-wire encoding — see [`Level::to_u8`] / [`Level::from_u8`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Level {
    L0 = 0,
    L1 = 1,
    L2 = 2,
}

impl Level {
    /// Wire encoding of the level (0 = L0, 1 = L1, 2 = L2).
    pub fn to_u8(self) -> u8 {
        self as u8
    }

    /// Decode a wire level byte; `None` for any value outside `0..=2`.
    pub fn from_u8(v: u8) -> Option<Level> {
        match v {
            0 => Some(Level::L0),
            1 => Some(Level::L1),
            2 => Some(Level::L2),
            _ => None,
        }
    }
}

/// The kept-probe mask for `valid_probe_mask` at `level`: the subset of valid
/// probes whose delta tiles are stored (and thus indexed by kept rank). This is
/// the single source of truth for which probes a coarsened brick keeps — used by
/// the wire payload-length identity, the emit path, the CPU compose reference,
/// and (via the WGSL port) the GPU compose passes.
/// - **L0** — every valid probe (kept == valid).
/// - **L1** — the valid corner probes (`{0,AF-1}³`).
/// - **L2** — a single representative slot, the lowest-set valid bit, carrying
///   the synthesized brick-mean tile (a computed write, not a copied probe).
///
/// The result is always a subset of `valid_probe_mask`.
pub fn kept_mask(level: Level, valid_probe_mask: u64) -> u64 {
    match level {
        Level::L0 => valid_probe_mask,
        Level::L1 => {
            let mut kept = 0u64;
            for local in corner_locals() {
                kept |= valid_probe_mask & (1u64 << local);
            }
            kept
        }
        Level::L2 => {
            if valid_probe_mask == 0 {
                0
            } else {
                1u64 << valid_probe_mask.trailing_zeros()
            }
        }
    }
}

/// One stored tile in the base SH volume's on-disk stored set.
///
/// This deliberately differs from [`kept_mask`], which describes the delta
/// sections' valid-only kept-rank payload. Base L1 needs arithmetic corner
/// slots at sample time, so it reserves all eight corners and writes an invalid
/// corner as a zero tile. L2's slot is a synthesized mean, not a copied probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoredTile {
    /// A probe tile at this x-fastest local index. L1 includes invalid corners.
    Probe(usize),
    /// The one L2 brick mean over the brick's valid probes.
    BrickMean,
}

/// The base SH stored set for one 4×4×4 brick, in payload order.
///
/// - L0 stores valid probes in x-fastest local order.
/// - L1 stores all eight [`corner_locals`] in that order, including invalid
///   corners as zero tiles.
/// - L2 stores one synthesized mean tile.
/// - A brick with no valid probes stores no tile at any level.
pub fn stored_tile_set(level: Level, valid_probe_mask: u64) -> Vec<StoredTile> {
    if valid_probe_mask == 0 {
        return Vec::new();
    }

    match level {
        Level::L0 => (0..PROBES_PER_CELL)
            .filter(|&local| valid_probe_mask & (1u64 << local) != 0)
            .map(StoredTile::Probe)
            .collect(),
        Level::L1 => corner_locals().into_iter().map(StoredTile::Probe).collect(),
        Level::L2 => vec![StoredTile::BrickMean],
    }
}

/// Stored tile count for one base-volume brick under the v10 stored-set
/// contract. This is intentionally not the delta kept-rank count: L1 reserves
/// all eight corner slots whenever the brick has any valid probe.
pub fn stored_tiles(level: Level, valid_mask: &[bool; PROBES_PER_CELL]) -> usize {
    let valid_probe_mask =
        valid_mask.iter().enumerate().fold(
            0u64,
            |mask, (local, &valid)| {
                if valid { mask | (1u64 << local) } else { mask }
            },
        );
    stored_tile_set(level, valid_probe_mask).len()
}

/// One brick's range in the stored-tile atlas. `base_slot` is the first slot
/// owned by this brick; slots are allocated brick-major in x-fastest affinity
/// order. An all-invalid brick has a zero count and its base equals the running
/// prefix at that point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredBrickRange {
    pub base_slot: u32,
    pub stored_tile_count: u32,
}

/// Prefix-sum layout of every affinity brick in a grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredBrickPrefixSum {
    pub affinity_dimensions: [u32; 3],
    pub bricks: Vec<StoredBrickRange>,
    pub total_stored_tiles: u32,
}

/// Whole-grid stored-tile prefix sum for the base-volume v10 contract.
///
/// `brick_levels` is one level per affinity brick in x-fastest order;
/// `probe_validity` is one boolean per base probe in x-fastest grid order. The
/// returned ranges provide every brick's stored count and first slot, and the
/// total is `N_s` for `irradiance_atlas_array_layout([N_s, 1, 1], ...)`.
/// `None` reports a malformed input shape or a count that cannot fit the
/// on-wire `u32` slot space.
pub fn stored_brick_prefix_sum(
    grid_dimensions: [u32; 3],
    brick_levels: &[Level],
    probe_validity: &[bool],
) -> Option<StoredBrickPrefixSum> {
    stored_node_prefix_sum(
        grid_dimensions,
        brick_levels,
        &vec![0; brick_levels.len()],
        probe_validity,
    )
}

/// Whole-grid stored-tile prefix sum for the node-aware v11 base contract.
///
/// Every brick carries its containing node's level and scale. A node is an
/// aligned cube of `2^scale` bricks. Its stored set is charged to the aligned
/// origin brick; all other member bricks receive a zero-length range. A scaled
/// node therefore requires at least one valid probe in that origin brick so
/// the elected compose workgroup can derive its shared word. Scale 0 is exactly
/// [`stored_brick_prefix_sum`]'s v10 layout.
pub fn stored_node_prefix_sum(
    grid_dimensions: [u32; 3],
    brick_levels: &[Level],
    brick_scales: &[u8],
    probe_validity: &[bool],
) -> Option<StoredBrickPrefixSum> {
    let probe_count = grid_dimensions
        .iter()
        .try_fold(1usize, |count, &dimension| {
            count.checked_mul(dimension as usize)
        })?;
    if probe_validity.len() != probe_count {
        return None;
    }

    let affinity_dimensions =
        grid_dimensions.map(|dimension| dimension.div_ceil(u32::from(AFFINITY_FACTOR)));
    let brick_count = affinity_dimensions
        .iter()
        .try_fold(1usize, |count, &dimension| {
            count.checked_mul(dimension as usize)
        })?;
    if brick_levels.len() != brick_count || brick_scales.len() != brick_count {
        return None;
    }

    let mut total_stored_tiles = 0u32;
    let mut bricks = Vec::with_capacity(brick_count);
    for brick_z in 0..affinity_dimensions[2] as usize {
        for brick_y in 0..affinity_dimensions[1] as usize {
            for brick_x in 0..affinity_dimensions[0] as usize {
                let brick_index = brick_x
                    + brick_y * affinity_dimensions[0] as usize
                    + brick_z * affinity_dimensions[0] as usize * affinity_dimensions[1] as usize;
                let scale = brick_scales[brick_index];
                if scale > MAX_NODE_SCALE || (scale > 0 && brick_levels[brick_index] == Level::L0) {
                    return None;
                }
                let node_edge = 1usize << scale;
                let node_origin = [
                    brick_x / node_edge * node_edge,
                    brick_y / node_edge * node_edge,
                    brick_z / node_edge * node_edge,
                ];
                if node_origin[0] + node_edge > affinity_dimensions[0] as usize
                    || node_origin[1] + node_edge > affinity_dimensions[1] as usize
                    || node_origin[2] + node_edge > affinity_dimensions[2] as usize
                    || (scale > 0
                        && ((node_origin[0] + node_edge) * AF > grid_dimensions[0] as usize
                            || (node_origin[1] + node_edge) * AF > grid_dimensions[1] as usize
                            || (node_origin[2] + node_edge) * AF > grid_dimensions[2] as usize))
                {
                    return None;
                }
                for member_z in node_origin[2]..node_origin[2] + node_edge {
                    for member_y in node_origin[1]..node_origin[1] + node_edge {
                        for member_x in node_origin[0]..node_origin[0] + node_edge {
                            let member = member_x
                                + member_y * affinity_dimensions[0] as usize
                                + member_z
                                    * affinity_dimensions[0] as usize
                                    * affinity_dimensions[1] as usize;
                            if brick_levels[member] != brick_levels[brick_index]
                                || brick_scales[member] != scale
                            {
                                return None;
                            }
                        }
                    }
                }

                let is_node_origin = [brick_x, brick_y, brick_z] == node_origin;
                let mut valid_probe_mask = 0u64;
                for local_z in 0..AF {
                    for local_y in 0..AF {
                        for local_x in 0..AF {
                            let probe_x = brick_x * AF + local_x;
                            let probe_y = brick_y * AF + local_y;
                            let probe_z = brick_z * AF + local_z;
                            if probe_x >= grid_dimensions[0] as usize
                                || probe_y >= grid_dimensions[1] as usize
                                || probe_z >= grid_dimensions[2] as usize
                            {
                                continue;
                            }
                            let probe_index = probe_x
                                + probe_y * grid_dimensions[0] as usize
                                + probe_z
                                    * grid_dimensions[0] as usize
                                    * grid_dimensions[1] as usize;
                            if probe_validity[probe_index] {
                                let local = local_x + local_y * AF + local_z * AF * AF;
                                valid_probe_mask |= 1u64 << local;
                            }
                        }
                    }
                }

                let stored_tile_count = if scale == 0 {
                    u32::try_from(
                        stored_tile_set(brick_levels[brick_index], valid_probe_mask).len(),
                    )
                    .ok()?
                } else if !is_node_origin {
                    0
                } else {
                    // Every compose pass elects this brick's workgroup as the
                    // sole scaled-node writer and derives node metadata from a
                    // valid word in the brick. Reject a node whose validity
                    // exists only in later member bricks.
                    if valid_probe_mask == 0 {
                        return None;
                    }
                    if brick_levels[brick_index] == Level::L1 {
                        8
                    } else {
                        1
                    }
                };
                bricks.push(StoredBrickRange {
                    base_slot: total_stored_tiles,
                    stored_tile_count,
                });
                total_stored_tiles = total_stored_tiles.checked_add(stored_tile_count)?;
            }
        }
    }

    Some(StoredBrickPrefixSum {
        affinity_dimensions,
        bricks,
        total_stored_tiles,
    })
}

/// Stored tile count for a delta entry at a candidate level = the popcount of
/// its [`kept_mask`]. Unlike the base model, this reads the delta section's
/// self-describing compact probe set.
pub fn stored_delta_tiles(level: Level, valid_probe_mask: u64) -> usize {
    kept_mask(level, valid_probe_mask).count_ones() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn const_tiles(value: f32, texels: usize) -> [Option<Tile>; PROBES_PER_CELL] {
        std::array::from_fn(|_| Some(vec![Vec3::splat(value); texels]))
    }

    #[test]
    fn corner_locals_are_the_eight_brick_corners() {
        // {0,3}^3, x-fastest: 0, 3, 12, 15, 48, 51, 60, 63.
        assert_eq!(corner_locals(), [0, 3, 12, 15, 48, 51, 60, 63]);
    }

    #[test]
    fn local_xyz_round_trips_x_fastest() {
        for lz in 0..AF {
            for ly in 0..AF {
                for lx in 0..AF {
                    let local = lx + ly * AF + lz * AF * AF;
                    assert_eq!(local_xyz(local), (lx, ly, lz));
                }
            }
        }
    }

    #[test]
    fn trilinear_weights_partition_unity_and_hit_endpoints() {
        // Corner (0,0,0) at target (0,0,0) → weight 1.
        assert!((trilinear_weight((0, 0, 0), (0, 0, 0)) - 1.0).abs() < 1e-6);
        // Corner (3,3,3) at target (3,3,3) → weight 1.
        assert!((trilinear_weight((3, 3, 3), (3, 3, 3)) - 1.0).abs() < 1e-6);
        // Corner (3,0,0) at target (0,0,0) → weight 0.
        assert!(trilinear_weight((0, 0, 0), (3, 0, 0)).abs() < 1e-6);
        // Weights across the 8 corners partition unity at any interior target.
        for target_local in 0..PROBES_PER_CELL {
            let target = local_xyz(target_local);
            let mut sum = 0.0;
            for c in corner_locals() {
                sum += trilinear_weight(target, local_xyz(c));
            }
            assert!(
                (sum - 1.0).abs() < 1e-5,
                "trilinear weights must partition unity at local {target_local}"
            );
        }
    }

    #[test]
    fn node_l1_weights_and_corners_cover_every_wire_scale() {
        for scale in 0..=MAX_NODE_SCALE {
            let edge = node_probe_edge(scale).unwrap();
            let origin = [17, 29, 41];
            assert_eq!(node_corner_coord(origin, scale, 0), Some(origin));
            assert_eq!(
                node_corner_coord(origin, scale, 7),
                Some(origin.map(|axis| axis + edge - 1)),
            );
            assert_eq!(
                node_local_coord([edge + 1, edge * 2 + 2, edge * 3 + 3], scale),
                Some([1, 2, 3]),
            );

            for z in 0..edge {
                for y in 0..edge {
                    for x in 0..edge {
                        let local = [x, y, z];
                        let sum: f32 = (0..8)
                            .map(|corner| node_l1_corner_weight(local, corner, scale).unwrap())
                            .sum();
                        assert!(
                            (sum - 1.0).abs() < 1.0e-5,
                            "scale {scale} local {local:?} weights summed to {sum}",
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn node_geometry_rejects_out_of_contract_inputs() {
        assert_eq!(node_probe_edge(MAX_NODE_SCALE + 1), None);
        assert_eq!(node_corner_coord([0; 3], 0, 8), None);
        assert_eq!(node_l1_corner_weight([0; 3], 8, 0), None);
        assert_eq!(node_l1_corner_weight([4, 0, 0], 0, 0), None);
    }

    #[test]
    fn l1_trilinear_reproduces_linear_ramp_exactly() {
        // A brick whose per-probe value is linear in local x: trilinear from the
        // 8 corners must reproduce it exactly at every interior probe.
        let texels = 4;
        let mut tiles: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        for local in 0..PROBES_PER_CELL {
            let (lx, _ly, _lz) = local_xyz(local);
            let val = 10.0 + lx as f32 * 2.0;
            tiles[local] = Some(vec![Vec3::splat(val); texels]);
        }
        for target in 0..PROBES_PER_CELL {
            let recon = reconstruct_l1_tile(&tiles, target, texels).unwrap();
            let (lx, _, _) = local_xyz(target);
            let expect = 10.0 + lx as f32 * 2.0;
            assert!(
                (recon[0].x - expect).abs() < 1e-4,
                "L1 trilinear must reproduce a linear ramp: got {} expect {expect}",
                recon[0].x
            );
        }
    }

    #[test]
    fn l1_renormalizes_over_surviving_corners() {
        // Only corner (0,0,0) valid. A target with a positive weight to it
        // (here local (1,1,1), raw weight (2/3)^3) renormalizes to reproduce the
        // survivor's value exactly. A target whose only surviving corner has
        // weight 0 (the opposite corner (3,3,3)) has no basis → `None`.
        let texels = 2;
        let mut tiles: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        tiles[0] = Some(vec![Vec3::splat(7.0); texels]);

        let interior = 1 + 1 * AF + 1 * AF * AF; // local (1,1,1)
        let recon = reconstruct_l1_tile(&tiles, interior, texels).unwrap();
        for v in &recon {
            assert!((v.x - 7.0).abs() < 1e-6);
        }

        let opposite = (AF - 1) + (AF - 1) * AF + (AF - 1) * AF * AF; // local (3,3,3)
        assert!(reconstruct_l1_tile(&tiles, opposite, texels).is_none());
    }

    #[test]
    fn l1_is_none_without_a_valid_corner() {
        let texels = 2;
        let mut tiles: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        // A valid non-corner probe (local 1) must not rescue L1.
        tiles[1] = Some(vec![Vec3::splat(1.0); texels]);
        assert!(reconstruct_l1_tile(&tiles, 0, texels).is_none());
    }

    #[test]
    fn l2_mean_of_constant_brick_is_exact() {
        let texels = 16;
        let tiles = const_tiles(3.0, texels);
        let recon = reconstruct_l2_tile(&tiles, texels).unwrap();
        for v in &recon {
            assert!((v.x - 3.0).abs() < 1e-6);
        }
    }

    #[test]
    fn l2_is_none_for_empty_brick() {
        let texels = 4;
        let tiles: [Option<Tile>; PROBES_PER_CELL] = std::array::from_fn(|_| None);
        assert!(reconstruct_l2_tile(&tiles, texels).is_none());
    }

    #[test]
    fn base_stored_tile_set_reserves_invalid_l1_corners() {
        let mut valid = [true; PROBES_PER_CELL];
        assert_eq!(stored_tiles(Level::L0, &valid), 64);
        assert_eq!(stored_tiles(Level::L1, &valid), 8);
        assert_eq!(stored_tiles(Level::L2, &valid), 1);

        // Base L1 stores an arithmetic corner slot even when that probe is
        // invalid; the producer writes that slot as a zero tile.
        valid[0] = false;
        assert_eq!(stored_tiles(Level::L0, &valid), 63);
        assert_eq!(stored_tiles(Level::L1, &valid), 8);
        assert_eq!(stored_tiles(Level::L2, &valid), 1);

        let empty = [false; PROBES_PER_CELL];
        assert_eq!(stored_tiles(Level::L0, &empty), 0);
        assert_eq!(stored_tiles(Level::L1, &empty), 0);
        assert_eq!(stored_tiles(Level::L2, &empty), 0);

        let mask = !(1u64 << 0);
        assert_eq!(
            stored_tile_set(Level::L1, mask),
            corner_locals().map(StoredTile::Probe).to_vec()
        );
        assert_eq!(
            stored_tile_set(Level::L2, mask),
            vec![StoredTile::BrickMean]
        );
    }

    #[test]
    fn stored_brick_prefix_sum_is_brick_major_and_x_fastest() {
        // Two complete bricks along X. The first L0 brick has two valid probes;
        // the second L1 brick has one valid non-corner probe but still reserves
        // all eight arithmetic corner slots.
        let grid = [8, 4, 4];
        let mut valid = vec![false; 8 * 4 * 4];
        valid[0] = true;
        valid[1] = true;
        valid[5] = true;
        let prefix = stored_brick_prefix_sum(grid, &[Level::L0, Level::L1], &valid).unwrap();

        assert_eq!(prefix.affinity_dimensions, [2, 1, 1]);
        assert_eq!(
            prefix.bricks,
            vec![
                StoredBrickRange {
                    base_slot: 0,
                    stored_tile_count: 2,
                },
                StoredBrickRange {
                    base_slot: 2,
                    stored_tile_count: 8,
                },
            ]
        );
        assert_eq!(prefix.total_stored_tiles, 10);
    }

    #[test]
    fn stored_brick_prefix_sum_rejects_mismatched_input_shapes() {
        assert!(stored_brick_prefix_sum([4, 4, 4], &[], &[true; 64]).is_none());
        assert!(stored_brick_prefix_sum([4, 4, 4], &[Level::L0], &[]).is_none());
    }

    #[test]
    fn stored_node_prefix_sum_charges_only_the_aligned_origin() {
        let grid = [8, 8, 8];
        let levels = vec![Level::L1; 8];
        let scales = vec![1; 8];
        let valid = vec![true; 8 * 8 * 8];
        let prefix = stored_node_prefix_sum(grid, &levels, &scales, &valid).unwrap();
        assert_eq!(prefix.total_stored_tiles, 8);
        assert_eq!(prefix.bricks[0].stored_tile_count, 8);
        assert!(
            prefix.bricks[1..]
                .iter()
                .all(|range| range.stored_tile_count == 0 && range.base_slot == 8)
        );

        let l2 = stored_node_prefix_sum(grid, &vec![Level::L2; 8], &scales, &valid).unwrap();
        assert_eq!(l2.total_stored_tiles, 1);
        assert_eq!(l2.bricks[0].stored_tile_count, 1);
    }

    #[test]
    fn stored_node_prefix_sum_rejects_disagreement_partial_nodes_and_scaled_l0() {
        let valid = vec![true; 8 * 8 * 8];
        let mut levels = vec![Level::L1; 8];
        let mut scales = vec![1; 8];
        levels[7] = Level::L2;
        assert!(stored_node_prefix_sum([8, 8, 8], &levels, &scales, &valid).is_none());

        levels.fill(Level::L1);
        scales[7] = 0;
        assert!(stored_node_prefix_sum([8, 8, 8], &levels, &scales, &valid).is_none());
        assert!(
            stored_node_prefix_sum([7, 8, 8], &levels, &vec![1; 8], &vec![true; 7 * 8 * 8],)
                .is_none()
        );
        assert!(
            stored_node_prefix_sum([8, 8, 8], &vec![Level::L0; 8], &vec![1; 8], &valid,).is_none()
        );
    }

    // Regression: a scaled node with validity only outside its origin brick
    // allocated stored slots that no elected compose workgroup could write.
    #[test]
    fn stored_node_prefix_sum_rejects_empty_scaled_origin_brick() {
        let grid = [8, 8, 8];
        let scales = vec![1; 8];
        let mut valid = vec![false; 8 * 8 * 8];
        valid[7] = true;

        for level in [Level::L1, Level::L2] {
            assert!(
                stored_node_prefix_sum(grid, &vec![level; 8], &scales, &valid).is_none(),
                "{level:?} must retain a valid word in the elected origin brick"
            );
        }
    }

    #[test]
    fn stored_delta_tile_counts_track_popcount() {
        let full = u64::MAX;
        assert_eq!(stored_delta_tiles(Level::L0, full), 64);
        assert_eq!(stored_delta_tiles(Level::L1, full), 8);
        assert_eq!(stored_delta_tiles(Level::L2, full), 1);

        // Only non-corner bits set → L1 keeps nothing, L2 keeps its one tile.
        let non_corner = 1u64 << 1;
        assert_eq!(stored_delta_tiles(Level::L0, non_corner), 1);
        assert_eq!(stored_delta_tiles(Level::L1, non_corner), 0);
        assert_eq!(stored_delta_tiles(Level::L2, non_corner), 1);

        assert_eq!(stored_delta_tiles(Level::L2, 0), 0);
    }

    #[test]
    fn level_wire_bytes_round_trip_and_reject_out_of_range() {
        for (lvl, byte) in [(Level::L0, 0u8), (Level::L1, 1), (Level::L2, 2)] {
            assert_eq!(lvl.to_u8(), byte);
            assert_eq!(Level::from_u8(byte), Some(lvl));
        }
        assert_eq!(Level::from_u8(3), None);
        assert_eq!(Level::from_u8(255), None);
    }

    #[test]
    fn kept_mask_is_a_subset_of_validity_at_every_level() {
        for valid in [0u64, 1, u64::MAX, 0x00FF_00FF_00FF_00FF, 1u64 << 63, 0b1010] {
            for lvl in [Level::L0, Level::L1, Level::L2] {
                let kept = kept_mask(lvl, valid);
                assert_eq!(kept & !valid, 0, "kept must be a subset of validity");
                assert_eq!(
                    kept.count_ones() as usize,
                    stored_delta_tiles(lvl, valid),
                    "stored_delta_tiles must equal popcount(kept_mask)"
                );
            }
        }
    }

    #[test]
    fn kept_mask_selects_the_expected_lattice() {
        let full = u64::MAX;
        // L0 keeps everything.
        assert_eq!(kept_mask(Level::L0, full), full);
        // L1 keeps exactly the 8 corner bits.
        let corner_bits: u64 = corner_locals().iter().map(|&l| 1u64 << l).sum();
        assert_eq!(kept_mask(Level::L1, full), corner_bits);
        // L2 keeps a single bit: the lowest-set valid bit.
        assert_eq!(kept_mask(Level::L2, full), 1u64 << 0);
        assert_eq!(kept_mask(Level::L2, 0b1100), 1u64 << 2);
        assert_eq!(kept_mask(Level::L2, 0), 0);
        // L1 over a mask with no valid corner keeps nothing.
        assert_eq!(kept_mask(Level::L1, 1u64 << 1), 0);
    }
}
