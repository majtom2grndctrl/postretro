// What the mover probe channel holds beside the SH base, priced before any
// ray: the inputs the SH working-set gate folds into its phases.
// See: context/lib/build_pipeline.md §Compiler pipeline (working-set gate)

use std::collections::HashSet;
use std::sync::Arc;

use postretro_level_format::mover_probe_channel::{MOVER_IRRADIANCE_TILE_F16_COUNT, PoseMark};
use postretro_level_format::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;
use postretro_level_format::sh_volume::OctahedralAtlasTexel;
use postretro_level_format::texture_names::TextureNamesSection;

use super::bake::MoverBounceJob;
use super::direct::{DirectEntry, LightEntries, ProbeCells, light_movers};
use super::gather::{DomainProbe, PosedProbe};
use super::posed_scene::PosedScene;
use super::posed_solid::{PosedMovers, PosedSolid};
use super::states::{BounceProbe, ChangedProbe, DepthProbe, MidState};
use crate::affinity_grid::AFFINITY_FACTOR;
use crate::map_data::{LightType, MapKinematicMover, MapLight};
use crate::mover_light_gate::Pose;
use crate::sh_bake::{ProbeGridLayout, vec3_from};
use crate::surface_radiance::PatchSet;

/// One gathered tile, border included, as the posed gather holds it.
const GATHERED_TILE_BYTES: u64 = 36 * size_of::<OctahedralAtlasTexel>() as u64;
/// One closed-pose depth tile.
const DEPTH_TILE_BYTES: u64 = (PROBE_DEPTH_TILE_TEXELS * size_of::<[u16; 2]>()) as u64;
/// One stored bounce state, id 27's compact RGB tile.
const STATE_TILE_BYTES: u64 = (MOVER_IRRADIANCE_TILE_F16_COUNT * size_of::<u16>()) as u64;
const MARK_BYTES: u64 = size_of::<PoseMark>() as u64;
/// One id-59 tile reference in the mover-absent depth's B-tree: its key,
/// its slice and the node's share around them.
const ABSENT_TILE_ENTRY_BYTES: u64 = 48;
/// One probe in the tile-range union's B-tree, with the node's share.
const UNION_ENTRY_BYTES: u64 = 16;

/// What the mover probe channel adds to the SH working set, from the map's
/// movers, its posed scenes and its probe grid alone. Every term is priced
/// at the mover's own `light_bounces`, never the build's fidelity, so a
/// `low` build is priced as a `high` one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MoverChannelCost {
    /// Movers with a bounce job, and those of them re-baking radiance
    /// (`light_bounces` above 0).
    pub(crate) movers: u64,
    pub(crate) radiance_movers: u64,
    /// Every posed scene's heap, held from the gate to the end of the SH
    /// base stage.
    pub(crate) scene_bytes: u64,
    /// The posed solids' share of the scenes: resolved once for the
    /// compile, they stay held through the direct half.
    pub(crate) solid_bytes: u64,
    /// The most patches any one pose's sides carry.
    pub(crate) max_mover_patches: u64,
    /// Id 34's later bounces the bake may keep apart: every one, when a
    /// mover re-baking radiance may read them at either fidelity.
    pub(crate) kept_later_bounces: u64,
    pub(crate) valid_probes: u64,
    pub(crate) total_probes: u64,
    /// The tile range mover depth may fill: no mover keeps more than one
    /// depth record past it.
    pub(crate) depth_cap: u64,
    /// The direct half's entries at their bound.
    pub(crate) direct_entry_bytes: u64,
    /// The most any one mover's reduced states hold at their bound.
    pub(crate) max_mover_states_bytes: u64,
    /// The most any one mover's mid poses hold while it bakes: every mid
    /// pose's states and one mid pose's gather.
    pub(crate) max_mid_pose_bytes: u64,
    /// Whether some mover re-baking radiance has a mid pose: its posed
    /// later bounces then gather the domain's static patches apart before
    /// they scatter.
    pub(crate) near_field_later: bool,
    /// Whether a pose may re-bake a later bounce: a mover re-baking
    /// radiance at a fill above 0. Its buffers are allocated at either
    /// fidelity.
    pub(crate) posed_later: bool,
    /// `--mover-payload-max-size`: the most the bounce and depth blocks
    /// hold.
    pub(crate) payload_cap: u64,
    /// Id 34's affinity cells: one valid-probe mask each in the finish
    /// loop's cells.
    pub(crate) affinity_cells: u64,
    /// The finished movers' names the loop keeps for the cap's error: each
    /// label and its slot, the list's growth slack included.
    pub(crate) name_bytes: u64,
}

impl MoverChannelCost {
    /// The channel's cost for `jobs` over `layout` at `fill`, with mover
    /// depth in a tile range of `depth_cap`, the blocks capped at
    /// `payload_cap` and the direct half's bound over `direct_lights`
    /// (every static and animated baked light).
    pub(crate) fn new(
        jobs: &[MoverBounceJob],
        layout: &ProbeGridLayout,
        fill: f32,
        depth_cap: u32,
        payload_cap: u64,
        direct_lights: &[&MapLight],
        movers: &PosedMovers,
    ) -> Self {
        if jobs.is_empty() {
            return Self::default();
        }
        let scenes = || jobs.iter().flat_map(|job| &job.scenes);
        let valid_probes = layout.validity.iter().filter(|&&valid| valid != 0).count() as u64;
        let mids = |job: &MoverBounceJob| job.scenes.len().saturating_sub(2) as u64;
        let domain = |job: &MoverBounceJob| job.domain_probes.len() as u64;
        let radiance = || jobs.iter().filter(|job| job.high_bounces > 0);
        let records = valid_probes.min(u64::from(depth_cap) + 1);
        Self {
            movers: jobs.len() as u64,
            radiance_movers: radiance().count() as u64,
            scene_bytes: scenes().map(super::PosedScene::heap_bytes).sum(),
            solid_bytes: movers.heap_bytes(),
            max_mover_patches: scenes()
                .map(|scene| scene.patches.len() as u64)
                .max()
                .unwrap_or(0),
            kept_later_bounces: if fill != 0.0 && radiance().next().is_some() {
                u64::from(crate::surface_radiance::LATER_BOUNCE_COUNT)
            } else {
                0
            },
            valid_probes,
            total_probes: layout.total_probes() as u64,
            depth_cap: u64::from(depth_cap),
            direct_entry_bytes: direct_entry_bound(direct_lights, movers, layout),
            max_mover_states_bytes: jobs
                .iter()
                .map(|job| {
                    mover_states_bytes(
                        valid_probes,
                        records,
                        job.scenes.len() as u64,
                        domain(job),
                        job.high_bounces > 0,
                    )
                })
                .max()
                .unwrap_or(0),
            max_mid_pose_bytes: jobs
                .iter()
                .map(|job| mid_pose_bytes(mids(job), domain(job)))
                .max()
                .unwrap_or(0),
            near_field_later: fill != 0.0 && radiance().any(|job| mids(job) > 0),
            posed_later: fill != 0.0 && radiance().next().is_some(),
            payload_cap,
            affinity_cells: layout
                .dims
                .iter()
                .map(|&dim| u64::from(dim.div_ceil(AFFINITY_FACTOR)))
                .product(),
            name_bytes: jobs
                .iter()
                .map(|job| job.label.len() as u64 + 2 * size_of::<String>() as u64)
                .sum(),
        }
    }

    /// One mover's reduced states at their bound ([`mover_states_bytes`]).
    /// Movers finish one at a time, each dropping its states before the
    /// next bakes, so the largest one mover's bounds them all.
    pub(crate) fn states_bytes(&self) -> u64 {
        self.max_mover_states_bytes
    }

    /// One mover's raw pose states, read back from the memo or baked,
    /// before they reduce: both termini at every valid probe and its mid
    /// poses' states over its domain.
    pub(crate) fn raw_mover_bytes(&self) -> u64 {
        if self.movers == 0 {
            return 0;
        }
        2 * self.terminus_bytes() + self.max_mid_pose_bytes
    }

    /// One terminus's raw states: a changed probe per valid probe with its
    /// gathered and depth tiles.
    fn terminus_bytes(&self) -> u64 {
        self.valid_probes
            * (GATHERED_TILE_BYTES
                + DEPTH_TILE_BYTES
                + size_of::<ChangedProbe>() as u64
                + size_of::<(u32, Vec<[u16; 2]>)>() as u64)
    }

    /// What the mover bake holds from its first slot to its last, beside
    /// every pose's bake and every mover's finish: the bake's mover-absent
    /// depth and the skip bars' own (each a validity byte per probe and an
    /// id-59 tile reference per tiled probe), the bars' compact slots, id
    /// 34's affinity cells, the valid probes, the tile-range union (the
    /// joined movers' probes, at most one past the range, plus the joining
    /// mover's records, at most one past it too: twice that many entries),
    /// and the finished movers' names.
    pub(crate) fn loop_bytes(&self) -> u64 {
        if self.movers == 0 {
            return 0;
        }
        let tiles = self.valid_probes.min(self.depth_cap);
        let absent_depth = self.total_probes + tiles * ABSENT_TILE_ENTRY_BYTES;
        let slots = self.total_probes * size_of::<u32>() as u64;
        let cells = self.affinity_cells * size_of::<u64>() as u64;
        // Collected from a filter, so up to twice its length by capacity.
        let valid = 2 * self.valid_probes * size_of::<u32>() as u64;
        let union = 2 * (self.valid_probes.min(self.depth_cap) + 1) * UNION_ENTRY_BYTES;
        2 * absent_depth + slots + cells + valid + union + self.name_bytes
    }

    /// What finishing one mover holds beyond its states, the blocks and the
    /// loop ([`Self::loop_bytes`]): its skips' rebuilt copy of the states,
    /// one more mover's states at their bound.
    pub(crate) fn finish_bytes(&self) -> u64 {
        self.states_bytes()
    }

    /// What one pose holds while it runs, beside the SH base's patch cache:
    /// its posed copy of the static patch radiance and its own sides', the
    /// re-shaded (patch, light) pairs, the later-bounce buffers over both
    /// patch sets, its gathered probes, the mover's other terminus, its mid
    /// poses' near-field work (the domain's static patches, each with its
    /// gathered bounce before the scatter, and the compact slots), and one
    /// pose's memo read or write. Id 34's kept later bounces add in place
    /// ([`Self::kept_later_bytes`]), so no tail sum is held.
    pub(crate) fn pose_bytes(&self, patch_count: u64, light_reach_total: u64) -> u64 {
        if self.movers == 0 {
            return 0;
        }
        let side = size_of::<[f32; 3]>() as u64;
        let patches = patch_count + self.max_mover_patches;
        let radiance = 2 * side * patches;
        let pairs = light_reach_total * (2 * size_of::<(u32, u32)>() as u64 + 2 * side);
        let later = if self.posed_later {
            3 * side * patches + 64 * side
        } else {
            0
        };
        let gathered = self.valid_probes
            * (size_of::<PosedProbe>() as u64 + GATHERED_TILE_BYTES + DEPTH_TILE_BYTES);
        let other_pose = self.valid_probes
            * (GATHERED_TILE_BYTES
                + DEPTH_TILE_BYTES
                + size_of::<ChangedProbe>() as u64
                + size_of::<(u32, Vec<[u16; 2]>)>() as u64);
        let tables = self.total_probes * 4 + 256 * size_of::<[f32; 3]>() as u64;
        let near_field = if self.near_field_later {
            patch_count * (size_of::<u32>() as u64 + side) + self.total_probes * 4
        } else {
            0
        };
        radiance
            + near_field
            + pairs
            + later
            + gathered
            + other_pose
            + tables
            + self.max_mid_pose_bytes
            + self.max_mover_states_bytes
    }

    /// Id 34's later bounces kept for the movers, one front side per patch
    /// each.
    pub(crate) fn kept_later_bytes(&self, patch_count: u64) -> u64 {
        self.kept_later_bounces * patch_count * size_of::<[f32; 3]>() as u64
    }
}

/// One mover's reduced states at their bound over `valid` valid probes with
/// `poses` poses and `domain` near-field probes: every valid probe affected
/// when it re-bakes `radiance`, and carrying depth up to `records` (one past
/// the tile range), with a mark per pose and a tile at each terminus, and a
/// tile at every mid pose over the domain. A mover at `light_bounces` 0
/// stores no bounce state.
fn mover_states_bytes(valid: u64, records: u64, poses: u64, domain: u64, radiance: bool) -> u64 {
    let mids = poses.saturating_sub(2);
    let marks = poses * MARK_BYTES;
    let bounce = if radiance {
        valid * (2 * STATE_TILE_BYTES + size_of::<BounceProbe>() as u64 + marks)
            + mids * domain * STATE_TILE_BYTES
    } else {
        0
    };
    let depth = records * (2 * DEPTH_TILE_BYTES + size_of::<DepthProbe>() as u64 + marks)
        + mids * domain.min(records) * DEPTH_TILE_BYTES;
    bounce + depth
}

/// What one mover's `mids` mid poses hold over its `domain` near-field
/// probes while it bakes: each mid pose's classified states (a stored tile
/// and a depth tile per probe at most), and one mid pose's gather in flight
/// with its classification (the probe's id-34 tile beside its packed one).
/// Nothing without a mid pose.
fn mid_pose_bytes(mids: u64, domain: u64) -> u64 {
    if mids == 0 {
        return 0;
    }
    let state = size_of::<(u32, MidState<Vec<OctahedralAtlasTexel>>)>() as u64
        + GATHERED_TILE_BYTES
        + size_of::<(u32, MidState<Vec<[u16; 2]>>)>() as u64
        + DEPTH_TILE_BYTES;
    let gather = size_of::<DomainProbe>() as u64 + DEPTH_TILE_BYTES + GATHERED_TILE_BYTES + state;
    domain * (mids * state + gather)
}

/// What one pose of `mover` holds from placement until the SH base gate,
/// priced at `pose`: its knot; the pose as placed; its posed solid as
/// [`PosedMovers::resolve`] builds it, with the `Arc` that holds it, the
/// solid's own copy of the pose, its sides' geometry and face bounds; and
/// the posed scene the bounce plan builds over those sides, with its patch
/// set. Every pose of a mover is the same brushes translated, so one pose
/// prices them all; a translated side can cover a few more or fewer patch
/// cells, so this is an estimate. The scene is priced even on a map with no
/// probe grid, which builds none. Sides past the patch cap price no patches:
/// the bounce plan refuses them by name. The patch bytes are counted, not
/// built ([`PatchSet::try_heap_bytes`]).
pub(crate) fn pose_held_bytes(
    mover: &MapKinematicMover,
    pose: &Pose,
    texture_names: &TextureNamesSection,
) -> u64 {
    let knot = size_of::<f32>() as u64;
    let placed = size_of::<Pose>() as u64 + pose.brush_heap_bytes();
    let solid = PosedSolid::new(mover, pose, texture_names);
    // The pose's slot in the mover's solids, and the `Arc`'s two counts
    // beside the solid on the heap.
    let solid_bytes =
        (size_of::<Arc<PosedSolid>>() + 2 * size_of::<usize>() + size_of::<PosedSolid>()) as u64
            + solid.pose().brush_heap_bytes()
            + solid.heap_bytes();
    // Counted without building the scene's set, so pricing allocates
    // nothing the bake keeps or frees later.
    let patches = PatchSet::try_heap_bytes(solid.geometry()).unwrap_or(0);
    let scene = size_of::<PosedScene>() as u64 + patches;
    knot + placed + solid_bytes + scene
}

/// The direct half's entries at their bound, priced with the bake's own
/// prefilter and no ray. The bake writes an entry for a (light, mover, cell)
/// only where a valid probe of the cell lies inside the light's reach and
/// some pose of the mover may meet a sample segment from it
/// ([`super::visibility::PosedOccluder::may_meet`]); the entry then carries a
/// `q` per pose of the mover and the light's tile for every valid probe of
/// the cell.
/// So this sums, over exactly those (light, mover, cell) triples, the cell's
/// valid probes at their payload plus one entry header, and one record per
/// light that meets any mover. A point or spot light
/// reaches no probe past its falloff range (taken a metre wider); a sun
/// reaches every probe. Never below what the bake holds; the movers a light
/// cannot meet, and the cells no pose shadows, cost nothing.
pub(crate) fn direct_entry_bound(
    lights: &[&MapLight],
    posed: &PosedMovers,
    layout: &ProbeGridLayout,
) -> u64 {
    if posed.is_empty() || layout.is_empty() {
        return 0;
    }
    let cells = ProbeCells::of_layout(layout);
    let entry_bytes = |cell: usize, poses: usize| {
        u64::from(cells.valid_masks[cell].count_ones())
            * (poses + MOVER_IRRADIANCE_TILE_F16_COUNT * 2) as u64
            + size_of::<DirectEntry>() as u64
    };
    let mut total = 0u64;
    let mut met: HashSet<(usize, usize)> = HashSet::new();
    for light in lights {
        let movers = light_movers(light, posed);
        if movers.is_empty() {
            continue;
        }
        met.clear();
        for_each_probe_in_reach(layout, light, |probe| {
            let cell = cell_of_probe(layout, probe);
            let point = vec3_from(layout.probe_positions[probe]);
            for (slot, mover) in movers.iter().enumerate() {
                if !met.contains(&(cell, slot))
                    && mover
                        .poses
                        .iter()
                        .flatten()
                        .any(|pose| pose.may_meet(point, light))
                {
                    met.insert((cell, slot));
                }
            }
        });
        if !met.is_empty() {
            // The light's own record of its entries.
            total += size_of::<LightEntries>() as u64;
        }
        total += met
            .iter()
            .map(|&(cell, slot)| entry_bytes(cell, movers[slot].poses.len()))
            .sum::<u64>();
    }
    total
}

/// Every valid probe a light can reach: within a metre past a point or spot
/// light's falloff range, or anywhere for a sun.
fn for_each_probe_in_reach(
    layout: &ProbeGridLayout,
    light: &MapLight,
    mut visit: impl FnMut(usize),
) {
    let ball = match light.light_type {
        LightType::Directional => None,
        LightType::Point | LightType::Spot => {
            Some((light.origin, f64::from(light.falloff_range.max(0.0)) + 1.0))
        }
    };
    let Some((centre, radius)) = ball else {
        (0..layout.total_probes())
            .filter(|&probe| layout.validity[probe] != 0)
            .for_each(visit);
        return;
    };
    let spacing = f64::from(layout.cell_size[0]).max(1.0e-6);
    let lo = ((centre - radius - layout.world_min) / spacing).floor();
    let hi = ((centre + radius - layout.world_min) / spacing).ceil();
    let clamp = |value: f64, dim: u32| value.clamp(0.0, f64::from(dim) - 1.0) as usize;
    let [nx, ny, nz] = layout.dims;
    for z in clamp(lo.z, nz)..=clamp(hi.z, nz) {
        for y in clamp(lo.y, ny)..=clamp(hi.y, ny) {
            for x in clamp(lo.x, nx)..=clamp(hi.x, nx) {
                let probe = x + y * nx as usize + z * nx as usize * ny as usize;
                if layout.validity[probe] != 0
                    && layout.probe_positions[probe].distance(centre) <= radius
                {
                    visit(probe);
                }
            }
        }
    }
}

/// The dense affinity cell holding dense probe `probe`.
fn cell_of_probe(layout: &ProbeGridLayout, probe: usize) -> usize {
    let [nx, ny, _] = layout.dims.map(|dim| dim as usize);
    let [ax, ay, _] = layout.dims.map(|dim| dim.div_ceil(4) as usize);
    let (x, y, z) = (probe % nx, probe / nx % ny, probe / (nx * ny));
    x / 4 + y / 4 * ax + z / 4 * ax * ay
}

#[cfg(test)]
#[path = "cost_tests.rs"]
mod tests;
