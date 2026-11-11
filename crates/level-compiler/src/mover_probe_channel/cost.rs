// What the mover probe channel holds beside the SH base, priced before any
// ray: the inputs the SH working-set gate folds into its phases.
// See: context/lib/build_pipeline.md §Compiler pipeline (working-set gate)

use std::collections::HashSet;

use postretro_level_format::mover_probe_channel::{MOVER_IRRADIANCE_TILE_F16_COUNT, PoseMark};
use postretro_level_format::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;
use postretro_level_format::sh_volume::OctahedralAtlasTexel;

use super::absent_bounces::AbsentLaterBounces;
use super::bake::MoverBounceJob;
use super::direct::{DirectEntry, ProbeCells, light_movers};
use super::gather::{DomainProbe, PosedProbe};
use super::posed_solid::PosedMovers;
use super::states::{BounceProbe, ChangedProbe, DepthProbe, MidState};
use crate::map_data::{LightType, MapLight};
use crate::sh_bake::{ProbeGridLayout, vec3_from};

/// One gathered tile, border included, as the posed gather holds it.
const GATHERED_TILE_BYTES: u64 = 36 * size_of::<OctahedralAtlasTexel>() as u64;
/// One closed-pose depth tile.
const DEPTH_TILE_BYTES: u64 = (PROBE_DEPTH_TILE_TEXELS * size_of::<[u16; 2]>()) as u64;
/// One stored bounce state, id 27's compact RGB tile.
const STATE_TILE_BYTES: u64 = (MOVER_IRRADIANCE_TILE_F16_COUNT * size_of::<u16>()) as u64;
const MARK_BYTES: u64 = size_of::<PoseMark>() as u64;

/// What the mover probe channel adds to the SH working set, from the map's
/// movers, its posed scenes and its probe grid alone.
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
    /// Id 34's later bounces the bake keeps apart: past the posed ones of
    /// movers whose `light_bounces` stops short, and past the near-field
    /// domain of movers with mid poses.
    pub(crate) kept_later_bounces: u64,
    pub(crate) valid_probes: u64,
    pub(crate) total_probes: u64,
    /// The tile range mover depth may fill: no mover keeps more than one
    /// depth record past it.
    pub(crate) depth_cap: u64,
    /// The direct half's entries at their bound.
    pub(crate) direct_entry_bytes: u64,
    /// Over the movers re-baking radiance: Σ pose count (each bounce
    /// probe's marks), and Σ (poses − 2) × near-field probes (the most
    /// mid-pose states they store).
    pub(crate) bounce_pose_marks: u64,
    pub(crate) bounce_mid_tiles: u64,
    /// Over every mover: Σ pose count (each depth record's marks), and Σ
    /// (poses − 2) × near-field probes up to one past the tile range (the
    /// most mid-pose depth tiles it stores).
    pub(crate) depth_pose_marks: u64,
    pub(crate) depth_mid_tiles: u64,
    /// The most any one mover's states hold at their bound.
    pub(crate) max_mover_states_bytes: u64,
    /// The most any one mover's mid poses hold while it bakes: every mid
    /// pose's states and one mid pose's gather.
    pub(crate) max_mid_pose_bytes: u64,
    /// Whether some mover re-baking radiance has a mid pose: its posed
    /// later bounces then gather the domain's static patches apart before
    /// they scatter.
    pub(crate) near_field_later: bool,
}

impl MoverChannelCost {
    /// The channel's cost for `jobs` over `layout` at `fill`, with mover
    /// depth in a tile range of `depth_cap` and the direct half's bound over
    /// `direct_lights` (every static and animated baked light).
    pub(crate) fn new(
        jobs: &[MoverBounceJob],
        layout: &ProbeGridLayout,
        fill: f32,
        depth_cap: u32,
        direct_lights: &[&MapLight],
        movers: &PosedMovers,
    ) -> Self {
        if jobs.is_empty() {
            return Self::default();
        }
        let scenes = || jobs.iter().flat_map(|job| &job.scenes);
        let valid_probes = layout.validity.iter().filter(|&&valid| valid != 0).count() as u64;
        let records = |domain: u64| domain.min(u64::from(depth_cap) + 1);
        let mids = |job: &MoverBounceJob| job.scenes.len().saturating_sub(2) as u64;
        let domain = |job: &MoverBounceJob| job.domain_probes.len() as u64;
        let radiance = || jobs.iter().filter(|job| job.bounces > 0);
        Self {
            movers: jobs.len() as u64,
            radiance_movers: jobs.iter().filter(|job| job.bounces > 0).count() as u64,
            scene_bytes: scenes().map(super::PosedScene::heap_bytes).sum(),
            solid_bytes: movers.heap_bytes(),
            max_mover_patches: scenes()
                .map(|scene| scene.patches.len() as u64)
                .max()
                .unwrap_or(0),
            kept_later_bounces: kept_later_bounces(jobs, fill),
            valid_probes,
            total_probes: layout.total_probes() as u64,
            depth_cap: u64::from(depth_cap),
            direct_entry_bytes: direct_entry_bound(direct_lights, movers, layout),
            bounce_pose_marks: radiance().map(|job| job.scenes.len() as u64).sum(),
            bounce_mid_tiles: radiance().map(|job| mids(job) * domain(job)).sum(),
            depth_pose_marks: jobs.iter().map(|job| job.scenes.len() as u64).sum(),
            depth_mid_tiles: jobs
                .iter()
                .map(|job| mids(job) * records(domain(job)))
                .sum(),
            max_mover_states_bytes: jobs
                .iter()
                .map(|job| mover_states_bytes(valid_probes, job.scenes.len() as u64, domain(job)))
                .max()
                .unwrap_or(0),
            max_mid_pose_bytes: jobs
                .iter()
                .map(|job| mid_pose_bytes(mids(job), domain(job)))
                .max()
                .unwrap_or(0),
            near_field_later: fill != 0.0 && radiance().any(|job| mids(job) > 0),
        }
    }

    /// Every mover's states held at once: a bounce state per valid probe for
    /// each mover re-baking radiance, and for each mover a depth record per
    /// valid probe up to one past the tile range.
    ///
    /// Depth is counted per mover, never over distinct probes: several
    /// movers over the same probes each keep a record there. Each mover's
    /// records are bounded by its own reduction: below-floor probes drop as
    /// its two poses reduce, and it keeps at most one record past the range
    /// ([`super::states::reduce_poses`]), since one mover past the range
    /// fails the build by name. The range rides id 34's volume key, so no
    /// memo is read under another range; the cut on read handles entries
    /// written before past-range movers stopped being memoized. The raw
    /// traced tiles a mover's poses hold before the reduction are one
    /// mover's at a time, priced in [`Self::pose_bytes`].
    ///
    /// Per pose: every probe's marks run one per pose, the termini store at
    /// most one tile each per probe, and a mid pose stores tiles only over
    /// the mover's near-field domain.
    pub(crate) fn accumulated_states_bytes(&self) -> u64 {
        let bounce = self.radiance_movers
            * self.valid_probes
            * (2 * STATE_TILE_BYTES + size_of::<BounceProbe>() as u64)
            + self.valid_probes * self.bounce_pose_marks * MARK_BYTES
            + self.bounce_mid_tiles * STATE_TILE_BYTES;
        let records = self.valid_probes.min(self.depth_cap + 1);
        let depth = self.movers * records * (2 * DEPTH_TILE_BYTES + size_of::<DepthProbe>() as u64)
            + records * self.depth_pose_marks * MARK_BYTES
            + self.depth_mid_tiles * DEPTH_TILE_BYTES;
        bounce + depth
    }

    /// What one pose holds while it runs, beside the SH base's patch cache:
    /// its posed copy of the static patch radiance and its own sides', the
    /// re-shaded (patch, light) pairs, the later-bounce buffers over both
    /// patch sets, its gathered probes, the mover's other terminus, its mid
    /// poses' near-field work (the domain's static patches, each with its
    /// gathered bounce before the scatter, and the compact slots), and one
    /// mover's memo write. Id 34's kept later bounces add in place
    /// ([`Self::kept_later_bytes`]), so no tail sum is held.
    pub(crate) fn pose_bytes(&self, patch_count: u64, light_reach_total: u64) -> u64 {
        if self.movers == 0 {
            return 0;
        }
        let side = size_of::<[f32; 3]>() as u64;
        let patches = patch_count + self.max_mover_patches;
        let radiance = 2 * side * patches;
        let pairs = light_reach_total * (2 * size_of::<(u32, u32)>() as u64 + 2 * side);
        let later = 3 * side * patches + 64 * side;
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

/// One mover's states at their bound over `valid` valid probes with `poses`
/// poses and `domain` near-field probes: every valid probe affected and
/// carrying depth, with a mark per pose and a tile at each terminus, and a
/// tile at every mid pose over the domain.
fn mover_states_bytes(valid: u64, poses: u64, domain: u64) -> u64 {
    let mids = poses.saturating_sub(2);
    valid
        * (2 * STATE_TILE_BYTES
            + size_of::<BounceProbe>() as u64
            + 2 * DEPTH_TILE_BYTES
            + size_of::<DepthProbe>() as u64
            + 2 * poses * MARK_BYTES)
        + mids * domain * (STATE_TILE_BYTES + DEPTH_TILE_BYTES)
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

/// The SH base's own later bounces the jobs keep: every one, when some job
/// reads them ([`AbsentLaterBounces::needed_by`]).
pub(crate) fn kept_later_bounces<'j>(
    jobs: impl IntoIterator<Item = &'j MoverBounceJob>,
    fill: f32,
) -> u64 {
    if AbsentLaterBounces::needed_by(jobs, fill) {
        u64::from(crate::surface_radiance::LATER_BOUNCE_COUNT)
    } else {
        0
    }
}

/// The direct half's entries at their bound, priced with the bake's own
/// prefilter and no ray. The bake writes an entry for a (light, mover, cell)
/// only where a valid probe of the cell lies inside the light's reach and
/// some pose of the mover may meet a sample segment from it
/// ([`super::visibility::PosedOccluder::may_meet`]); the entry then carries a
/// `q` per pose of the mover and the light's tile for every valid probe of
/// the cell.
/// So this sums, over exactly those (light, mover, cell) triples, the cell's
/// valid probes at their payload plus one entry header. A point or spot light
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
mod tests {
    use postretro_level_format::probe_depth_tiles::{
        MAX_PROBE_DEPTH_TILES, PROBE_DEPTH_TILE_TEXELS,
    };

    use super::*;

    /// Bytes `movers` movers of `poses` poses hold keeping a depth record at
    /// each of the same `records` probes: a mark per pose, both termini's
    /// tiles, and a tile at every mid pose for the first `domain` of them.
    fn held_depth(movers: u64, records: u64, poses: usize, domain: u64) -> u64 {
        (0..movers)
            .map(|_| {
                let records: Vec<DepthProbe> = (0..records as u32)
                    .map(|probe| {
                        let stored = if u64::from(probe) < domain { poses } else { 2 };
                        DepthProbe {
                            probe,
                            marks: vec![PoseMark::Stored; poses],
                            tiles: vec![[0u16; 2]; stored * PROBE_DEPTH_TILE_TEXELS],
                        }
                    })
                    .collect();
                (records.capacity() * size_of::<DepthProbe>()) as u64
                    + records
                        .iter()
                        .map(|record| {
                            (record.tiles.capacity() * size_of::<[u16; 2]>()
                                + record.marks.capacity() * size_of::<PoseMark>())
                                as u64
                        })
                        .sum::<u64>()
            })
            .sum()
    }

    // Twenty four-pose movers over the same 3,000 valid probes, 500 of them
    // in each one's near-field domain, each keep a depth record there: under
    // the full range, 60,000 records, past the tile range plus one mover's
    // probes, and the bound prices every one with its marks and mid-pose
    // tiles. Under a range of 1,000, each mover keeps at most one record past
    // it, and the bound scales with the range, not the valid probes.
    #[test]
    fn many_movers_over_the_same_probes_are_priced_at_every_depth_record() {
        let (movers, valid, poses, domain) = (20u64, 3_000u64, 4usize, 500u64);
        let cost = |depth_cap: u32| {
            let records = domain.min(u64::from(depth_cap) + 1);
            MoverChannelCost {
                movers,
                valid_probes: valid,
                total_probes: valid,
                depth_cap: u64::from(depth_cap),
                depth_pose_marks: movers * poses as u64,
                depth_mid_tiles: movers * (poses as u64 - 2) * records,
                ..MoverChannelCost::default()
            }
        };

        let full = cost(MAX_PROBE_DEPTH_TILES);
        assert!(movers * valid > u64::from(MAX_PROBE_DEPTH_TILES) + valid);
        let held = held_depth(movers, valid, poses, domain);
        let priced = full.accumulated_states_bytes();
        assert!(priced >= held, "priced {priced} B under {held} B held");

        let cap = 1_000u32;
        let narrow = cost(cap);
        let held = held_depth(movers, u64::from(cap) + 1, poses, domain);
        let priced = narrow.accumulated_states_bytes();
        assert!(priced >= held, "priced {priced} B under {held} B held");
        let per_record = 2 * DEPTH_TILE_BYTES + size_of::<DepthProbe>() as u64;
        let records = u64::from(cap) + 1;
        assert_eq!(
            priced,
            movers * records * (per_record + poses as u64 * MARK_BYTES)
                + movers * (poses as u64 - 2) * domain * DEPTH_TILE_BYTES
        );
        assert!(priced < full.accumulated_states_bytes() / 2);
    }
}
