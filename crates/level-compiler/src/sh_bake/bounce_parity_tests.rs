//! On-demand measurement: the one-sample SH bounce against the full set on real maps.
//! See: context/lib/build_pipeline.md §PRL Compilation (Soft-shadow bake cost)
//! Cost: every map, campaign-test included, is baked three times (one-sample,
//! full set, reseeded full set).

use postretro_level_format::delta_sh_volumes::{
    DEFAULT_DELTA_PROBE_F16_STRIDE, DELTA_TILE_TEXEL_F16_COUNT, DeltaShVolumesSection,
    PROBES_PER_CELL,
};

use super::*;
use crate::buried_lights::BuriedLights;
use crate::delta_sh_bake::{DeltaBakeInputs, bake_delta_sh_volumes_with_visibility};
use crate::fixture_pipeline::load_fixture;

const DEFAULT_MAPS: &str = "soft_shadow_test,stress-warren-mini,campaign-test";
const TILE: usize = DEFAULT_IRRADIANCE_TILE_DIMENSION as usize;
const BORDER: usize = DEFAULT_IRRADIANCE_TILE_BORDER as usize;
/// Pass bound on the mean signed relative error over affected probes.
const MAX_MEAN_SIGNED_RELATIVE_ERROR: f64 = 0.01;
/// The control bake's seed salt; any value draws an unrelated rotation.
const CONTROL_SEED_SALT: u64 = 1;

/// RGB tiles (`TILE * TILE` texels each) for every probe of one bake.
type Tiles = Vec<Vec<[f32; 3]>>;

fn interior_values<'a>(tile: &'a [[f32; 3]]) -> impl Iterator<Item = f32> + 'a {
    (BORDER..TILE - BORDER)
        .flat_map(|y| (BORDER..TILE - BORDER).map(move |x| y * TILE + x))
        .flat_map(move |i| tile[i])
}

/// Per-probe RMS over the interior texels and RGB channels of `a - b`.
fn interior_rms(a: &[[f32; 3]], b: &[[f32; 3]]) -> f32 {
    let (sum, count) = interior_values(a).zip(interior_values(b)).fold(
        (0.0f64, 0usize),
        |(sum, count), (p, q)| {
            let d = f64::from(p - q);
            (sum + d * d, count + 1)
        },
    );
    (sum / count as f64).sqrt() as f32
}

/// `Σ (a - b) / Σ b` over the interior texels and RGB channels: the probe's
/// mean brightening (positive) or darkening (negative) relative to `b`.
fn interior_signed_relative(a: &[[f32; 3]], b: &[[f32; 3]]) -> f64 {
    let (difference, reference) = interior_values(a)
        .zip(interior_values(b))
        .fold((0.0f64, 0.0f64), |(d, r), (p, q)| {
            (d + f64::from(p - q), r + f64::from(q))
        });
    difference / reference
}

/// Per-probe RMS of the rounding an f16 tile carries, by the uniform rounding
/// model: `ulp / sqrt(12)` per nonzero value. The bake keeps no f32 tile to
/// measure against, so this is the expected f16 error, not a measured one.
fn interior_f16_rounding_rms(tile: &[[f32; 3]]) -> f32 {
    let (sum, count) = interior_values(tile).fold((0.0f64, 0usize), |(sum, count), v| {
        let ulp = if v > 0.0 {
            let bits = f32_to_f16_bits(v);
            f64::from(f16_bits_to_f32(bits + 1) - f16_bits_to_f32(bits))
        } else {
            0.0
        };
        (sum + ulp * ulp / 12.0, count + 1)
    });
    (sum / count as f64).sqrt() as f32
}

fn interior_is_nonzero(tile: &[[f32; 3]]) -> bool {
    interior_values(tile).any(|v| v != 0.0)
}

/// The compact atlas's tiles, in compact-slot order.
fn section_tiles(section: &OctahedralShVolumeSection, texel: impl Fn(usize) -> [f32; 3]) -> Tiles {
    let valid = section.probes.iter().filter(|p| p.validity != 0).count();
    let width = section.atlas_dimensions[0] as usize;
    let layer_texels = width * section.atlas_dimensions[1] as usize;
    (0..valid)
        .map(|slot| {
            let [layer, tx, ty] = irradiance_array_tile_location(
                slot,
                section.tiles_per_layer,
                section.atlas_tiles_per_row,
            );
            let mut tile = Vec::with_capacity(TILE * TILE);
            for y in 0..TILE {
                for x in 0..TILE {
                    let row = ty as usize * TILE + y;
                    let column = tx as usize * TILE + x;
                    tile.push(texel(layer as usize * layer_texels + row * width + column));
                }
            }
            tile
        })
        .collect()
}

fn rgba16f_tiles(section: &OctahedralShVolumeSection) -> Tiles {
    assert_eq!(section.irradiance_format, IRRADIANCE_FORMAT_RGBA16F);
    section_tiles(section, |i| {
        let bytes = &section.compact_atlas[i * 8..i * 8 + 6];
        std::array::from_fn(|c| {
            f16_bits_to_f32(u16::from_le_bytes([bytes[2 * c], bytes[2 * c + 1]]))
        })
    })
}

/// The full-set section's tiles after a BC6H round trip.
fn bc6h_round_trip_tiles(section: &OctahedralShVolumeSection) -> Tiles {
    let encoded = encode_sh_volume_section_bc6h(section, false);
    let (width, height) = (section.atlas_dimensions[0], section.atlas_dimensions[1]);
    let (padded_width, padded_height) = (width.div_ceil(4) * 4, height.div_ceil(4) * 4);
    let layer_len = (padded_width / 4) as usize * (padded_height / 4) as usize * 16;
    let mut decoded = Vec::with_capacity(section.layer_count as usize * (width * height) as usize);
    for layer in encoded.compact_atlas.chunks(layer_len) {
        let rgba = crate::bc6h::decode_bc6h_rgb_to_f32_rgba(layer, padded_width, padded_height)
            .expect("the encoder writes Mode 11");
        for y in 0..height as usize {
            for x in 0..width as usize {
                let base = (y * padded_width as usize + x) * 4;
                decoded.push([rgba[base], rgba[base + 1], rgba[base + 2]]);
            }
        }
    }
    section_tiles(section, |i| decoded[i])
}

/// Every probe tile of a delta section, in sub-block order.
fn delta_tiles(section: &DeltaShVolumesSection) -> Tiles {
    section
        .delta_subblocks
        .chunks(DEFAULT_DELTA_PROBE_F16_STRIDE)
        .map(|probe| {
            probe
                .chunks(DELTA_TILE_TEXEL_F16_COUNT)
                .map(|t| std::array::from_fn(|c| f16_bits_to_f32(t[c])))
                .collect()
        })
        .collect()
}

/// Each delta probe's compact slot in `base`, in [`delta_tiles`] order. The
/// bake writes one dense sub-block per CSR entry, so probe `t` is in-cell probe
/// `local = t % 64` of the entry `t / 64`, whose affinity cell is the one whose
/// `affinity_offsets` range holds the entry. `delta_probe_position` puts that
/// probe on base global coords `cell_xyz * factor + local_xyz` (x-fastest
/// `local = lx + ly*4 + lz*16`) at the base grid's origin and spacing; the base
/// linear index is x-fastest over `grid_dimensions`, and the compact atlas
/// numbers valid probes in linear order. `None` past the grid or on an invalid
/// base probe.
fn delta_base_slots(
    delta: &DeltaShVolumesSection,
    base: &OctahedralShVolumeSection,
) -> Vec<Option<usize>> {
    let mut next_slot = 0;
    let base_slots: Vec<Option<usize>> = base
        .probes
        .iter()
        .map(|probe| {
            (probe.validity != 0).then(|| {
                next_slot += 1;
                next_slot - 1
            })
        })
        .collect();
    let factor = u32::from(delta.affinity_factor);
    let [cells_x, cells_y, _] = delta.affinity_dims;
    let [grid_x, grid_y, grid_z] = base.grid_dimensions;
    let mut slots = Vec::with_capacity(delta.affinity_lights.len() * PROBES_PER_CELL);
    for (cell, entries) in delta.affinity_offsets.windows(2).enumerate() {
        let cell = cell as u32;
        let corner = [
            cell % cells_x,
            (cell / cells_x) % cells_y,
            cell / (cells_x * cells_y),
        ]
        .map(|c| c * factor);
        for _ in entries[0]..entries[1] {
            slots.extend((0..PROBES_PER_CELL as u32).map(|local| {
                let x = corner[0] + local % factor;
                let y = corner[1] + (local / factor) % factor;
                let z = corner[2] + local / (factor * factor);
                (x < grid_x && y < grid_y && z < grid_z)
                    .then(|| base_slots[(x + grid_x * (y + grid_y * z)) as usize])
                    .flatten()
            }));
        }
    }
    slots
}

/// One estimator's per-probe deviation from the full set.
#[derive(Default)]
struct Deviation {
    /// RMS of the estimator minus the full set.
    rms: Vec<f32>,
    /// Estimator minus full set, summed, relative to the full set's sum.
    signed: Vec<f64>,
    /// Whether any interior value differs from the full set.
    affected: Vec<bool>,
}

impl Deviation {
    fn push(&mut self, tile: &[[f32; 3]], full: &[[f32; 3]]) {
        self.rms.push(interior_rms(tile, full));
        self.signed.push(interior_signed_relative(tile, full));
        self.affected
            .push(interior_values(tile).ne(interior_values(full)));
    }
}

/// Per-probe measurements for every probe with a nonzero full-set bounce.
#[derive(Default)]
struct ProbeErrors {
    /// One-sample against the full set: the estimator under test.
    one: Deviation,
    /// The full set reseeded against itself: the noise the bake already has.
    control: Deviation,
    /// RMS of the shipped base encoding's error at the probe: the bar.
    bar: Vec<f32>,
    /// Expected f16 rounding RMS of the full set, reported but not gating;
    /// empty when not reported.
    f16: Vec<f32>,
    /// RMS of the full set itself.
    signal: Vec<f32>,
}

impl ProbeErrors {
    fn push(&mut self, one: &[[f32; 3]], control: &[[f32; 3]], full: &[[f32; 3]], bar: f32) {
        self.one.push(one, full);
        self.control.push(control, full);
        self.bar.push(bar);
        self.signal
            .push(interior_rms(full, &[[0.0; 3]; TILE * TILE]));
    }
}

fn percentile(sorted: &[f32], p: f64) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[(((sorted.len() - 1) as f64 * p).round() as usize).min(sorted.len() - 1)]
}

/// An estimator row's error at p50, p99 and max, and its mean signed error.
struct RowStats {
    at: [f32; 3],
    within_bar: bool,
    mean_signed: f64,
}

/// One table row, less its verdict cell, for `deviation` over the probes
/// `keep` selects.
fn row(
    label: &str,
    estimator: &str,
    scope: &str,
    errors: &ProbeErrors,
    deviation: &Deviation,
    keep: impl Fn(usize) -> bool,
) -> (String, RowStats) {
    let kept: Vec<usize> = (0..errors.bar.len()).filter(|&i| keep(i)).collect();
    let sorted = |values: &[f32]| {
        let mut out: Vec<f32> = kept.iter().map(|&i| values[i]).collect();
        out.sort_by(f32::total_cmp);
        out
    };
    let (error, bar, signal) = (
        sorted(&deviation.rms),
        sorted(&errors.bar),
        sorted(&errors.signal),
    );
    let points = [0.5, 0.99, 1.0];
    let at = points.map(|p| percentile(&error, p));
    let bar_at = points.map(|p| percentile(&bar, p));
    let within_bar = at.iter().zip(&bar_at).all(|(e, b)| e <= b);
    let exceeding = kept
        .iter()
        .filter(|&&i| deviation.rms[i] > errors.bar[i])
        .count();
    let mean_signed = if kept.is_empty() {
        0.0
    } else {
        kept.iter().map(|&i| deviation.signed[i]).sum::<f64>() / kept.len() as f64
    };
    let f16 = if errors.f16.is_empty() {
        "—".to_string()
    } else {
        let f16 = sorted(&errors.f16);
        let [p50, p99, max] = points.map(|p| percentile(&f16, p));
        format!("{p50:.3e} / {p99:.3e} / {max:.3e}")
    };
    let line = format!(
        "| {label} | {estimator} | {scope} | {} | {:.3e} | {:.3e} / {:.3e} | {:.3e} / {:.3e} | {:.3e} / {:.3e} | {exceeding} ({:.1}%) | {:+.3}% | {f16} |",
        kept.len(),
        percentile(&signal, 0.5),
        at[0],
        bar_at[0],
        at[1],
        bar_at[1],
        at[2],
        bar_at[2],
        100.0 * exceeding as f64 / kept.len().max(1) as f64,
        100.0 * mean_signed,
    );
    (
        line,
        RowStats {
            at,
            within_bar,
            mean_signed,
        },
    )
}

/// Four rows. One-sample over every probe with a nonzero full-set bounce,
/// judged on its percentiles against the bar, then over the probes it
/// changes, judged on their mean signed error. The control over the same
/// nonzero probes, with one-sample's p99 as a multiple of the control's, then
/// over the probes the reseed changes; both control rows are information.
fn report(label: &str, errors: &ProbeErrors) -> bool {
    let verdict = |pass| if pass { "pass" } else { "FAIL" };
    let (line, one) = row(label, "one-sample", "nonzero", errors, &errors.one, |_| {
        true
    });
    println!("{line} percentiles {} |", verdict(one.within_bar));
    let (line, one_affected) = row(label, "one-sample", "affected", errors, &errors.one, |i| {
        errors.one.affected[i]
    });
    let unbiased = one_affected.mean_signed.abs() <= MAX_MEAN_SIGNED_RELATIVE_ERROR;
    println!("{line} bias {} |", verdict(unbiased));
    let (line, control) = row(label, "control", "nonzero", errors, &errors.control, |_| {
        true
    });
    println!(
        "{line} p99 one-sample / control {:.2} (info) |",
        one.at[1] / control.at[1],
    );
    let (line, _) = row(label, "control", "affected", errors, &errors.control, |i| {
        errors.control.affected[i]
    });
    println!("{line} (info) |");
    one.within_bar && unbiased
}

/// Bakes each map's cold id-34 SH volume and its id-27 indirect deltas (where
/// animated lights exist) three ways through the fixture front end (buried
/// lights excluded): one-sample, full set, and a control that reseeds the full
/// set (`BounceVisibility::FullSetReseeded`). Per probe with a nonzero
/// full-set bounce it measures the interior texels before any encode, and
/// passes when, for each bake, both of these hold:
/// - one-sample's p50, p99 and max error stay within the same percentiles of
///   the bar, the error the shipped base encoding already leaves at the same
///   probes: the full-set id 34's BC6H round trip through the compiler's
///   encoder. An id 27 probe is paired with the coincident id 34 probe
///   (`delta_base_slots`), because a delta is composed onto that probe's base
///   irradiance at runtime, so the base's BC6H error is the error already
///   visible there. The delta's own RGB16F rounding sits below any Monte Carlo
///   noise, so it is printed as "f16 (info)" and does not gate;
/// - the mean signed relative error over the probes one-sample changes is
///   within `MAX_MEAN_SIGNED_RELATIVE_ERROR`.
///
/// The control rows, the full set's own seed-to-seed noise, and one-sample's
/// p99 as a multiple of the control's are printed "(info)" and do not gate:
/// one emitter sample against up to 32 necessarily adds about √samples noise,
/// so the ratio measures that trade, not a defect.
///
/// Run it with:
///   cargo test -p postretro-level-compiler --bin prl-build one_sample_bounce_parity -- --ignored --nocapture
/// `POSTRETRO_BOUNCE_PARITY_MAPS=<a>,<b>` replaces the map list.
#[test]
#[ignore = "measurement: bakes real maps' SH volumes and deltas three times, one-sample, full-set and reseeded"]
fn one_sample_bounce_parity_on_real_maps() {
    let maps =
        std::env::var("POSTRETRO_BOUNCE_PARITY_MAPS").unwrap_or_else(|_| DEFAULT_MAPS.into());
    let config = ShConfig {
        probe_spacing: DEFAULT_PROBE_SPACING,
    };
    let control = BakeControl::unrestricted();
    let reseeded = BounceVisibility::FullSetReseeded(CONTROL_SEED_SALT);
    println!(
        "| bake (bar) | estimator | probes | count | p50 signal RMS | p50 error / bar | p99 error / bar | max error / bar | error > bar | mean signed rel. error | f16 (info) p50 / p99 / max | verdict |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|");
    let mut all_pass = true;
    for map in maps.split(',').map(str::trim).filter(|m| !m.is_empty()) {
        let fx = load_fixture(map);
        let buried = BuriedLights::classify(&fx.tree, &fx.lights);
        let static_lights = StaticBakedLights::from_lights_excluding(&fx.lights, &buried);
        let animated_lights = AnimatedBakedLights::from_lights_excluding(&fx.lights, &buried);
        let inputs = ShBakeCtx {
            bvh: &fx.bvh,
            primitives: &fx.primitives,
            geometry: &fx.geometry,
            tree: &fx.tree,
            exterior_leaves: &fx.exterior_leaves,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: fx.lights.len(),
        };

        let timed = |mode| {
            let started = std::time::Instant::now();
            let section = bake_sh_volume_with_visibility(&inputs, &config, &control, mode);
            (section, started.elapsed())
        };
        let (one, one_time) = timed(BounceVisibility::OneSample);
        let (full, full_time) = timed(BounceVisibility::FullSet);
        let (reseed, reseed_time) = timed(reseeded);
        eprintln!(
            "{map}: {} static / {} animated lights, {} probes; id 34 one-sample {:.1?}, full-set {:.1?}, control {:.1?}",
            static_lights.len(),
            animated_lights.len(),
            full.probes.len(),
            one_time,
            full_time,
            reseed_time,
        );
        let full_tiles = rgba16f_tiles(&full);
        // The base encoding's error at every compact slot: id 34's bar, and
        // id 27's at the coincident probe.
        let base_bc6h_error: Vec<f32> = bc6h_round_trip_tiles(&full)
            .iter()
            .zip(&full_tiles)
            .map(|(b, f)| interior_rms(b, f))
            .collect();
        let mut errors = ProbeErrors::default();
        for (((o, c), f), &bar) in rgba16f_tiles(&one)
            .iter()
            .zip(&rgba16f_tiles(&reseed))
            .zip(&full_tiles)
            .zip(&base_bc6h_error)
        {
            if interior_is_nonzero(f) {
                errors.push(o, c, f, bar);
            }
        }
        all_pass &= report(&format!("{map} id 34 (BC6H)"), &errors);

        if animated_lights.is_empty() {
            continue;
        }
        let portals = crate::portals::generate_portals(&fx.tree);
        let delta_inputs = DeltaBakeInputs {
            bvh: &fx.bvh,
            primitives: &fx.primitives,
            geometry: &fx.geometry,
            tree: &fx.tree,
            exterior_leaves: &fx.exterior_leaves,
            portals: &portals,
            animated_lights: &animated_lights,
        };
        let delta = |mode| {
            let started = std::time::Instant::now();
            let section =
                bake_delta_sh_volumes_with_visibility(&delta_inputs, &config, None, &control, mode)
                    .0
                    .expect("animated lights bake a delta section");
            (section, started.elapsed())
        };
        let (one_delta, one_time) = delta(BounceVisibility::OneSample);
        let (full_delta, full_time) = delta(BounceVisibility::FullSet);
        let (reseed_delta, reseed_time) = delta(reseeded);
        let base_slots = delta_base_slots(&full_delta, &full);
        let mut errors = ProbeErrors::default();
        let mut unpaired = 0usize;
        for (((o, c), f), slot) in delta_tiles(&one_delta)
            .iter()
            .zip(&delta_tiles(&reseed_delta))
            .zip(&delta_tiles(&full_delta))
            .zip(&base_slots)
        {
            if !interior_is_nonzero(f) {
                continue;
            }
            let Some(slot) = *slot else {
                unpaired += 1;
                continue;
            };
            errors.push(o, c, f, base_bc6h_error[slot]);
            errors.f16.push(interior_f16_rounding_rms(f));
        }
        eprintln!(
            "{map}: id 27 {} CSR entries; one-sample {:.1?}, full-set {:.1?}, control {:.1?}; {unpaired} nonzero delta probes without a valid base probe",
            full_delta.affinity_lights.len(),
            one_time,
            full_time,
            reseed_time,
        );
        all_pass &= unpaired == 0;
        all_pass &= report(&format!("{map} id 27 (id 34 BC6H)"), &errors);
    }
    assert!(
        all_pass,
        "one-sample bounce exceeded its bar at some percentile, biased the affected probes, or left a delta probe unpaired"
    );
}
