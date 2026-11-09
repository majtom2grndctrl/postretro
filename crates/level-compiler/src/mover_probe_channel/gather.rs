// The posed probe gather: id 34's gather with one mover posed, its absolute
// bounce state per probe, and closed-pose depth tiles from the same rays.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::cell::Cell;

use glam::Vec3;
use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
};
use postretro_level_format::probe_depth_tiles::bordered_tile;
use postretro_level_format::sh_volume::{OctahedralAtlasTexel, OctahedralShVolumeSection};
use rayon::prelude::*;

use super::InsideBrushRule;
use super::bounce::PosedRadiance;
use super::domain::{
    BOUNCE_FLOOR_LEVELS, compact_tile, display_levels, max_abs_difference, round_trip_error,
};
use super::posed_scene::{PosedHit, PosedScene, PosedSurfaces};
use crate::bake_control::BakeControl;
use crate::sh_bake::{
    Hit, ProbeGridLayout, RadianceSource, RaytracingCtx, SKY_COLOR, ShBakeCtx, TileRayTable,
    bake_probe_with_source, gather_hit, pack_octahedral_irradiance_tile, probe_gather_directions,
    static_light_refs, vec3_from,
};
use crate::surface_radiance::{PatchGather, PatchLookup, PatchSet};

thread_local! {
    /// Whether a gather ray of the probe this thread is gathering met the
    /// pose first. A probe gathers all its rays on one thread.
    static MET: Cell<bool> = const { Cell::new(false) };
}

/// What one gather ray from a probe meets with the mover posed.
enum PosedRay {
    /// The probe lies inside a posed brush: every ray reads distance 0.
    Inside,
    /// The pose, nearer than any static hit.
    Posed(PosedHit),
    Static(Hit),
    Sky,
}

impl PosedRay {
    /// One ray of the probe at `origin` along `dir`: the nearer of the static
    /// hit and the pose's. A probe inside a posed brush (with contact)
    /// meets it on every ray, at distance 0, so its depth floors its weight.
    fn trace(ctx: &RaytracingCtx<'_>, scene: &PosedScene, origin: Vec3, dir: Vec3) -> Self {
        if !scene.containing(origin).is_empty() {
            return Self::Inside;
        }
        let hit = gather_hit(ctx, origin, dir);
        let nearest = hit.as_ref().map_or(f32::INFINITY, |hit| hit.distance);
        match (scene.trace(origin, dir, nearest), hit) {
            (Some(posed), _) => Self::Posed(posed),
            (None, Some(hit)) => Self::Static(hit),
            (None, None) => Self::Sky,
        }
    }

    fn met(&self) -> bool {
        matches!(self, Self::Inside | Self::Posed(_))
    }

    fn distance(&self, far_sentinel: f32) -> f32 {
        match self {
            Self::Inside => 0.0,
            Self::Posed(posed) => posed.distance,
            Self::Static(hit) => hit.distance,
            Self::Sky => far_sentinel,
        }
    }
}

/// The probe gather's radiance source with one mover posed.
pub(crate) struct PosedGather<'a> {
    world: PatchGather<'a>,
    scene: &'a PosedScene,
    radiance: &'a PosedRadiance,
}

impl<'a> PosedGather<'a> {
    pub(crate) fn new(
        patches: &'a PatchSet,
        radiance: &'a PosedRadiance,
        scene: &'a PosedScene,
    ) -> Self {
        Self {
            world: PatchGather::new(patches, &radiance.world, PatchLookup::PRODUCTION),
            scene,
            radiance,
        }
    }

    /// One gather ray from the probe at `origin`: the nearer of the static
    /// hit and the pose's, and its radiance. A probe inside a posed brush
    /// reads every ray at distance 0, and black under the dark rule, as its
    /// direct does; under the skip rule it reads past the brushes holding it.
    pub(crate) fn sample(
        &self,
        ctx: &RaytracingCtx<'_>,
        origin: Vec3,
        dir: Vec3,
        far_sentinel: f32,
    ) -> (Vec3, f32) {
        let ray = PosedRay::trace(ctx, self.scene, origin, dir);
        if ray.met() {
            MET.set(true);
        }
        let radiance = match &ray {
            PosedRay::Inside => match self.scene.rule {
                InsideBrushRule::Dark => Vec3::ZERO,
                // The dev A/B only: the static world alone past the brush.
                InsideBrushRule::SkipContainingBrush => match gather_hit(ctx, origin, dir) {
                    None => Vec3::from(SKY_COLOR),
                    Some(hit) => self.world.radiance_at(&hit, dir),
                },
            },
            PosedRay::Posed(posed) => match (self.scene.surfaces, posed.triangle) {
                (PosedSurfaces::Patches, Some(triangle)) => self
                    .scene
                    .patches
                    .lookup_bilinear(triangle, posed.point, dir)
                    .map_or(Vec3::ZERO, |(blend, side)| {
                        blend
                            .entries()
                            .iter()
                            .map(|&(patch, w)| self.radiance.mover.get(patch, side) * w)
                            .sum()
                    }),
                _ => Vec3::ZERO,
            },
            PosedRay::Static(hit) => self.world.radiance_at(hit, dir),
            PosedRay::Sky => Vec3::from(SKY_COLOR),
        };
        (radiance, ray.distance(far_sentinel))
    }
}

/// One valid probe the posed gather changed.
pub(crate) struct PosedProbe {
    /// Dense probe index.
    pub(crate) probe: u32,
    /// Compact slot in id 34's valid-probe order.
    pub(crate) slot: u32,
    /// Distance from the pose's bounds, m.
    pub(crate) distance_m: f32,
    /// Largest |S^b − id 34| over the tile, and the id-34 tile's own
    /// round-trip error: the bar.
    pub(crate) difference: f32,
    pub(crate) bar: f32,
    /// The same change in display levels ([`display_levels`]).
    pub(crate) levels: f32,
    /// S^b's tile, kept when the probe is affected (or every probe, asked).
    pub(crate) tile: Option<Vec<OctahedralAtlasTexel>>,
    /// The closed-pose bordered depth tile, when a gather ray meets the pose.
    pub(crate) depth: Option<Vec<[u16; 2]>>,
}

impl PosedProbe {
    /// Past both the round-trip bar and the perceptual floor
    /// ([`BOUNCE_FLOOR_LEVELS`]): the affected-probe rule.
    pub(crate) fn affected(&self) -> bool {
        self.difference > self.bar && self.levels > BOUNCE_FLOOR_LEVELS
    }

    /// Heap bytes the probe holds beside its own size.
    #[cfg(test)]
    pub(crate) fn heap_bytes(&self) -> u64 {
        (self.tile.as_ref().map_or(0, |tile| {
            tile.capacity() * size_of::<OctahedralAtlasTexel>()
        }) + self
            .depth
            .as_ref()
            .map_or(0, |depth| depth.capacity() * size_of::<[u16; 2]>())) as u64
    }
}

/// Each valid probe's compact slot in id 34's valid-probe order, indexed by
/// dense probe.
pub(crate) fn compact_slots(layout: &ProbeGridLayout) -> Vec<u32> {
    let mut slots = Vec::with_capacity(layout.total_probes());
    let mut slot = 0u32;
    for &valid in &layout.validity {
        slots.push(slot);
        slot += u32::from(valid != 0);
    }
    slots
}

/// Gather every valid probe of `layout` under `gather`, as id 34's gather
/// does, and keep each probe that differs from id 34 (`base`, the compact
/// RGBA16F intermediate) or whose rays meet the pose. A probe whose rays
/// meet nothing posed and whose hits read their static radiance gathers
/// id 34's bytes exactly, so it differs by zero.
// The gather's inputs plus the comparison's.
#[allow(clippy::too_many_arguments)]
pub(crate) fn gather_posed_probes(
    sh_ctx: &ShBakeCtx<'_>,
    layout: &ProbeGridLayout,
    gather: &PosedGather<'_>,
    scene: &PosedScene,
    base: &OctahedralShVolumeSection,
    uncompressed: bool,
    keep_every_tile: bool,
    control: &BakeControl,
) -> Vec<PosedProbe> {
    let directions = probe_gather_directions();
    let rays = TileRayTable::new(&directions);
    let lights = static_light_refs(sh_ctx);
    let slots = compact_slots(layout);
    let dim = DEFAULT_IRRADIANCE_TILE_DIMENSION as usize;
    (0..layout.total_probes())
        .into_par_iter()
        .filter(|&i| layout.validity[i] != 0)
        .filter_map(|i| {
            let _permit = control.governor().enter();
            let position = vec3_from(layout.probe_positions[i]);
            let mut distances = [0.0f32; 256];
            MET.set(false);
            let baked = bake_probe_with_source(
                &directions,
                sh_ctx,
                position,
                &lights,
                None,
                layout.far_sentinel,
                true,
                i as u64,
                RadianceSource::Posed(gather),
                &mut distances,
            );
            let tile = pack_octahedral_irradiance_tile(
                &baked.coefficients,
                true,
                DEFAULT_IRRADIANCE_TILE_DIMENSION,
                DEFAULT_IRRADIANCE_TILE_BORDER,
            );
            let base_tile = compact_tile(base, slots[i] as usize);
            let difference = max_abs_difference(&tile, &base_tile);
            let met = MET.get();
            if difference == 0.0 && !met && !keep_every_tile {
                return None;
            }
            let bar = if difference > 0.0 {
                round_trip_error(&base_tile, dim, uncompressed)
            } else {
                0.0
            };
            let levels = if difference > 0.0 {
                display_levels(&tile, &base_tile)
            } else {
                0.0
            };
            let mut probe = PosedProbe {
                probe: i as u32,
                slot: slots[i],
                distance_m: scene.distance_to_bounds(position) as f32,
                difference,
                bar,
                levels,
                tile: None,
                depth: met.then(|| bordered_tile(&rays.interior(&distances))),
            };
            if probe.affected() || keep_every_tile {
                probe.tile = Some(tile);
            }
            Some(probe)
        })
        .collect()
}

/// The closed-pose depth of a mover that bakes no bounce (`light_bounces`
/// 0): every valid probe some gather ray of which meets the pose, with the
/// tile the full posed gather bins from the same rays, ascending by probe.
/// A probe none of whose fixed directions points into the pose's bounds
/// cannot meet it, so it traces nothing.
pub(crate) fn trace_posed_depth(
    sh_ctx: &ShBakeCtx<'_>,
    layout: &ProbeGridLayout,
    scene: &PosedScene,
    control: &BakeControl,
) -> Vec<(u32, Vec<[u16; 2]>)> {
    let directions = probe_gather_directions();
    let rays = TileRayTable::new(&directions);
    let ctx = sh_ctx.ray_ctx();
    (0..layout.total_probes())
        .into_par_iter()
        .filter(|&i| layout.validity[i] != 0)
        .filter_map(|i| {
            let position = vec3_from(layout.probe_positions[i]);
            if scene.containing(position).is_empty()
                && !directions
                    .iter()
                    .any(|&dir| scene.may_meet_ray(position, dir))
            {
                return None;
            }
            let _permit = control.governor().enter();
            let mut distances = [0.0f32; 256];
            let mut met = false;
            for (slot, &dir) in distances.iter_mut().zip(&directions) {
                let ray = PosedRay::trace(&ctx, scene, position, dir);
                met |= ray.met();
                *slot = ray.distance(layout.far_sentinel);
            }
            met.then(|| (i as u32, bordered_tile(&rays.interior(&distances))))
        })
        .collect()
}

/// The mover-absent depth tile of valid probe `probe`: its gather rays
/// against the static world alone, binned as id 59 bins them. What the
/// posed gather bins at a terminus where no ray of the probe meets the pose.
pub(crate) fn static_depth_tile(
    ctx: &RaytracingCtx<'_>,
    layout: &ProbeGridLayout,
    rays: &TileRayTable,
    directions: &[Vec3],
    probe: u32,
) -> Vec<[u16; 2]> {
    let position = vec3_from(layout.probe_positions[probe as usize]);
    let mut distances = [0.0f32; 256];
    for (slot, &dir) in distances.iter_mut().zip(directions) {
        *slot = gather_hit(ctx, position, dir).map_or(layout.far_sentinel, |hit| hit.distance);
    }
    bordered_tile(&rays.interior(&distances))
}
