// TEMPORARY dev-only driver for the mover probe channel's bounce slice: the
// posed bounce and depth re-bake behind `POSTRETRO_DEV_POSE_MOVER`, a
// measurement mode over every mover, the slice's report, and a dump for the
// reference checks. Plan task 12 deletes this file and its call sites.
// See: context/plans/in-progress/mover-light-flow--probe-channel/plan.md (task 2)

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, bail};
use postretro_level_format::probe_depth_tiles::{
    MAX_PROBE_DEPTH_TILES, PROBE_DEPTH_TILE_TEXELS, f16_bits_to_f32,
};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::texture_names::TextureNamesSection;

use super::domain::{compact_tile, round_trip_error, write_compact_rgb};
use super::{
    INSIDE_BRUSH_RULE, MoverBounceRequest, PosedBounce, PosedScene, PosedSurfaces, dev_pose,
};
use crate::map_data::{MapKinematicMover, MapKinematicWaypoint};
use crate::mover_light_gate::{MoverPoses, Terminus};
use crate::sh_bake::ProbeDepthTiles;

/// `all`, or one mover's name: re-bake those movers' bounce and depth at
/// both termini and report them, leaving the PRL unchanged. M1's cost run.
pub(crate) const MEASURE_VAR: &str = "POSTRETRO_DEV_MOVER_BOUNCE_MEASURE";
/// A path: write the dense id-34 tiles, their round-trip errors, id 59 and
/// (with the pose switch) every valid probe's posed tile and depth.
pub(crate) const DUMP_VAR: &str = "POSTRETRO_DEV_MOVER_BOUNCE_DUMP";

/// Probe-channel tiles: 6×6 RGB f16 (bounce) and 10×10 RG f16 (depth).
const BOUNCE_TILE_BYTES: u64 = 216;
const DEPTH_TILE_BYTES: u64 = PROBE_DEPTH_TILE_TEXELS as u64 * 4;
/// The bounce index's per-cell entry: mover id, reserved, probe mask.
const BOUNCE_ENTRY_BYTES: u64 = 16;
/// Probes per affinity cell edge.
const AFFINITY: u32 = 4;

/// What the switches ask of this build.
pub(crate) struct DevBounce {
    substitute: bool,
    dump: Option<PathBuf>,
    pub(crate) request: MoverBounceRequest,
}

impl DevBounce {
    pub(crate) fn active(&self) -> bool {
        self.substitute || self.dump.is_some() || !self.request.is_empty()
    }
}

/// Read the switches. With none set, the request is empty and the build is
/// today's.
pub(crate) fn plan(
    movers: &[MapKinematicMover],
    waypoints: &[MapKinematicWaypoint],
    texture_names: &TextureNamesSection,
    uncompressed: bool,
) -> anyhow::Result<DevBounce> {
    let pose = dev_pose::parse()?;
    let measure = std::env::var(MEASURE_VAR).ok();
    let dump = std::env::var_os(DUMP_VAR).map(PathBuf::from);
    if pose.is_some() && measure.is_some() {
        bail!("{} and {MEASURE_VAR} are exclusive", dev_pose::ENV_VAR);
    }
    let poses = MoverPoses::resolve(movers, waypoints);
    let mut scenes = Vec::new();
    if let Some(pose) = &pose {
        let mover = movers
            .iter()
            .find(|mover| mover.name == pose.name)
            .with_context(|| format!("no kinematic_mover named `{}`", pose.name))?;
        let analysed = poses
            .mover(mover.mover_id)
            .with_context(|| format!("mover `{}` has no analysable termini", pose.name))?;
        scenes.push(PosedScene::new(
            mover,
            analysed,
            pose.terminus,
            texture_names,
            pose.rule,
            pose.surfaces,
        )?);
    }
    if let Some(which) = &measure {
        for analysed in &poses.analysed {
            let mover = movers
                .iter()
                .find(|mover| mover.mover_id == analysed.mover_id)
                .expect("an analysed mover is a map mover");
            if which != "all" && mover.name != *which {
                continue;
            }
            for terminus in Terminus::BOTH {
                scenes.push(PosedScene::new(
                    mover,
                    analysed,
                    terminus,
                    texture_names,
                    INSIDE_BRUSH_RULE,
                    PosedSurfaces::Patches,
                )?);
            }
        }
        if scenes.is_empty() {
            bail!("{MEASURE_VAR}={which}: no analysed mover matches");
        }
        log::warn!(
            "[MoverLight] DEV {MEASURE_VAR}={which}: re-baking bounce and depth for {} poses; \
             results are reported and discarded",
            scenes.len()
        );
    }
    Ok(DevBounce {
        substitute: pose.is_some(),
        request: MoverBounceRequest {
            scenes,
            uncompressed,
            keep_every_tile: dump.is_some(),
        },
        dump,
    })
}

/// Report, dump, then (with the pose switch) put the posed state in the
/// PRL: S^b over id 34 at affected probes, closed-pose tiles into id 59.
pub(crate) fn finish(
    dev: &DevBounce,
    results: &[PosedBounce],
    section: &mut OctahedralShVolumeSection,
    tiles: &mut ProbeDepthTiles,
) -> anyhow::Result<()> {
    report(results, section, tiles);
    if let Some(path) = &dev.dump {
        write_dump(
            path,
            section,
            tiles,
            results.first(),
            dev.request.uncompressed,
        )
        .with_context(|| format!("{DUMP_VAR}: write {}", path.display()))?;
    }
    if dev.substitute
        && let Some(posed) = results.first()
    {
        for probe in posed.affected() {
            let tile = probe
                .tile
                .as_ref()
                .expect("an affected probe keeps its tile");
            write_compact_rgb(section, probe.slot as usize, tile);
        }
        let before = tiles.section.tile_count();
        let mut merged: BTreeMap<u32, &[[u16; 2]]> = tiles
            .section
            .probe_indices
            .iter()
            .zip(tiles.section.texels.chunks(PROBE_DEPTH_TILE_TEXELS))
            .map(|(&probe, tile)| (probe, tile))
            .collect();
        let mut replaced = 0usize;
        for (probe, tile) in posed.depth() {
            replaced += usize::from(merged.insert(probe, tile).is_some());
        }
        if merged.len() > MAX_PROBE_DEPTH_TILES as usize {
            bail!(
                "posed id 59 holds {} tiles, past the {MAX_PROBE_DEPTH_TILES}-tile range; \
                 the slice does not trim",
                merged.len()
            );
        }
        let probe_indices: Vec<u32> = merged.keys().copied().collect();
        let texels: Vec<[u16; 2]> = merged
            .values()
            .flat_map(|tile| tile.iter().copied())
            .collect();
        tiles.section.probe_indices = probe_indices;
        tiles.section.texels = texels;
        log::warn!(
            "[MoverLight] DEV posed state in the PRL: {} id-34 tiles replaced by S^b; id 59 \
             {before} -> {} tiles ({replaced} replaced, {} added)",
            posed.affected().count(),
            tiles.section.tile_count(),
            tiles.section.tile_count() - before,
        );
    }
    Ok(())
}

fn percentile(sorted: &[f32], p: f64) -> f32 {
    if sorted.is_empty() {
        return f32::NAN;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// The affinity cell of dense probe `probe`.
fn affinity_cell(probe: u32, dims: [u32; 3]) -> [u32; 3] {
    let (nx, ny) = (dims[0], dims[1]);
    let z = probe / (nx * ny);
    let y = (probe - z * nx * ny) / nx;
    let x = probe - z * nx * ny - y * nx;
    [x / AFFINITY, y / AFFINITY, z / AFFINITY]
}

fn report(results: &[PosedBounce], section: &OctahedralShVolumeSection, tiles: &ProbeDepthTiles) {
    let dims = section.grid_dimensions;
    for posed in results {
        let mut distances: Vec<f32> = posed.affected().map(|probe| probe.distance_m).collect();
        distances.sort_by(f32::total_cmp);
        let depth = posed.depth().count();
        let [one, later, gather] = posed.timings.map(|t| t.as_secs_f64());
        log::warn!(
            "[MoverLight] DEV bounce {} {}: {} posed patches, {} re-shaded (patch, light) pairs; \
             {} probes differ, {} affected past the bar; distance from pose bounds min {:.2} \
             median {:.2} p95 {:.2} max {:.2} m; {} probes' rays meet the pose (depth tiles); \
             bounce one {one:.2}s, later bounces {later:.2}s, gather {gather:.2}s",
            posed.label,
            posed.terminus.label(),
            posed.posed_patches,
            posed.reshaded_pairs,
            posed
                .probes
                .iter()
                .filter(|probe| probe.difference > 0.0)
                .count(),
            distances.len(),
            percentile(&distances, 0.0),
            percentile(&distances, 0.5),
            percentile(&distances, 0.95),
            percentile(&distances, 1.0),
            depth,
        );
        let mut farthest: Vec<_> = posed.affected().collect();
        farthest.sort_by(|a, b| b.distance_m.total_cmp(&a.distance_m));
        for probe in farthest.iter().take(5) {
            let (x, y, z) = (
                probe.probe % dims[0],
                probe.probe / dims[0] % dims[1],
                probe.probe / (dims[0] * dims[1]),
            );
            log::warn!(
                "[MoverLight] DEV   farthest affected: probe {} ({x}, {y}, {z}) at {:.2} m, \
                 |S^b - id34| {:.5} > bar {:.5}",
                probe.probe,
                probe.distance_m,
                probe.difference,
                probe.bar,
            );
        }
    }
    // Per mover, both termini: the section's bounce and depth blocks.
    let mut movers: BTreeMap<u32, (BTreeSet<u32>, BTreeSet<u32>)> = BTreeMap::new();
    for posed in results {
        let (affected, depth) = movers.entry(posed.mover_id).or_default();
        affected.extend(posed.affected().map(|probe| probe.probe));
        depth.extend(posed.depth().map(|(probe, _)| probe));
    }
    let (mut bounce_total, mut depth_total) = (0u64, 0u64);
    let mut live: BTreeSet<u32> = BTreeSet::new();
    let mut closed_pose = 0u64;
    for (mover, (affected, depth)) in &movers {
        let cells: BTreeSet<[u32; 3]> = affected.iter().map(|&p| affinity_cell(p, dims)).collect();
        let bounce =
            2 * BOUNCE_TILE_BYTES * affected.len() as u64 + BOUNCE_ENTRY_BYTES * cells.len() as u64;
        let depth_bytes = (2 * DEPTH_TILE_BYTES + 8) * depth.len() as u64;
        bounce_total += bounce;
        depth_total += depth_bytes;
        live.extend(depth.iter().copied());
        closed_pose += 2 * depth.len() as u64;
        log::warn!(
            "[MoverLight] DEV mover {mover}: bounce {} affected probes in {} cells, {bounce} B; \
             depth {} probes, {depth_bytes} B",
            affected.len(),
            cells.len(),
            depth.len(),
        );
    }
    let id59 = tiles.section.tile_count() as u64;
    let cap = u64::from(MAX_PROBE_DEPTH_TILES);
    let new_live = live
        .iter()
        .filter(|probe| tiles.section.probe_indices.binary_search(probe).is_err())
        .count();
    log::warn!(
        "[MoverLight] DEV totals: bounce block {bounce_total} B, depth block {depth_total} B; \
         id 59 {id59} tiles, live tiles {} ({new_live} on probes id 59 does not tile), \
         closed-pose tiles {closed_pose}; id 59 + live = {:.3}% of {cap}, + closed-pose = {:.3}%; \
         trim drops {} id-59 tiles",
        live.len(),
        100.0 * (id59 + live.len() as u64) as f64 / cap as f64,
        100.0 * (id59 + live.len() as u64 + closed_pose) as f64 / cap as f64,
        (id59 + live.len() as u64).saturating_sub(cap),
    );
}

/// Little-endian: `MPCD`, version 1, grid dims (u32 × 3), origin and cell
/// size (f32 × 3 each), the tile edge, the valid count, then per valid
/// probe ascending: index (u32), id-34 tile (RGB f32 per texel), its
/// round-trip error (f32), a posed flag (u8) and posed tile, a depth flag
/// (u8) and posed depth tile (RG f32 per texel, set where a ray meets the
/// pose); then id 59's count and per tile its probe (u32) and texels (RG
/// f32).
fn write_dump(
    path: &PathBuf,
    section: &OctahedralShVolumeSection,
    tiles: &ProbeDepthTiles,
    posed: Option<&PosedBounce>,
    uncompressed: bool,
) -> std::io::Result<()> {
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    let f32s = |out: &mut dyn Write, values: &mut dyn Iterator<Item = f32>| {
        for value in values {
            out.write_all(&value.to_le_bytes())?;
        }
        Ok::<(), std::io::Error>(())
    };
    out.write_all(b"MPCD")?;
    out.write_all(&1u32.to_le_bytes())?;
    for d in section.grid_dimensions {
        out.write_all(&d.to_le_bytes())?;
    }
    f32s(&mut out, &mut section.grid_origin.into_iter())?;
    f32s(&mut out, &mut section.cell_size.into_iter())?;
    out.write_all(&section.tile_dimension.to_le_bytes())?;
    let valid: Vec<usize> = section
        .probes
        .iter()
        .enumerate()
        .filter(|(_, probe)| probe.validity != 0)
        .map(|(index, _)| index)
        .collect();
    out.write_all(&(valid.len() as u32).to_le_bytes())?;
    let by_probe: BTreeMap<u32, &super::gather::PosedProbe> = posed
        .map(|posed| posed.probes.iter().map(|p| (p.probe, p)).collect())
        .unwrap_or_default();
    let dim = section.tile_dimension as usize;
    for (slot, &probe) in valid.iter().enumerate() {
        out.write_all(&(probe as u32).to_le_bytes())?;
        let base = compact_tile(section, slot);
        let rgb = |tile: &[postretro_level_format::sh_volume::OctahedralAtlasTexel]| {
            tile.iter()
                .flat_map(|texel| texel.rgba[..3].iter().map(|&b| f16_bits_to_f32(b)))
                .collect::<Vec<f32>>()
        };
        f32s(&mut out, &mut rgb(&base).into_iter())?;
        f32s(
            &mut out,
            &mut std::iter::once(round_trip_error(&base, dim, uncompressed)),
        )?;
        let entry = by_probe.get(&(probe as u32));
        match entry.and_then(|probe| probe.tile.as_ref()) {
            Some(tile) => {
                out.write_all(&[1])?;
                f32s(&mut out, &mut rgb(tile).into_iter())?;
            }
            None => out.write_all(&[0])?,
        }
        match entry.and_then(|probe| probe.depth.as_ref()) {
            Some(depth) => {
                out.write_all(&[1])?;
                f32s(
                    &mut out,
                    &mut depth.iter().flat_map(|t| t.map(f16_bits_to_f32)),
                )?;
            }
            None => out.write_all(&[0])?,
        }
    }
    out.write_all(&(tiles.section.tile_count() as u32).to_le_bytes())?;
    for (probe, tile) in tiles
        .section
        .probe_indices
        .iter()
        .zip(tiles.section.texels.chunks(PROBE_DEPTH_TILE_TEXELS))
    {
        out.write_all(&probe.to_le_bytes())?;
        f32s(
            &mut out,
            &mut tile.iter().flat_map(|t| t.map(f16_bits_to_f32)),
        )?;
    }
    out.flush()
}
