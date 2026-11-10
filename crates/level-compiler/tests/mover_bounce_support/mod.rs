// Shared harness for the mover probe channel's bounce-half suites: maps built
// into a temporary content root, reference variants with a mover made a
// static brush, and the SH base read back per probe.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

#![allow(dead_code)] // Each suite uses a different subset.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use postretro_level_compiler::bc6h::{decode_bc6h_block, encode_bc6h_rgb_from_f32_rgba};
use postretro_level_format::indirect_display::{DEFAULT_INDIRECT_SCALE, soft_knee_tonemap};
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
use postretro_level_format::mover_probe_channel::{
    MOVER_IRRADIANCE_TILE_F16_COUNT, MoverProbeChannelSection,
};
use postretro_level_format::octahedral::irradiance_array_tile_location;
use postretro_level_format::probe_depth_tiles::{
    ProbeDepthTilesSection, f16_bits_to_f32 as f16_to_f32,
};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::{SectionId, read_container, read_section_data};

use crate::prl_build_support::{REACH_VERIFY_ENV, prl_build};

/// Quake map units to engine meters.
pub const UNIT: f64 = 0.0254;

pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|crates| crates.parent())
        .expect("workspace root")
        .to_path_buf()
}

pub fn dev_map(name: &str) -> String {
    std::fs::read_to_string(workspace_root().join("content/dev/maps").join(name))
        .unwrap_or_else(|error| panic!("read {name}: {error}"))
}

/// Quake coordinates of an engine point: engine x is Quake −y, engine y is
/// Quake z, engine z is Quake −x.
pub fn quake(engine: [f64; 3]) -> [f64; 3] {
    [-engine[2] / UNIT, -engine[0] / UNIT, engine[1] / UNIT]
}

/// One build's output, kept with its directory.
pub struct Built {
    pub dir: tempfile::TempDir,
    pub prl: PathBuf,
    pub logs: String,
}

/// How a build reads the stage cache.
#[derive(Clone, Copy)]
pub enum Cache<'a> {
    Cold,
    /// Fill or read `dir`, reach verify off so the lightmap memos hit too.
    Warm(&'a Path),
}

/// Build `map_text` as `<name>.map` in a fresh content root whose textures
/// are the dev tree's (where the platform links them), with `flags`.
pub fn build(map_text: &str, name: &str, cache: Cache<'_>, flags: &[&str]) -> Built {
    try_build(map_text, name, cache, flags)
        .unwrap_or_else(|logs| panic!("prl-build failed\n{logs}"))
}

/// [`build`], handing back the logs of a failed build.
pub fn try_build(
    map_text: &str,
    name: &str,
    cache: Cache<'_>,
    flags: &[&str],
) -> Result<Built, String> {
    let dir = tempfile::tempdir().expect("create build directory");
    let maps = dir.path().join("maps");
    std::fs::create_dir_all(&maps).expect("create maps directory");
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        workspace_root().join("content/dev/textures"),
        dir.path().join("textures"),
    )
    .expect("link the dev textures");
    let input = maps.join(format!("{name}.map"));
    std::fs::write(&input, map_text).expect("write map");
    let prl = dir.path().join(format!("{name}.prl"));
    let mut command: Command = prl_build();
    command
        .env("RUST_LOG", "info")
        .arg(&input)
        .arg("-o")
        .arg(&prl)
        .args(["--no-tui", "--jobs", "6"])
        .arg("--baked-root")
        .arg(dir.path().join("baked"))
        .args(flags);
    match cache {
        Cache::Cold => {
            command.arg("--no-cache");
        }
        Cache::Warm(cache) => {
            command
                .env(REACH_VERIFY_ENV, "0")
                .arg("--cache-dir")
                .arg(cache);
        }
    }
    let output = command.output().expect("spawn prl-build");
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        Ok(Built { dir, prl, logs })
    } else {
        Err(logs)
    }
}

pub fn section(prl: &Path, id: SectionId) -> Option<Vec<u8>> {
    let bytes = std::fs::read(prl).expect("read the packed PRL");
    let mut cursor = Cursor::new(bytes.as_slice());
    let meta = read_container(&mut cursor).expect("read the container");
    read_section_data(&mut cursor, &meta, id as u32).expect("read the section")
}

pub fn channel(prl: &Path) -> Option<MoverProbeChannelSection> {
    section(prl, SectionId::MoverProbeChannel)
        .map(|bytes| MoverProbeChannelSection::from_bytes(&bytes).expect("id 57 decodes"))
}

pub fn depth_tiles(prl: &Path) -> ProbeDepthTilesSection {
    section(prl, SectionId::ProbeDepthTiles)
        .map(|bytes| ProbeDepthTilesSection::from_bytes(&bytes).expect("id 59 decodes"))
        .unwrap_or_default()
}

/// `map` with `key` set on its worldspawn.
pub fn with_worldspawn_key(map: &str, key: &str, value: &str) -> String {
    let marker = "\"classname\" \"worldspawn\"";
    let at = map.find(marker).expect("a worldspawn") + marker.len();
    format!("{}\n\"{key}\" \"{value}\"{}", &map[..at], &map[at..])
}

/// Top-level `{ … }` entity blocks of a map, comments dropped.
fn entities(map: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for line in map.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            continue;
        }
        if trimmed == "{" {
            depth += 1;
        }
        if depth > 0 {
            current.push_str(line);
            current.push('\n');
        }
        if trimmed == "}" {
            depth -= 1;
            if depth == 0 {
                out.push(std::mem::take(&mut current));
            }
        }
    }
    out
}

fn key_of(entity: &str, key: &str) -> Option<String> {
    let prefix = format!("\"{key}\" \"");
    entity.lines().find_map(|line| {
        line.trim()
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix('"'))
            .map(str::to_string)
    })
}

/// The brush blocks of one entity block.
fn brushes(entity: &str) -> Vec<String> {
    let inner = entity
        .trim()
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .expect("an entity block");
    entities(inner)
}

/// One brush translated by `offset` (Quake units): every plane point moves.
fn translate_brush(brush: &str, offset: [f64; 3]) -> String {
    brush
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if !trimmed.starts_with('(') {
                return line.to_string();
            }
            let mut out = String::new();
            let mut rest = trimmed;
            for _ in 0..3 {
                let open = rest.find('(').expect("a plane point");
                let close = rest.find(')').expect("a closed plane point");
                let point: Vec<f64> = rest[open + 1..close]
                    .split_whitespace()
                    .map(|value| value.parse().expect("a coordinate"))
                    .collect();
                out.push_str(&format!(
                    "( {} {} {} ) ",
                    point[0] + offset[0],
                    point[1] + offset[1],
                    point[2] + offset[2]
                ));
                rest = &rest[close + 1..];
            }
            out.push_str(rest.trim());
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `map` with kinematic mover `name` made static at its waypoint `waypoint`:
/// its brushes join the worldspawn moved by that waypoint's offset from the
/// path's first waypoint, and the mover, its waypoints and every entity it
/// carries leave the map.
pub fn staticize(map: &str, name: &str, waypoint: &str) -> String {
    let blocks = entities(map);
    let mover = blocks
        .iter()
        .find(|block| {
            key_of(block, "classname").as_deref() == Some("kinematic_mover")
                && key_of(block, "name").as_deref() == Some(name)
        })
        .unwrap_or_else(|| panic!("no kinematic_mover `{name}`"));
    let first = key_of(mover, "path").expect("a mover path");
    let origin = |waypoint: &str| -> [f64; 3] {
        let block = blocks
            .iter()
            .find(|block| key_of(block, "name").as_deref() == Some(waypoint))
            .unwrap_or_else(|| panic!("no waypoint `{waypoint}`"));
        let values: Vec<f64> = key_of(block, "origin")
            .expect("a waypoint origin")
            .split_whitespace()
            .map(|value| value.parse().expect("a coordinate"))
            .collect();
        [values[0], values[1], values[2]]
    };
    let (from, to) = (origin(&first), origin(waypoint));
    let offset = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let moved: Vec<String> = brushes(mover)
        .iter()
        .map(|brush| translate_brush(brush, offset))
        .collect();
    let mut waypoints = vec![first];
    while let Some(next) = blocks
        .iter()
        .find(|block| key_of(block, "name").as_deref() == waypoints.last().map(String::as_str))
        .and_then(|block| key_of(block, "next"))
        .filter(|next| !next.is_empty() && !waypoints.contains(next))
    {
        waypoints.push(next);
    }
    let mut out = Vec::new();
    for block in &blocks {
        let classname = key_of(block, "classname");
        if classname.as_deref() == Some("worldspawn") {
            let body = block.trim().strip_suffix('}').expect("a closed worldspawn");
            out.push(format!("{body}{}\n}}", moved.join("\n")));
            continue;
        }
        let is_mover = std::ptr::eq(block, mover);
        let is_waypoint = key_of(block, "name").is_some_and(|n| waypoints.contains(&n))
            && classname.as_deref() == Some("kinematic_waypoint");
        let carried = key_of(block, "carrier").as_deref() == Some(name);
        if !is_mover && !is_waypoint && !carried {
            out.push(block.clone());
        }
    }
    out.join("\n") + "\n"
}

/// An RGB tile: 36 texels, border included.
pub type Tile = Vec<[f32; 3]>;

/// Id 34 read back per probe from a build at uniform L0 storage
/// (`_sh_coarsen "0"`), BC6H or uncompressed.
pub struct Volume {
    pub section: OctahedralShVolumeSection,
    /// Stored slot per dense probe, valid probes only.
    slots: Vec<Option<usize>>,
    /// Decoded layers, RGB f32 row-major.
    layers: Vec<Vec<[f32; 3]>>,
}

impl Volume {
    pub fn read(prl: &Path) -> Self {
        let section = OctahedralShVolumeSection::from_bytes(
            &section(prl, SectionId::OctahedralShVolume).expect("id 34"),
        )
        .expect("id 34 decodes");
        let [nx, ny, nz] = section.grid_dimensions.map(|dim| dim as usize);
        let [ax, ay, az] = section.grid_dimensions.map(|dim| dim.div_ceil(4) as usize);
        let mut slots = vec![None; nx * ny * nz];
        let mut slot = 0;
        for bz in 0..az {
            for by in 0..ay {
                for bx in 0..ax {
                    for local in 0..64 {
                        let (lx, ly, lz) = (local % 4, local / 4 % 4, local / 16);
                        let (x, y, z) = (bx * 4 + lx, by * 4 + ly, bz * 4 + lz);
                        if x >= nx || y >= ny || z >= nz {
                            continue;
                        }
                        let probe = x + y * nx + z * nx * ny;
                        if section.probes[probe].validity != 0 {
                            slots[probe] = Some(slot);
                            slot += 1;
                        }
                    }
                }
            }
        }
        let [width, height] = section.atlas_dimensions.map(|dim| dim as usize);
        let layers = (0..section.layer_count as usize)
            .map(|layer| {
                if section.irradiance_format == IRRADIANCE_FORMAT_RGBA16F {
                    let stride = width * height * 8;
                    section.compact_atlas[layer * stride..(layer + 1) * stride]
                        .as_chunks::<8>()
                        .0
                        .iter()
                        .map(|texel| {
                            std::array::from_fn(|c| {
                                f16_to_f32(u16::from_le_bytes([texel[2 * c], texel[2 * c + 1]]))
                            })
                        })
                        .collect()
                } else {
                    let (bw, bh) = (width.div_ceil(4), height.div_ceil(4));
                    let stride = bw * bh * 16;
                    let bytes = &section.compact_atlas[layer * stride..(layer + 1) * stride];
                    let mut out = vec![[0.0f32; 3]; width * height];
                    for (index, block) in bytes.as_chunks::<16>().0.iter().enumerate() {
                        let decoded = decode_bc6h_block(block).expect("a BC6H block");
                        for (texel, rgb) in decoded.iter().enumerate() {
                            let (x, y) = (index % bw * 4 + texel % 4, index / bw * 4 + texel / 4);
                            if x < width && y < height {
                                out[y * width + x] = *rgb;
                            }
                        }
                    }
                    out
                }
            })
            .collect();
        Self {
            section,
            slots,
            layers,
        }
    }

    pub fn is_valid(&self, probe: usize) -> bool {
        self.slots[probe].is_some()
    }

    pub fn probe_count(&self) -> usize {
        self.slots.len()
    }

    /// Probe `probe`'s position in Quake units.
    pub fn quake_position(&self, probe: usize) -> [f64; 3] {
        let [nx, ny, _] = self.section.grid_dimensions.map(|dim| dim as usize);
        let grid = [probe % nx, probe / nx % ny, probe / (nx * ny)];
        quake(std::array::from_fn(|axis| {
            f64::from(self.section.grid_origin[axis])
                + grid[axis] as f64 * f64::from(self.section.cell_size[axis])
        }))
    }

    /// Valid probe `probe`'s stored tile.
    pub fn tile(&self, probe: usize) -> Tile {
        let slot = self.slots[probe].expect("a valid probe");
        let [layer, tx, ty] = irradiance_array_tile_location(
            slot,
            self.section.tiles_per_layer,
            self.section.atlas_tiles_per_row,
        );
        let dim = self.section.tile_dimension as usize;
        let width = self.section.atlas_dimensions[0] as usize;
        let layer = &self.layers[layer as usize];
        (0..dim * dim)
            .map(|texel| {
                let (x, y) = (
                    tx as usize * dim + texel % dim,
                    ty as usize * dim + texel / dim,
                );
                layer[y * width + x]
            })
            .collect()
    }
}

/// An id-57 compact RGB tile as f32.
pub fn rgb_tile(halves: &[u16]) -> Tile {
    assert_eq!(halves.len(), MOVER_IRRADIANCE_TILE_F16_COUNT);
    halves
        .as_chunks::<3>()
        .0
        .iter()
        .map(|rgb| rgb.map(f16_to_f32))
        .collect()
}

/// Each bounce entry of `mover`: per affected dense probe of `volume`'s
/// grid, `[S^b₀, S^b₁]`, each `None` at a terminus that stores no state and
/// composes id 34 there.
pub fn bounce_states_in(
    channel: &MoverProbeChannelSection,
    volume: &Volume,
    mover: u32,
) -> std::collections::BTreeMap<usize, [Option<Tile>; 2]> {
    let [nx, ny, _] = volume.section.grid_dimensions.map(|dim| dim as usize);
    let [ax, ay, _] = channel.affinity_dims.map(|dim| dim as usize);
    let mut out = std::collections::BTreeMap::new();
    for view in channel.bounce_entry_views() {
        if view.entry.mover_id != mover {
            continue;
        }
        let cell = view.cell as usize;
        let (cx, cy, cz) = (cell % ax, cell / ax % ay, cell / (ax * ay));
        for local in 0..64u32 {
            let Some(tiles) = view.probe_tiles(local) else {
                continue;
            };
            let local = local as usize;
            let (x, y, z) = (
                cx * 4 + local % 4,
                cy * 4 + local / 4 % 4,
                cz * 4 + local / 16,
            );
            out.insert(
                x + y * nx + z * nx * ny,
                tiles.map(|tile| tile.map(|halves| rgb_tile(&halves.to_vec()))),
            );
        }
    }
    out
}

/// The largest absolute difference over two tiles' texels and channels.
pub fn max_abs(a: &[[f32; 3]], b: &[[f32; 3]]) -> f32 {
    a.iter()
        .zip(b)
        .flat_map(|(a, b)| (0..3).map(move |c| (a[c] - b[c]).abs()))
        .fold(0.0, f32::max)
}

/// A tile's BC6H round-trip error encoded alone: the affected-probe bar.
pub fn round_trip_error(tile: &[[f32; 3]]) -> f32 {
    const DIM: usize = 6;
    const PADDED: usize = 8;
    let mut rgba = vec![0.0f32; PADDED * PADDED * 4];
    for y in 0..PADDED {
        for x in 0..PADDED {
            let texel = tile[y.min(DIM - 1) * DIM + x.min(DIM - 1)];
            rgba[(y * PADDED + x) * 4..][..3].copy_from_slice(&texel);
            rgba[(y * PADDED + x) * 4 + 3] = 1.0;
        }
    }
    let blocks = encode_bc6h_rgb_from_f32_rgba(&rgba, PADDED as u32, PADDED as u32);
    let mut error = 0.0f32;
    for (index, block) in blocks.as_chunks::<16>().0.iter().enumerate() {
        let decoded = decode_bc6h_block(block).expect("a BC6H block");
        for (local, rgb) in decoded.iter().enumerate() {
            let (x, y) = (index % 2 * 4 + local % 4, index / 2 * 4 + local / 4);
            if x < DIM && y < DIM {
                for c in 0..3 {
                    error = error.max((rgb[c] - tile[y * DIM + x][c]).abs());
                }
            }
        }
    }
    error
}

/// The bounce floor's measure: the largest change, in 8-bit sRGB levels, of
/// a mid-grey receiver lit by one texel through the renderer's indirect
/// scale and soft-knee tonemap.
pub fn display_levels(a: &[[f32; 3]], b: &[[f32; 3]]) -> f32 {
    let encode = |linear: f32| {
        let x = linear.clamp(0.0, 1.0);
        if x <= 0.003_130_8 {
            12.92 * x
        } else {
            1.055 * x.powf(1.0 / 2.4) - 0.055
        }
    };
    let shown = |rgb: [f32; 3]| {
        soft_knee_tonemap(rgb.map(|c| 0.5 * DEFAULT_INDIRECT_SCALE * c)).map(|c| 255.0 * encode(c))
    };
    a.iter()
        .zip(b)
        .flat_map(|(a, b)| {
            let (a, b) = (shown(*a), shown(*b));
            (0..3).map(move |c| (a[c] - b[c]).abs())
        })
        .fold(0.0, f32::max)
}

/// Whether two f16 depth moments agree within f16 rounding: one unit in the
/// last place of the larger.
pub fn f16_close(a: u16, b: u16) -> bool {
    let (a, b) = (f16_to_f32(a), f16_to_f32(b));
    let ulp = (a.abs().max(b.abs()) * 2.0f32.powi(-10)).max(2.0f32.powi(-24));
    (a - b).abs() <= ulp
}
