// The motion proof: a swept surface lit through the mover probe channel, read
// at every knot of a fine-pose truth bake (`light_poses` 76 on the rising
// room, 98 on the big rising room, 113 on em_sweeper), against the default
// build composed at the same t′. Per receiver on the surface, the forward
// shader's probe weights (trilinear × smooth backface × Chebyshev on the
// composed depth tile) give the far-side share (the normalised weight on
// probes behind the surface), and the truth's weights light the composed
// bounce and direct of each build.
// The truth reads its own stored states at its knots, where either depth rule
// is exact. An even-pose build read by `DepthRule::ThroughBase` (even poses
// alone, depth through the base on every interval) is read the same way: it
// must fail the share bar, and its value miss at each knot bounds the
// shipped build's there. Only the tested mover's channel entries are read;
// the map's other movers stay absent.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting) ·
// context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::collections::HashMap;

use glam::DVec3;
use postretro_level_format::indirect_display::DEFAULT_INDIRECT_SCALE;
use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::mover_probe_channel::{
    DepthMix, DepthRecordView, DepthSegment, DepthSource, PoseMark, depth_segment, mix_depth_tile,
    pose_interval,
};
use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION, irradiance_tile_texel_coord,
};
use postretro_level_format::probe_depth_tiles::{PROBE_DEPTH_TILE_TEXELS, sample_tile_moments};
use postretro_level_format::sh_volume::OctahedralAtlasTexel;

use super::{Build, rgb, sh_tile, texel};
use crate::bc6h::f16_bits_to_f32;
use crate::fixture_pipeline::fixture_path;
use crate::mover_light_gate::{even_knots, poses_in_band};
use crate::mover_probe_channel::fixture_scene::SPACING;
use crate::mover_probe_channel::reference_maps::quake_offset;
use crate::mover_probe_channel::{MoverAbsentDepth, direct_display_levels, display_levels};

/// The far-side share bar, read per truth knot: the trip's time-weighted
/// mean of |share − truth's| within 0.01. The largest at any knot is
/// reported, not gated, on every surface (owner, 2026-11-12). The mean
/// integrates over travel by the trapezoid rule on the truth's knot t′ (each
/// mover runs at constant speed in t′), so a short geometry-knot window
/// weighs its share of the trip, not one knot's (owner, 2026-11-12).
const SHARE_MEAN_BAR: f64 = 0.01;
/// Composed values at every knot within three display levels of the truth,
/// or within 1.5 levels of the even-pose, through-base build's miss at that
/// knot, except at receivers that read through a long stored interval
/// ([`LONG_SHARE`]), which are reported for the long-interval follow-up
/// (owner, 2026-11-12).
const VALUE_BAR_LEVELS: f32 = 3.0;
const VALUE_SHARED_MISS_LEVELS: f32 = 1.5;
/// A receiver reads through a long stored interval when at least this share
/// of its light, by the truth's probe weights, comes from probes whose
/// stored interval at that t′ (`Build::stored_span`) is longer than one
/// even step of the shipped build: most of its value is then a blend over a
/// long interval, the follow-up's ground.
const LONG_SHARE: f64 = 0.5;

/// The runtime's Chebyshev constants (`sh_sample.wgsl`).
const MIN_VARIANCE: f32 = 1.0e-4;
const BIAS_CELL_FRACTION: f32 = 0.05;
const MIN_VISIBILITY: f32 = 0.03;
/// The smooth backface weight's floor on movers (`sh_backface_weight`).
const BACKFACE_FLOOR: f64 = 0.03;
/// Receivers per surface: a `RECEIVERS × RECEIVERS` grid.
const RECEIVERS: usize = 7;
/// Quake map units per engine metre.
const QUAKE_PER_METRE: f64 = 1.0 / 0.0254;

/// How a build blends depth between two stored states.
#[derive(Clone, Copy, PartialEq)]
enum DepthRule {
    /// The format's rule ([`depth_segment`]): mean and variance between
    /// poses, raw through the base between termini.
    Shipped,
    /// Raw moments through the base on every interval, the rule before the
    /// mean-and-variance blend.
    ThroughBase,
}

/// One build of a map as the forward shader reads its tested mover.
struct Read<'a> {
    build: &'a Build,
    rule: DepthRule,
    absent: MoverAbsentDepth<'a>,
    depth: HashMap<u32, DepthRecordView<'a>>,
}

impl<'a> Read<'a> {
    fn new(build: &'a Build, rule: DepthRule) -> Self {
        let depth = build
            .section
            .depth_record_views()
            .filter(|view| view.record.mover_id == build.mover_id)
            .map(|view| (view.record.probe_index, view))
            .collect();
        Self {
            build,
            rule,
            absent: MoverAbsentDepth::new(&build.base.section, &build.base.depth_tiles),
            depth,
        }
    }

    /// The probe's live depth tile at `t` (`depth_blend::segment` and
    /// `blend_probe` for one stated mover), as its f16 texels: its base
    /// where the mover carries no depth record for it.
    fn depth_tile(&self, probe: u32, t: f32) -> Vec<[u16; 2]> {
        let base = planes(&self.absent.base_moments(probe));
        let Some(view) = self.depth.get(&probe) else {
            return packed(&base);
        };
        let marks = &view.record.marks;
        let span = pose_interval(&self.build.knots, t, |pose| {
            marks[pose as usize] != PoseMark::Unmarked
        });
        let segment = match self.rule {
            DepthRule::Shipped => depth_segment(span, marks.len()),
            DepthRule::ThroughBase => through_base(span.lo, span.hi, span.weight),
        };
        let state = |source: DepthSource| match source {
            DepthSource::Pose(pose) => view.pose_tile(pose as usize).map_or_else(
                || base.clone(),
                |tile| {
                    let pairs: Vec<[f32; 2]> = tile
                        .iter()
                        .map(|texel| texel.map(f16_bits_to_f32))
                        .collect();
                    planes(&pairs)
                },
            ),
            DepthSource::Base => base.clone(),
        };
        let mut out = vec![0.0; 2 * PROBE_DEPTH_TILE_TEXELS];
        mix_depth_tile(
            &state(segment.from),
            &state(segment.to),
            segment.weight,
            segment.mix,
            &mut out,
        );
        packed(&out)
    }
}

/// [`DepthRule::ThroughBase`] for one interval: D_lo → base → D_hi in raw
/// moments, the base at the interval's midpoint.
fn through_base(lo: u32, hi: u32, weight: f32) -> DepthSegment {
    let (from, to, weight) = if weight <= 0.0 {
        (DepthSource::Pose(lo), DepthSource::Pose(lo), 0.0)
    } else if weight <= 0.5 {
        (DepthSource::Pose(lo), DepthSource::Base, 2.0 * weight)
    } else {
        (DepthSource::Base, DepthSource::Pose(hi), 2.0 * weight - 1.0)
    };
    DepthSegment {
        from,
        to,
        weight,
        mix: DepthMix::Raw,
    }
}

/// Moment pairs as one tile of means, then second moments.
fn planes(pairs: &[[f32; 2]]) -> Vec<f32> {
    pairs
        .iter()
        .map(|pair| pair[0])
        .chain(pairs.iter().map(|pair| pair[1]))
        .collect()
}

/// A tile of means, then second moments, as the live tile stores it.
fn packed(planes: &[f32]) -> Vec<[u16; 2]> {
    let (means, second) = planes.split_at(planes.len() / 2);
    means
        .iter()
        .zip(second)
        .map(|(&mean, &mean_sq)| [mean, mean_sq].map(f32_to_f16_bits))
        .collect()
}

/// `sh_chebyshev_visibility`.
fn chebyshev([mean, mean_sq]: [f32; 2], distance: f32, bias: f32) -> f32 {
    let variance = (mean_sq - mean * mean).max(MIN_VARIANCE);
    let delta = (distance - mean - bias).max(0.0);
    let visibility = if delta > 0.0 {
        variance / (variance + delta * delta)
    } else {
        1.0
    };
    visibility.clamp(MIN_VISIBILITY, 1.0)
}

/// An irradiance tile's RGB toward `normal`, as the sampler's bilinear
/// filter reads it.
fn toward(tile: &[OctahedralAtlasTexel], normal: [f32; 3]) -> [f32; 3] {
    let dim = DEFAULT_IRRADIANCE_TILE_DIMENSION as usize;
    let [u, v] = irradiance_tile_texel_coord(
        normal,
        DEFAULT_IRRADIANCE_TILE_DIMENSION,
        DEFAULT_IRRADIANCE_TILE_BORDER,
    )
    .map(|c| c - 0.5);
    let (x0, y0) = (u.floor(), v.floor());
    let (fx, fy) = (u - x0, v - y0);
    let at = |x: f32, y: f32| {
        let (x, y) = (
            (x as isize).clamp(0, dim as isize - 1) as usize,
            (y as isize).clamp(0, dim as isize - 1) as usize,
        );
        rgb(&tile[y * dim + x])
    };
    let (a, b, c, d) = (
        at(x0, y0),
        at(x0 + 1.0, y0),
        at(x0, y0 + 1.0),
        at(x0 + 1.0, y0 + 1.0),
    );
    std::array::from_fn(|i| {
        (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy
    })
}

/// One swept surface: receivers at t′ = 0, offset along the mover's travel,
/// and the surface normal, both in Quake units and axes.
struct Surface {
    name: &'static str,
    receivers: Vec<[f64; 3]>,
    normal: [f64; 3],
}

/// A `RECEIVERS²` grid over `a` × `b` on the plane `axis = at`.
fn plane(
    name: &'static str,
    axis: usize,
    at: f64,
    a: [f64; 2],
    b: [f64; 2],
    normal: [f64; 3],
) -> Surface {
    let step = |range: [f64; 2], i: usize| {
        range[0] + (range[1] - range[0]) * i as f64 / (RECEIVERS - 1) as f64
    };
    let mut receivers = Vec::new();
    for i in 0..RECEIVERS {
        for j in 0..RECEIVERS {
            let (u, v) = (step(a, i), step(b, j));
            let mut q = [0.0; 3];
            q[axis] = at;
            let others: Vec<usize> = (0..3).filter(|&other| other != axis).collect();
            q[others[0]] = u;
            q[others[1]] = v;
            receivers.push(q);
        }
    }
    Surface {
        name,
        receivers,
        normal,
    }
}

/// Engine metres (and axes) from Quake units.
fn engine([x, y, z]: [f64; 3]) -> DVec3 {
    DVec3::new(-y, z, -x) / QUAKE_PER_METRE
}

/// A build's far-side share and values over one surface at one truth knot.
#[derive(Default)]
struct Sample {
    /// Mean over receivers of the far-side share.
    share: f64,
    /// Each receiver's composed bounce and direct toward the normal, lit
    /// with the truth's weights.
    values: Vec<([f32; 3], [f32; 3])>,
    /// Each receiver's share of light, by the truth's weights, from probes
    /// whose stored interval in this build is longer than `long_span`.
    long: Vec<f64>,
}

/// Static direct terms per probe, shared by every build of one map (they
/// differ only in `light_poses`).
type Terms = HashMap<usize, Vec<(u32, [f32; 27])>>;

/// Every build read at truth knot `t` over `surface`, the truth first.
/// `long_span` is the stored interval, in t′, past which a probe reads
/// through a long interval.
fn sample(
    reads: &[Read<'_>],
    terms: &mut Terms,
    surface: &Surface,
    travel: DVec3,
    t: f32,
    long_span: f32,
) -> Vec<Sample> {
    let truth = reads[0].build;
    let section = &truth.base.section;
    let origin = DVec3::from(section.grid_origin.map(f64::from));
    let cell = DVec3::from(section.cell_size.map(f64::from));
    let dims = section.grid_dimensions;
    let bias = BIAS_CELL_FRACTION * section.cell_size.into_iter().fold(f32::INFINITY, f32::min);
    let normal = engine(surface.normal).normalize();
    let offset = quake_offset(travel * f64::from(t));
    let mut out: Vec<Sample> = reads.iter().map(|_| Sample::default()).collect();
    let mut value_cache: HashMap<(usize, usize), ([f32; 3], [f32; 3])> = HashMap::new();
    let mut span_cache: HashMap<(usize, usize), f32> = HashMap::new();
    for q in &surface.receivers {
        let receiver = engine(std::array::from_fn(|a| q[a] + offset[a]));
        let grid = (receiver - origin) / cell;
        let base = grid.floor();
        let frac = grid - base;
        let mut corners = Vec::new();
        for corner in 0..8u32 {
            let step = DVec3::new(
                f64::from(corner & 1),
                f64::from((corner >> 1) & 1),
                f64::from((corner >> 2) & 1),
            );
            let index = base + step;
            if index.min_element() < 0.0
                || index.x >= f64::from(dims[0])
                || index.y >= f64::from(dims[1])
                || index.z >= f64::from(dims[2])
            {
                continue;
            }
            let probe =
                (index.x + f64::from(dims[0]) * (index.y + f64::from(dims[1]) * index.z)) as usize;
            if section.probes[probe].validity == 0 {
                continue;
            }
            let trilinear: f64 = (0..3)
                .map(|a| {
                    if step[a] > 0.0 {
                        frac[a]
                    } else {
                        1.0 - frac[a]
                    }
                })
                .product();
            let to_probe = (step - frac) * cell;
            let distance = to_probe.length();
            let cosine = if distance > 1e-6 {
                (to_probe / distance).dot(normal).clamp(-1.0, 1.0)
            } else {
                1.0
            };
            let backface = ((cosine + 1.0) * 0.5).powi(8) + BACKFACE_FLOOR;
            let to_sample = (-to_probe).as_vec3().to_array();
            let weights: Vec<f64> = reads
                .iter()
                .map(|read| {
                    let tile = read.depth_tile(probe as u32, t);
                    let moments = sample_tile_moments(&tile, to_sample);
                    trilinear * backface * f64::from(chebyshev(moments, distance as f32, bias))
                })
                .collect();
            corners.push((probe, cosine < 0.0, weights));
        }
        let truth_sum: f64 = corners.iter().map(|(_, _, w)| w[0]).sum();
        assert!(
            truth_sum > 0.0,
            "{}: a receiver no probe lights",
            surface.name
        );
        for (b, read) in reads.iter().enumerate() {
            let sum: f64 = corners.iter().map(|(_, _, w)| w[b]).sum();
            let far: f64 = corners
                .iter()
                .filter(|(_, far, _)| *far)
                .map(|(_, _, w)| w[b])
                .sum();
            out[b].share += far / sum / surface.receivers.len() as f64;
            let (mut bounce, mut direct) = ([0.0f32; 3], [0.0f32; 3]);
            let mut long = 0.0;
            for (probe, _, w) in &corners {
                let (b_value, d_value) = *value_cache
                    .entry((b, *probe))
                    .or_insert_with(|| probe_value(read.build, terms, *probe, t, normal));
                let share = w[0] / truth_sum;
                for c in 0..3 {
                    bounce[c] += share as f32 * b_value[c];
                    direct[c] += share as f32 * d_value[c];
                }
                let span = *span_cache
                    .entry((b, *probe))
                    .or_insert_with(|| read.build.stored_span(*probe, t));
                // Spans are f32 fractions: an interval of exactly one even
                // step can differ from `long_span` by rounding alone, so it
                // must not count as long.
                if span > long_span * (1.0 + 1e-3) {
                    long += share;
                }
            }
            out[b].values.push((bounce, direct));
            out[b].long.push(long);
        }
    }
    out
}

/// The probe's composed bounce and direct toward `normal` at `t`.
fn probe_value(
    build: &Build,
    terms: &mut Terms,
    probe: usize,
    t: f32,
    normal: DVec3,
) -> ([f32; 3], [f32; 3]) {
    let normal = normal.as_vec3().to_array();
    let bounce = toward(&build.bounce(probe, t), normal);
    let terms = terms.entry(probe).or_insert_with(|| {
        build
            .scene
            .static_channel_lights()
            .iter()
            .map(|light| (light.light_index, build.scene.term(light, probe)))
            .collect()
    });
    let visibility = build.visibility(probe, t);
    let mut sum = [0f32; 27];
    for (light, term) in terms.iter() {
        let visible = visibility.get(light).copied().unwrap_or(1.0);
        for (total, value) in sum.iter_mut().zip(term) {
            *total += visible * value;
        }
    }
    (bounce, toward(&sh_tile(&sum), normal))
}

/// A receiver's value as the display shows it: bounce through the indirect
/// scale and baked direct SH at weight 1, as one irradiance in the indirect
/// scale's units for [`display_levels`].
fn combined((bounce, direct): ([f32; 3], [f32; 3])) -> OctahedralAtlasTexel {
    texel(std::array::from_fn(|c| {
        bounce[c] + direct[c] / DEFAULT_INDIRECT_SCALE
    }))
}

/// Build `b`'s worst value at one truth knot over the receivers `keep`
/// selects by their long share in build `gate` (so two builds are compared
/// over the same receivers), in display levels against the truth: combined,
/// then the bounce alone and the direct alone at that receiver.
fn worst_value(
    at: &[Sample],
    b: usize,
    gate: usize,
    keep: impl Fn(f64) -> bool,
) -> (f32, f32, f32) {
    at[b]
        .values
        .iter()
        .zip(&at[0].values)
        .zip(&at[gate].long)
        .filter(|(_, long)| keep(**long))
        .map(|(values, _)| values)
        .fold((0.0, 0.0, 0.0), |worst, (candidate, truth)| {
            let levels = display_levels(&[combined(*candidate)], &[combined(*truth)]);
            if levels > worst.0 {
                (
                    levels,
                    display_levels(&[texel(candidate.0)], &[texel(truth.0)]),
                    direct_display_levels(&[texel(candidate.1)], &[texel(truth.1)]),
                )
            } else {
                worst
            }
        })
}

/// Where `t` falls among a build's knots: on a knot, or inside an interval
/// whose ends are both even poses (fractions of the `even` count), or one
/// with a geometry knot at either end, with its length in even steps.
fn interval_kind(knots: &[f32], even: u32, t: f32) -> String {
    if knots.iter().any(|&knot| (knot - t).abs() < 1e-6) {
        return "on a knot".into();
    }
    let at = knots.partition_point(|&knot| knot <= t);
    let (lo, hi) = (knots[at - 1], knots[at]);
    let steps = (even - 1) as f32;
    let is_even = |knot: f32| (knot * steps - (knot * steps).round()).abs() < 1e-3;
    let kind = if is_even(lo) && is_even(hi) {
        "even interval"
    } else {
        "geometry interval"
    };
    format!(
        "{kind} {lo:.3}–{hi:.3} ({:.2} of an even step)",
        (hi - lo) * steps
    )
}

/// The trapezoid rule's weight of each knot over the trip `knots[0]..=last`,
/// summing to one.
fn trapezoid_weights(knots: &[f32]) -> Vec<f64> {
    let n = knots.len();
    assert!(n >= 2, "a trip has two knots");
    let span = f64::from(knots[n - 1] - knots[0]);
    (0..n)
        .map(|i| {
            let lo = f64::from(knots[i.saturating_sub(1)]);
            let hi = f64::from(knots[(i + 1).min(n - 1)]);
            (hi - lo) / 2.0 / span
        })
        .collect()
}

/// One candidate's reading of one surface against the truth.
struct Verdict {
    /// |share − truth's|, time-weighted over the trip: the gated mean.
    share_mean: f64,
    /// The same, unweighted over the truth's knots: reported.
    share_knot_mean: f64,
    share_worst: (f64, f32),
    values_worst: (f32, f32),
    /// At the worst value: the bounce alone's and the direct alone's
    /// levels, and the receiver's Quake position at t′ 0.
    values_split: (f32, f32, [f64; 3]),
    /// The trip's mean and worst far-side share, the candidate's own.
    own_share: (f64, f64),
}

impl Verdict {
    /// The far-side share bar: the trip's time-weighted mean.
    fn share_passes(&self) -> bool {
        self.share_mean <= SHARE_MEAN_BAR
    }

    fn line(&self) -> String {
        format!(
            "far-side share {:.3} mean | {:.3} worst; |Δ share| from truth {:.4} time-weighted \
             mean ({:.4} over knots), {:.4} worst (t′ {:.3}, reported); values worst {:.2} \
             levels over every receiver (t′ {:.3}; bounce {:.2}, direct {:.2} at {:?}) → share \
             {}",
            self.own_share.0,
            self.own_share.1,
            self.share_mean,
            self.share_knot_mean,
            self.share_worst.0,
            self.share_worst.1,
            self.values_worst.0,
            self.values_worst.1,
            self.values_split.0,
            self.values_split.1,
            self.values_split.2,
            if self.share_passes() { "pass" } else { "MISS" }
        )
    }
}

/// The value bar at one truth knot: the shipped build's worst receiver
/// outside long stored intervals within [`VALUE_BAR_LEVELS`] of the truth,
/// or within [`VALUE_SHARED_MISS_LEVELS`] of the even-pose, through-base
/// build's worst there. Receivers that read through a long stored interval
/// ([`LONG_SHARE`]) are reported only.
struct KnotValue {
    t: f32,
    /// The gated worst: receivers outside long stored intervals.
    shipped: f32,
    pre: f32,
    /// The reported worst: receivers that read through a long stored
    /// interval, with their bounce and direct alone.
    long: (f32, f32, f32),
}

impl KnotValue {
    fn within_three(&self) -> bool {
        self.shipped <= VALUE_BAR_LEVELS
    }

    fn passes(&self) -> bool {
        self.within_three() || self.shipped <= self.pre + VALUE_SHARED_MISS_LEVELS
    }
}

/// One surface's reading: the shipped and even-pose, through-base verdicts,
/// and the shipped build's value bar at every truth knot.
struct Row {
    surface: &'static str,
    shipped: Verdict,
    pre_amendment: Verdict,
    knots: Vec<KnotValue>,
}

/// The truth's count is clean on its own: none of its `truth_poses` even
/// poses, unmoved, parks a mover face or seam within the enclosure band of a
/// near-field probe, where it would store the solid's dark state (owner,
/// 2026-11-12). So placement moved none, and the truth's knots are the even
/// ones bit for bit.
fn assert_truth_poses_clear_every_band(map: &str, truth: &Build, truth_poses: u32) {
    let even = even_knots(truth_poses);
    let poses = truth.scene.mover_poses();
    let analysed = poses
        .analysed
        .iter()
        .find(|analysed| analysed.mover_id == truth.mover_id)
        .expect("the mover");
    let mover = truth
        .scene
        .fixture
        .kinematic_movers
        .iter()
        .find(|mover| mover.mover_id == truth.mover_id)
        .expect("the map's mover");
    let layout = truth.scene.fixture.probe_layout(SPACING);
    let in_band = poses_in_band(analysed, mover, &even, &layout, SPACING);
    assert!(
        in_band.is_empty(),
        "{map}: `light_poses {truth_poses}` parks a mover plane within a probe's enclosure band \
         at {in_band:?}; choose a clean truth count"
    );
    let knots: Vec<u32> = truth.knots.iter().map(|knot| knot.t.to_bits()).collect();
    assert_eq!(
        knots,
        even.iter().map(|t| t.to_bits()).collect::<Vec<_>>(),
        "{map}: the truth bakes its even knots"
    );
}

/// Bake the map as authored, with `light_poses truth_poses` on `mover`, and
/// with `light_poses` at its even count; read each surface at every truth
/// knot. Returns, per surface, the default build's verdict then the
/// even-pose build's read by [`DepthRule::ThroughBase`].
fn motion_proof(
    map: &str,
    mover: &str,
    truth_poses: u32,
    surfaces: &[Surface],
    travel_quake: [f64; 3],
) -> Vec<Row> {
    let text = std::fs::read_to_string(fixture_path(map)).expect("dev map");
    let named = format!("\"name\" \"{mover}\"");
    assert_eq!(text.matches(&named).count(), 1, "{map}: one {mover}");
    let dir = tempfile::tempdir().expect("maps dir");
    let write = |name: &str, poses: Option<u32>| {
        let text = match poses {
            Some(n) => text.replace(&named, &format!("{named}\n\"light_poses\" \"{n}\"")),
            None => text.clone(),
        };
        let path = dir.path().join(format!("{name}.map"));
        std::fs::write(&path, text).expect("write map");
        path
    };
    let shipped = Build::bake(&write(map, None), mover);
    let analysed = shipped.scene.mover_poses();
    let analysed = analysed
        .analysed
        .iter()
        .find(|analysed| analysed.mover_id == shipped.mover_id)
        .expect("the mover");
    let first = analysed.poses[0].bounds.min;
    let travel = analysed.poses[analysed.poses.len() - 1].bounds.min - first;
    // Every tested mover runs a straight two-waypoint path with no ramp: the
    // receivers ride `travel` × t′.
    let quake = quake_offset(travel);
    assert!(
        (0..3).all(|a| (quake[a] - travel_quake[a]).abs() < 1e-3),
        "{map}: {mover} travels {quake:?}"
    );
    // The even-pose placement: even poses only, one per probe spacing.
    let even = ((travel.length() / f64::from(SPACING)).ceil() as u32 + 1).clamp(2, 16);
    let truth = Build::bake(&write(&format!("{map}-truth"), Some(truth_poses)), mover);
    assert_eq!(
        truth.pose_count, truth_poses as usize,
        "{map}: the truth's poses"
    );
    assert_truth_poses_clear_every_band(map, &truth, truth_poses);
    let pre = Build::bake(&write(&format!("{map}-even"), Some(even)), mover);
    assert_eq!(pre.pose_count, even as usize, "{map}: the even poses");
    assert!(
        shipped.pose_count > even as usize,
        "{map}: {mover} bakes geometry knots"
    );
    eprintln!(
        "{map} {mover}: shipped {} knots, truth {}, pre-amendment {}",
        shipped.pose_count, truth.pose_count, pre.pose_count
    );
    // How finely the truth resolves the shipped build's intervals: those
    // with no truth knot strictly inside, and the shortest such.
    let truth_t: Vec<f32> = truth.knots.iter().map(|knot| knot.t).collect();
    let unresolved: Vec<f32> = shipped
        .knots
        .windows(2)
        .filter(|pair| {
            !truth_t
                .iter()
                .any(|&t| t > pair[0].t + 1e-6 && t < pair[1].t - 1e-6)
        })
        .map(|pair| pair[1].t - pair[0].t)
        .collect();
    eprintln!(
        "  {} of {} shipped interval(s) hold no truth knot inside; the shortest spans {:.4} t′ \
         ({:.2} Quake units)",
        unresolved.len(),
        shipped.pose_count - 1,
        unresolved.iter().copied().fold(f32::INFINITY, f32::min),
        f64::from(unresolved.iter().copied().fold(f32::INFINITY, f32::min))
            * travel_quake.iter().map(|v| v * v).sum::<f64>().sqrt(),
    );
    let reads = [
        Read::new(&truth, DepthRule::Shipped),
        Read::new(&shipped, DepthRule::Shipped),
        Read::new(&pre, DepthRule::ThroughBase),
    ];
    let mut terms = Terms::new();
    let mut out = Vec::new();
    // One even step of the shipped build, in t′.
    let even_step = 1.0 / (even - 1) as f32;
    for surface in surfaces {
        let knots: Vec<f32> = truth.knots.iter().map(|knot| knot.t).collect();
        let samples: Vec<Vec<Sample>> = knots
            .iter()
            .map(|&t| sample(&reads, &mut terms, surface, travel, t, even_step))
            .collect();
        let weights = trapezoid_weights(&knots);
        let verdict = |b: usize| {
            let mut share_mean = 0.0;
            let mut share_knot_mean = 0.0;
            let mut share_worst = (0.0, 0.0);
            let mut values_worst = (0.0f32, 0.0);
            let mut values_split = (0.0f32, 0.0f32, [0.0; 3]);
            let mut own = (0.0, 0.0f64);
            for ((&t, at), weight) in knots.iter().zip(&samples).zip(&weights) {
                let delta = (at[b].share - at[0].share).abs();
                share_mean += delta * weight;
                share_knot_mean += delta / knots.len() as f64;
                if delta > share_worst.0 {
                    share_worst = (delta, t);
                }
                own.0 += at[b].share * weight;
                own.1 = own.1.max(at[b].share);
                for ((candidate, truth), q) in at[b]
                    .values
                    .iter()
                    .zip(&at[0].values)
                    .zip(&surface.receivers)
                {
                    let levels = display_levels(&[combined(*candidate)], &[combined(*truth)]);
                    if levels > values_worst.0 {
                        values_worst = (levels, t);
                        values_split = (
                            display_levels(&[texel(candidate.0)], &[texel(truth.0)]),
                            direct_display_levels(&[texel(candidate.1)], &[texel(truth.1)]),
                            *q,
                        );
                    }
                }
            }
            Verdict {
                share_mean,
                share_knot_mean,
                share_worst,
                values_worst,
                values_split,
                own_share: own,
            }
        };
        let truth_own = verdict(0).own_share;
        let (with_knots, pre_amendment) = (verdict(1), verdict(2));
        eprintln!(
            "{map} {}: truth far-side share {:.3} mean | {:.3} worst over {} knots",
            surface.name,
            truth_own.0,
            truth_own.1,
            knots.len()
        );
        eprintln!("  shipped:       {}", with_knots.line());
        eprintln!("  pre-amendment: {}", pre_amendment.line());
        let shipped_knots: Vec<f32> = shipped.knots.iter().map(|knot| knot.t).collect();
        let mut values = Vec::new();
        for (&t, at) in knots.iter().zip(&samples) {
            let miss = worst_value(at, 1, 1, |long| long < LONG_SHARE);
            let pre = worst_value(at, 2, 1, |long| long < LONG_SHARE);
            let value = KnotValue {
                t,
                shipped: miss.0,
                pre: pre.0,
                long: worst_value(at, 1, 1, |long| long >= LONG_SHARE),
            };
            if !value.within_three() {
                eprintln!(
                    "    value past 3 levels at t′ {t:.3}, {}: shipped {:.2} (bounce {:.2}, \
                     direct {:.2}); pre-amendment {:.2} (bounce {:.2}, direct {:.2}) → {}",
                    interval_kind(&shipped_knots, even, t),
                    miss.0,
                    miss.1,
                    miss.2,
                    pre.0,
                    pre.1,
                    pre.2,
                    if value.passes() {
                        "passes by the shared-miss clause alone"
                    } else {
                        "MISS"
                    }
                );
            }
            values.push(value);
        }
        out.push(Row {
            surface: surface.name,
            shipped: with_knots,
            pre_amendment,
            knots: values,
        });
    }
    out
}

fn assert_shipped_passes_and_pre_amendment_misses(map: &str, rows: &[Row]) {
    // Every knot that passes by the shared-miss clause alone, and every
    // receiver past three levels inside a long stored interval: the
    // follow-up on direct interpolation over long intervals and the skip
    // bar's reach.
    for row in rows {
        for knot in row.knots.iter().filter(|k| !k.within_three() && k.passes()) {
            eprintln!(
                "{map} {}: t′ {:.3} passes by the shared-miss clause alone: shipped {:.2} \
                 levels, pre-amendment {:.2}",
                row.surface, knot.t, knot.shipped, knot.pre
            );
        }
        for knot in row.knots.iter().filter(|k| k.long.0 > VALUE_BAR_LEVELS) {
            eprintln!(
                "{map} {}: t′ {:.3} reported, inside a long stored interval: shipped {:.2} \
                 levels (bounce {:.2}, direct {:.2}), pre-amendment {:.2}",
                row.surface, knot.t, knot.long.0, knot.long.1, knot.long.2, knot.pre
            );
        }
    }
    let mut misses = Vec::new();
    for row in rows {
        if !row.shipped.share_passes() {
            misses.push(format!("{}: share {}", row.surface, row.shipped.line()));
        }
        for knot in row.knots.iter().filter(|knot| !knot.passes()) {
            misses.push(format!(
                "{}: values at t′ {:.3}: shipped {:.2} levels, pre-amendment {:.2}",
                row.surface, knot.t, knot.shipped, knot.pre
            ));
        }
    }
    assert!(
        misses.is_empty(),
        "{map}: the shipped build misses the motion proof's bars:\n{}",
        misses.join("\n")
    );
    // The guard: the even-pose, through-base build fails the share bar on
    // every surface.
    for row in rows {
        assert!(
            !row.pre_amendment.share_passes(),
            "{map} {}: the even-pose, through-base build must fail the far-side share bar: {}",
            row.surface,
            row.pre_amendment.line()
        );
    }
}

// The rising room's ceiling underside and floor top, ridden by the room
// (inner box x 848–1136, y 176–464; ceiling z −16, floor z −176 at t′ 0;
// 176 units up the trip), half a unit off each surface.
#[test]
#[ignore = "three bakes of mover-setpiece (as authored, light_poses 76, light_poses 6); \
            cargo test -p postretro-level-compiler --bin prl-build \
            the_rising_rooms_ceiling_and_floor_stay_within_the_motion_bars_of_the_fine_bake -- \
            --ignored --nocapture"]
fn the_rising_rooms_ceiling_and_floor_stay_within_the_motion_bars_of_the_fine_bake() {
    let surfaces = [
        plane(
            "ceiling",
            2,
            -16.5,
            [860.0, 1124.0],
            [188.0, 452.0],
            [0.0, 0.0, -1.0],
        ),
        plane(
            "floor",
            2,
            -175.5,
            [860.0, 1124.0],
            [188.0, 452.0],
            [0.0, 0.0, 1.0],
        ),
    ];
    let rows = motion_proof(
        "mover-setpiece",
        "setpiece_room",
        76,
        &surfaces,
        [0.0, 0.0, 176.0],
    );
    assert_shipped_passes_and_pre_amendment_misses("mover-setpiece", &rows);
}

// The big rising room's ceiling underside and floor top, ridden by the room
// (inner box x 1536–1920, y 1360–1792; ceiling z −112, floor z −272 at t′ 0;
// 272 units up the trip), half a unit off each surface. Its 64-unit walls
// and slabs hold whole probe layers, where the small room's 16-unit slabs
// hold at most one: the thick-walled case. The truth bakes 98 poses, one per
// 2.8 units, near the small room's 2.3: on this lattice the densest count
// to 400 that parks no plane in a layer's band (the clean counts past 50 are
// 52, 56, 84 and 98).
#[test]
#[ignore = "three bakes of mover-setpiece's big room (as authored, light_poses 98, light_poses \
            8); cargo test -p postretro-level-compiler --bin prl-build \
            the_big_rising_rooms_ceiling_and_floor_stay_within_the_motion_bars_of_the_fine_bake \
            -- --ignored --nocapture"]
fn the_big_rising_rooms_ceiling_and_floor_stay_within_the_motion_bars_of_the_fine_bake() {
    let surfaces = [
        plane(
            "big ceiling",
            2,
            -112.5,
            [1548.0, 1908.0],
            [1372.0, 1780.0],
            [0.0, 0.0, -1.0],
        ),
        plane(
            "big floor",
            2,
            -271.5,
            [1548.0, 1908.0],
            [1372.0, 1780.0],
            [0.0, 0.0, 1.0],
        ),
    ];
    let rows = motion_proof(
        "mover-setpiece",
        "setpiece_room_big",
        98,
        &surfaces,
        [0.0, 0.0, 272.0],
    );
    assert_shipped_passes_and_pre_amendment_misses("mover-setpiece", &rows);
}

// em_sweeper's east and west faces (x 1312 and 1280 at t′ 0, 256 units east
// over the trip), half a unit off each face, over y 72–440 and z 8–88. The
// truth bakes 113 poses, one per 2.3 units, so the time-weighted share mean
// reads inside each geometry-knot window (owner, 2026-11-12).
#[test]
#[ignore = "three bakes of enemy-movers (as authored, light_poses 113, light_poses 8); \
            cargo test -p postretro-level-compiler --bin prl-build \
            the_sweepers_faces_stay_within_the_motion_bars_of_the_fine_bake -- --ignored \
            --nocapture"]
fn the_sweepers_faces_stay_within_the_motion_bars_of_the_fine_bake() {
    let surfaces = [
        plane(
            "east face",
            0,
            1312.5,
            [72.0, 440.0],
            [8.0, 88.0],
            [1.0, 0.0, 0.0],
        ),
        plane(
            "west face",
            0,
            1279.5,
            [72.0, 440.0],
            [8.0, 88.0],
            [-1.0, 0.0, 0.0],
        ),
    ];
    let rows = motion_proof(
        "enemy-movers",
        "em_sweeper",
        113,
        &surfaces,
        [256.0, 0.0, 0.0],
    );
    assert_shipped_passes_and_pre_amendment_misses("enemy-movers", &rows);
}
