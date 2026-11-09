// Mover probe channel, direct half: per (mover, terminus, gated light, valid
// probe) the share q of the light's baked soft visibility that survives with
// the mover posed, and the light's own per-probe tile T.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::cell::RefCell;

use glam::Vec3;
use postretro_level_format::delta_sh_volumes::DELTA_TILE_TEXEL_F16_COUNT;
use postretro_level_format::mover_probe_channel::{
    MOVER_IRRADIANCE_TILE_F16_COUNT, MOVER_VISIBILITY_BYTES_PER_PROBE, MoverLightNamespace,
};
use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
};
use postretro_level_format::sh_reconstruct::local_xyz;
use rayon::prelude::*;

use super::direct_memo::{DirectMemo, MemoPrefix};
use super::visibility::{INSIDE_BRUSH_RULE, PosedOccluder, joint_soft_visibility};
use crate::affinity_grid::{
    AFFINITY_FACTOR, AffinityReachInputs, AffinityReachPolicy,
    decompose_affinity_for_lights_with_policies,
};
use crate::bake_control::BakeControl;
use crate::cache::{CacheKey, StageCache};
use crate::lightmap_bake::{DEFAULT_AREA_SAMPLE_COUNT, emitter_radius};
use crate::map_data::{LightType, MapLight};
use crate::mover_light_gate::{BOX_SLACK_M, MoverPoses, Terminus, sphere_touches_box};
use crate::ray_traversal::RAY_EPSILON;
use crate::sh_bake::{
    ProbeGridLayout, RaytracingCtx, bake_probe_direct_rgb_posed, incident_radiance_at_point,
    pack_octahedral_irradiance_tile, segment_clear, soft_visibility_seed, vec3_from,
};
use crate::texel_footprint::ProbeEarlyOut;

/// One light the direct half gates, as id 35 (static) or id 45 (animated)
/// bakes it.
#[derive(Debug, Clone)]
pub(crate) struct ChannelLight<'a> {
    pub(crate) namespace: MoverLightNamespace,
    /// Its id-57 light index: AlphaLights for a static light, the animated
    /// roster for an animated one.
    pub(crate) light_index: u32,
    /// The light as its section bakes it: authored for id 35, at unit
    /// radiance for id 45.
    pub(crate) baked: MapLight,
    /// The authored light, which the reach decomposition reads.
    pub(crate) authored: &'a MapLight,
    /// Its soft-visibility seed axis in its section's bake.
    pub(crate) seed_axis: u64,
    pub(crate) early_out: ProbeEarlyOut,
    /// Its section's affinity reach policy.
    pub(crate) policy: AffinityReachPolicy,
}

/// A mover the light may meet, posed at each terminus its reach touches.
#[derive(Debug, Clone)]
pub(crate) struct LightMover {
    pub(crate) mover_id: u32,
    /// By [`Terminus::index`]; `None` where the light's reach misses the pose.
    pub(crate) poses: [Option<PosedOccluder>; 2],
}

/// The movers `light` may meet, ascending by id: every analysed mover with a
/// pose inside the ball its sample segments sweep. A probe the light lights
/// lies within its falloff range of the origin, and a sample target within
/// its emitter radius, so every segment lies in the larger ball; a sun's
/// segments reach every pose.
pub(crate) fn light_movers(light: &MapLight, poses: &MoverPoses) -> Vec<LightMover> {
    let reach = match light.light_type {
        LightType::Directional => None,
        LightType::Point | LightType::Spot => Some(
            f64::from(light.falloff_range.max(1.0e-4).max(emitter_radius(light)))
                + BOX_SLACK_M
                + f64::from(RAY_EPSILON),
        ),
    };
    poses
        .analysed
        .iter()
        .filter_map(|mover| {
            let poses = Terminus::BOTH.map(|terminus| {
                let pose = mover.pose(terminus);
                let reached = pose.bounds.is_valid()
                    && reach.is_none_or(|radius| {
                        sphere_touches_box(light.origin, radius, &pose.bounds)
                    });
                reached.then(|| PosedOccluder::new(pose.clone(), INSIDE_BRUSH_RULE))
            });
            poses.iter().any(Option::is_some).then_some(LightMover {
                mover_id: mover.mover_id,
                poses,
            })
        })
        .collect()
}

/// One mover's gate on one light at one affinity cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectEntry {
    pub(crate) cell: u32,
    pub(crate) mover_id: u32,
    /// `q₀, q₁` per valid probe of the cell, x-fastest; `q = byte / 255`.
    pub(crate) visibility: Vec<u8>,
    /// The light's own term T per valid probe, x-fastest, in id 41's compact
    /// RGB f16 tile: what the section carries where no per-probe id-41 or
    /// id-45 entry holds it.
    pub(crate) tiles: Vec<u16>,
}

/// One light's entries, ascending by (cell, mover).
#[derive(Debug, Clone)]
pub(crate) struct LightEntries {
    pub(crate) namespace: MoverLightNamespace,
    pub(crate) light_index: u32,
    pub(crate) entries: Vec<DirectEntry>,
}

/// The direct half of a build: every gated light's entries.
#[derive(Debug, Clone, Default)]
pub(crate) struct MoverDirectBake {
    pub(crate) lights: Vec<LightEntries>,
}

impl MoverDirectBake {
    pub(crate) fn entry_count(&self) -> usize {
        self.lights.iter().map(|light| light.entries.len()).sum()
    }
}

/// The SH probe grid in affinity cells: each cell's valid probes as the
/// grid marks them, which is id 34's validity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProbeCells {
    pub(crate) dims: [u32; 3],
    pub(crate) affinity_dims: [u32; 3],
    /// Bit `local` (x-fastest within the cell) set for each valid probe.
    pub(crate) valid_masks: Vec<u64>,
}

impl ProbeCells {
    pub(crate) fn new(dims: [u32; 3], validity: impl Fn(usize) -> bool) -> Self {
        let affinity_dims = dims.map(|dim| dim.div_ceil(AFFINITY_FACTOR));
        let cells = affinity_dims.iter().map(|&dim| dim as usize).product();
        let valid_masks = (0..cells)
            .map(|cell| {
                cell_probes(dims, affinity_dims, cell)
                    .filter(|&(_, probe)| validity(probe))
                    .fold(0u64, |mask, (local, _)| mask | 1 << local)
            })
            .collect();
        Self {
            dims,
            affinity_dims,
            valid_masks,
        }
    }

    pub(crate) fn of_layout(layout: &ProbeGridLayout) -> Self {
        Self::new(layout.dims, |probe| layout.validity[probe] != 0)
    }

    pub(crate) fn cell_count(&self) -> usize {
        self.valid_masks.len()
    }

    /// The dense index of each valid probe in `cell`, x-fastest.
    pub(crate) fn valid_probes(&self, cell: usize) -> Vec<usize> {
        let mask = self.valid_masks[cell];
        cell_probes(self.dims, self.affinity_dims, cell)
            .filter(|&(local, _)| mask & (1 << local) != 0)
            .map(|(_, probe)| probe)
            .collect()
    }
}

/// `(local, dense probe index)` of every in-grid probe of `cell`, x-fastest.
fn cell_probes(
    dims: [u32; 3],
    affinity_dims: [u32; 3],
    cell: usize,
) -> impl Iterator<Item = (usize, usize)> {
    let factor = AFFINITY_FACTOR as usize;
    let [ax, ay, _] = affinity_dims.map(|dim| dim as usize);
    let [nx, ny, nz] = dims.map(|dim| dim as usize);
    let origin = [
        cell % ax * factor,
        cell / ax % ay * factor,
        cell / (ax * ay) * factor,
    ];
    (0..factor * factor * factor).filter_map(move |local| {
        let (lx, ly, lz) = local_xyz(local);
        let [x, y, z] = [origin[0] + lx, origin[1] + ly, origin[2] + lz];
        (x < nx && y < ny && z < nz).then_some((local, x + y * nx + z * nx * ny))
    })
}

/// What the direct bake traces: the static world id 35 traces, the probe
/// grid, and the reach decomposition's inputs.
pub(crate) struct DirectScene<'a> {
    pub(crate) ctx: RaytracingCtx<'a>,
    pub(crate) layout: &'a ProbeGridLayout,
    pub(crate) cells: &'a ProbeCells,
    pub(crate) reach: AffinityReachInputs<'a>,
}

/// Bake every light's entries, from the per-light memo where it holds them.
/// Output follows `lights`' order, holding only lights with an entry, so the
/// bytes never depend on the worker count.
pub(crate) fn bake_direct(
    poses: &MoverPoses,
    lights: &[ChannelLight<'_>],
    scene: &DirectScene<'_>,
    cache: Option<&StageCache>,
    control: &BakeControl,
) -> anyhow::Result<MoverDirectBake> {
    let movers: Vec<Vec<LightMover>> = lights
        .iter()
        .map(|light| light_movers(&light.baked, poses))
        .collect();
    let mut baked: Vec<Option<Vec<DirectEntry>>> = vec![None; lights.len()];
    let memo = cache.map(|cache| DirectMemo::new(cache, MemoPrefix::new(scene)));
    let mut keys: Vec<Option<CacheKey>> = (0..lights.len()).map(|_| None).collect();
    let (mut probed, mut hits) = (0, 0);
    for (slot, light) in lights.iter().enumerate() {
        if movers[slot].is_empty() {
            baked[slot] = Some(Vec::new());
            continue;
        }
        let Some(memo) = memo.as_ref() else {
            continue;
        };
        let key = memo.key(light, &movers[slot]);
        probed += 1;
        if let Some(entries) = memo.get(&key, scene.cells, &movers[slot]) {
            baked[slot] = Some(entries);
            hits += 1;
        } else {
            keys[slot] = Some(key);
        }
    }
    if let Some(memo) = memo.as_ref() {
        memo.log_outcome(probed, hits);
    }

    let missing: Vec<usize> = (0..lights.len())
        .filter(|&slot| baked[slot].is_none())
        .collect();
    let reach = decompose_affinity_for_lights_with_policies(
        &scene.reach,
        &missing
            .iter()
            .map(|&slot| lights[slot].authored)
            .collect::<Vec<_>>(),
        &missing
            .iter()
            .map(|&slot| lights[slot].policy)
            .collect::<Vec<_>>(),
    );
    anyhow::ensure!(
        missing.is_empty() || reach.affinity_dims == scene.cells.affinity_dims,
        "mover probe channel: reach grid {:?} disagrees with the probe grid's {:?}",
        reach.affinity_dims,
        scene.cells.affinity_dims,
    );
    // One work item per (light, reached cell): a sun's whole-map reach
    // spreads across every worker, and the collect keeps item order.
    let items: Vec<(usize, u32)> = missing
        .iter()
        .zip(&reach.per_light_cells)
        .flat_map(|(&slot, cells)| cells.iter().map(move |&cell| (slot, cell)))
        .collect();
    control.publish_total(items.len());
    let cell_entries: Vec<Vec<DirectEntry>> = items
        .par_iter()
        .map(|&(slot, cell)| {
            let _permit = control.governor().enter();
            let entries = bake_cell(&lights[slot], &movers[slot], cell, scene);
            control.advance(1);
            entries
        })
        .collect();
    for slot in &missing {
        baked[*slot] = Some(Vec::new());
    }
    for (&(slot, _), entries) in items.iter().zip(cell_entries) {
        baked[slot]
            .as_mut()
            .expect("every missing light was just reset")
            .extend(entries);
    }
    // The memo and the section both read entries by (cell, mover), whatever
    // order the reach decomposition lists cells in.
    for slot in &missing {
        if let Some(entries) = baked[*slot].as_mut() {
            entries.sort_by_key(|entry| (entry.cell, entry.mover_id));
            entries.dedup_by_key(|entry| (entry.cell, entry.mover_id));
        }
    }
    if let Some(memo) = memo.as_ref() {
        for (slot, key) in keys.iter().enumerate() {
            if let (Some(key), Some(entries)) = (key, baked[slot].as_ref()) {
                memo.put(key, entries);
            }
        }
    }

    Ok(MoverDirectBake {
        lights: lights
            .iter()
            .zip(baked)
            .filter_map(|(light, entries)| {
                let entries = entries.expect("every light baked or hit");
                (!entries.is_empty()).then_some(LightEntries {
                    namespace: light.namespace,
                    light_index: light.light_index,
                    entries,
                })
            })
            .collect(),
    })
}

/// Static-trace results for one probe and light, by sample target. The
/// sample set is fixed by the probe, light and seed, so every mover and
/// terminus at the probe reuses one static trace per sample.
#[derive(Default)]
struct StaticSamples(RefCell<Vec<(Vec3, bool)>>);

impl StaticSamples {
    fn clear(&self, ctx: &RaytracingCtx<'_>, from: Vec3, to: Vec3) -> bool {
        if let Some(&(_, clear)) = self.0.borrow().iter().find(|(target, _)| *target == to) {
            return clear;
        }
        let clear = segment_clear(ctx, from, to);
        self.0.borrow_mut().push((to, clear));
        clear
    }
}

/// `light`'s entries at `cell`: one per mover whose posed brushes meet a
/// static-clear sample at some valid probe. A probe where the light's term
/// is zero keeps q = 1, as does one no pose can meet.
pub(crate) fn bake_cell(
    light: &ChannelLight<'_>,
    movers: &[LightMover],
    cell: u32,
    scene: &DirectScene<'_>,
) -> Vec<DirectEntry> {
    let probes = scene.cells.valid_probes(cell as usize);
    if probes.is_empty() || movers.is_empty() {
        return Vec::new();
    }
    let layout = scene.layout;
    let mut visibility =
        vec![vec![u8::MAX; probes.len() * MOVER_VISIBILITY_BYTES_PER_PROBE]; movers.len()];
    let mut met = vec![false; movers.len()];
    for (slot, &probe) in probes.iter().enumerate() {
        let point = vec3_from(layout.probe_positions[probe]);
        let Some((radiance, l)) = incident_radiance_at_point(&light.baked, point) else {
            continue;
        };
        if radiance == Vec3::ZERO {
            continue;
        }
        let seed = soft_visibility_seed(probe as u64, 0, light.seed_axis);
        let statics = StaticSamples::default();
        for (mover, gate) in movers.iter().enumerate() {
            for terminus in Terminus::BOTH {
                let Some(occluder) = &gate.poses[terminus.index()] else {
                    continue;
                };
                if !occluder.may_meet(point, &light.baked) {
                    continue;
                }
                let joint = joint_soft_visibility(
                    point,
                    l,
                    &light.baked,
                    seed,
                    DEFAULT_AREA_SAMPLE_COUNT,
                    light.early_out,
                    |from, to| statics.clear(&scene.ctx, from, to),
                    |from, to| occluder.blocks(point, from, to),
                );
                if joint.meets_mover() {
                    visibility[mover][slot * MOVER_VISIBILITY_BYTES_PER_PROBE + terminus.index()] =
                        joint.surviving_byte();
                    met[mover] = true;
                }
            }
        }
    }
    if !met.contains(&true) {
        return Vec::new();
    }
    let tiles: Vec<u16> = probes
        .iter()
        .flat_map(|&probe| own_tile(light, &scene.ctx, layout, probe))
        .collect();
    movers
        .iter()
        .zip(visibility)
        .zip(met)
        .filter(|(_, met)| *met)
        .map(|((gate, visibility), _)| DirectEntry {
            cell,
            mover_id: gate.mover_id,
            visibility,
            tiles: tiles.clone(),
        })
        .collect()
}

/// The light's term at `probe` as its section bakes it alone, in id 41's
/// compact tile: the same lift, seed, early-out and packing.
fn own_tile(
    light: &ChannelLight<'_>,
    ctx: &RaytracingCtx<'_>,
    layout: &ProbeGridLayout,
    probe: usize,
) -> Vec<u16> {
    let coefficients = bake_probe_direct_rgb_posed(
        ctx,
        vec3_from(layout.probe_positions[probe]),
        &[&light.baked],
        &[light.seed_axis],
        &[light.early_out],
        probe as u64,
        None,
    );
    let tile: Vec<u16> = pack_octahedral_irradiance_tile(
        &coefficients,
        true,
        DEFAULT_IRRADIANCE_TILE_DIMENSION,
        DEFAULT_IRRADIANCE_TILE_BORDER,
    )
    .into_iter()
    .flat_map(|texel| {
        let rgba = texel.rgba;
        [rgba[0], rgba[1], rgba[2]]
    })
    .collect();
    debug_assert_eq!(DELTA_TILE_TEXEL_F16_COUNT, 3);
    debug_assert_eq!(tile.len(), MOVER_IRRADIANCE_TILE_F16_COUNT);
    tile
}
