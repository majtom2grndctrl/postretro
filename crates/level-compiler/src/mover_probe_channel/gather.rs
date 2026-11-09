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
use super::domain::{compact_tile, display_levels, max_abs_difference, round_trip_error};
use super::posed_scene::{PosedScene, PosedSurfaces};
use crate::bake_control::BakeControl;
use crate::sh_bake::{
    ProbeGridLayout, RAY_EPSILON, RadianceSource, RaytracingCtx, SKY_COLOR, ShBakeCtx,
    TileRayTable, bake_probe_with_source, gather_hit, pack_octahedral_irradiance_tile,
    probe_gather_directions, static_light_refs, vec3_from,
};
use crate::surface_radiance::{PatchGather, PatchLookup, PatchSet};

thread_local! {
    /// Whether a gather ray of the probe this thread is gathering met the
    /// pose first. A probe gathers all its rays on one thread.
    static MET: Cell<bool> = const { Cell::new(false) };
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
    /// hit and the pose's entry, and its radiance. A probe inside a posed
    /// brush reads every ray at distance 0, so its depth floors its weight;
    /// its radiance follows the inside-brush rule, as its direct does.
    pub(crate) fn sample(
        &self,
        ctx: &RaytracingCtx<'_>,
        origin: Vec3,
        dir: Vec3,
        far_sentinel: f32,
    ) -> (Vec3, f32) {
        let containing = self.scene.containing(origin);
        if containing.is_empty() {
            return self.along(ctx, origin, dir, far_sentinel, &[]);
        }
        MET.set(true);
        let radiance = match self.scene.rule {
            InsideBrushRule::Dark => Vec3::ZERO,
            InsideBrushRule::SkipContainingBrush => {
                self.along(ctx, origin, dir, far_sentinel, &containing).0
            }
        };
        (radiance, 0.0)
    }

    fn along(
        &self,
        ctx: &RaytracingCtx<'_>,
        origin: Vec3,
        dir: Vec3,
        far_sentinel: f32,
        skip: &[usize],
    ) -> (Vec3, f32) {
        let hit = gather_hit(ctx, origin, dir);
        let nearest = hit.as_ref().map_or(f32::INFINITY, |hit| hit.distance);
        // A posed face flush with a static one leaves the static hit.
        if let Some(posed) = self
            .scene
            .ray_entry(
                origin + dir * RAY_EPSILON,
                dir,
                nearest,
                skip,
                super::dev_trace::posed_tau(),
            )
            .filter(|posed| posed.distance < nearest)
        {
            MET.set(true);
            let radiance = match (self.scene.surfaces, posed.face) {
                (PosedSurfaces::Patches, Some(face)) => self
                    .scene
                    .patches
                    .lookup_bilinear(self.scene.face_triangle(face), posed.point, dir)
                    .map_or(Vec3::ZERO, |(blend, side)| {
                        blend
                            .entries()
                            .iter()
                            .map(|&(patch, w)| self.radiance.mover.get(patch, side) * w)
                            .sum()
                    }),
                _ => Vec3::ZERO,
            };
            return (radiance, posed.distance);
        }
        match hit {
            None => (Vec3::from(SKY_COLOR), far_sentinel),
            Some(hit) => (self.world.radiance_at(&hit, dir), hit.distance),
        }
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
    /// Past the round-trip bar and, when one is set, the perceptual floor.
    pub(crate) fn affected(&self) -> bool {
        self.difference > self.bar
            && super::dev_floor::bounce_floor().is_none_or(|floor| self.levels > floor)
    }
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
    let mut slots = Vec::with_capacity(layout.total_probes());
    let mut slot = 0u32;
    for &valid in &layout.validity {
        slots.push(slot);
        slot += u32::from(valid != 0);
    }
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
