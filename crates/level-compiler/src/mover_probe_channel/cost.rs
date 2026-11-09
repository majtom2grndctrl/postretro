// What the mover probe channel holds beside the SH base, priced before any
// ray: the inputs the SH working-set gate folds into its phases.
// See: context/lib/build_pipeline.md §Compiler pipeline (working-set gate)

use std::collections::HashSet;

use postretro_level_format::mover_probe_channel::{
    MOVER_IRRADIANCE_TILE_F16_COUNT, MOVER_VISIBILITY_BYTES_PER_PROBE,
};
use postretro_level_format::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;
use postretro_level_format::sh_volume::OctahedralAtlasTexel;

use super::bake::MoverBounceJob;
use super::direct::{DirectEntry, ProbeCells, light_movers};
use super::gather::PosedProbe;
use super::posed_solid::PosedMovers;
use super::states::{BounceProbe, DepthProbe};
use crate::map_data::{LightType, MapLight};
use crate::sh_bake::{ProbeGridLayout, vec3_from};

/// One gathered tile, border included, as the posed gather holds it.
const GATHERED_TILE_BYTES: u64 = 36 * size_of::<OctahedralAtlasTexel>() as u64;
/// One closed-pose depth tile.
const DEPTH_TILE_BYTES: u64 = (PROBE_DEPTH_TILE_TEXELS * size_of::<[u16; 2]>()) as u64;

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
    /// Id 34's later bounces the bake keeps apart for movers whose
    /// `light_bounces` stops short.
    pub(crate) kept_later_bounces: u64,
    pub(crate) valid_probes: u64,
    pub(crate) total_probes: u64,
    /// The tile range mover depth may fill before the bake stops.
    pub(crate) depth_cap: u64,
    /// The direct half's entries at their bound.
    pub(crate) direct_entry_bytes: u64,
}

impl MoverChannelCost {
    /// The channel's cost for `jobs` over `layout` at `fill`, with the
    /// direct half's bound over `direct_lights` (every static and animated
    /// baked light) and mover depth filling at most `depth_cap` tiles.
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
            valid_probes: layout.validity.iter().filter(|&&valid| valid != 0).count() as u64,
            total_probes: layout.total_probes() as u64,
            depth_cap: u64::from(depth_cap),
            direct_entry_bytes: direct_entry_bound(direct_lights, movers, layout),
        }
    }

    /// One mover's states at their bound: every valid probe affected with two
    /// tiles, and carrying two depth tiles.
    fn mover_states_bytes(&self) -> u64 {
        self.valid_probes
            * (2 * MOVER_IRRADIANCE_TILE_F16_COUNT as u64 * 2
                + size_of::<BounceProbe>() as u64
                + 2 * DEPTH_TILE_BYTES
                + size_of::<DepthProbe>() as u64)
    }

    /// Every mover's states held at once: a bounce state per valid probe for
    /// each mover re-baking radiance, and a depth state per valid probe for
    /// each mover, up to the tile range plus the one mover whose depth passes
    /// it (the bake stops there).
    pub(crate) fn accumulated_states_bytes(&self) -> u64 {
        let bounce = self.radiance_movers
            * self.valid_probes
            * (2 * MOVER_IRRADIANCE_TILE_F16_COUNT as u64 * 2 + size_of::<BounceProbe>() as u64);
        let depth_records =
            (self.movers * self.valid_probes).min(self.depth_cap + self.valid_probes);
        let depth = depth_records * (2 * DEPTH_TILE_BYTES + size_of::<DepthProbe>() as u64);
        bounce + depth
    }

    /// What one pose holds while it runs, beside the SH base's patch cache:
    /// its posed copy of the static patch radiance and its own sides', the
    /// re-shaded (patch, light) pairs, the later-bounce buffers over both
    /// patch sets and the static tail's sum, its gathered probes, the
    /// mover's other pose, and one mover's memo write.
    pub(crate) fn pose_bytes(&self, patch_count: u64, light_reach_total: u64) -> u64 {
        if self.movers == 0 {
            return 0;
        }
        let side = size_of::<[f32; 3]>() as u64;
        let patches = patch_count + self.max_mover_patches;
        let radiance = 2 * side * patches;
        let pairs = light_reach_total * (2 * size_of::<(u32, u32)>() as u64 + 2 * side);
        let later = 3 * side * patches + side * patch_count + 64 * side;
        let gathered = self.valid_probes
            * (size_of::<PosedProbe>() as u64 + GATHERED_TILE_BYTES + DEPTH_TILE_BYTES);
        let other_pose = self.valid_probes
            * (GATHERED_TILE_BYTES + DEPTH_TILE_BYTES + 2 * 2 * size_of::<usize>() as u64 * 2);
        let tables = self.total_probes * 4 + 256 * size_of::<[f32; 3]>() as u64;
        radiance + pairs + later + gathered + other_pose + tables + self.mover_states_bytes()
    }

    /// Id 34's later bounces kept for the movers, one front side per patch
    /// each.
    pub(crate) fn kept_later_bytes(&self, patch_count: u64) -> u64 {
        self.kept_later_bounces * patch_count * size_of::<[f32; 3]>() as u64
    }
}

/// The SH base's own later bounces the jobs keep: from the first a job
/// keeps to the last.
pub(crate) fn kept_later_bounces<'j>(
    jobs: impl IntoIterator<Item = &'j MoverBounceJob>,
    fill: f32,
) -> u64 {
    super::MoverBounceRequest::static_tail_from(jobs, fill).map_or(0, |first| {
        u64::from(crate::surface_radiance::LATER_BOUNCE_COUNT.saturating_sub(first))
    })
}

/// The direct half's entries at their bound, priced with the bake's own
/// prefilter and no ray. The bake writes an entry for a (light, mover, cell)
/// only where a valid probe of the cell lies inside the light's reach and
/// some pose of the mover may meet a sample segment from it
/// ([`super::visibility::PosedOccluder::may_meet`]); the entry then carries a
/// visibility pair and the light's tile for every valid probe of the cell.
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
    let per_probe = (MOVER_VISIBILITY_BYTES_PER_PROBE + MOVER_IRRADIANCE_TILE_F16_COUNT * 2) as u64;
    let entry_bytes = |cell: usize| {
        u64::from(cells.valid_masks[cell].count_ones()) * per_probe
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
        total += met.iter().map(|&(cell, _)| entry_bytes(cell)).sum::<u64>();
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
