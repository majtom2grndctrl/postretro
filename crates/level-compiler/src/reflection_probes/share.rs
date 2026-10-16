// Reflection-probe light share: each animated light's fraction of a probe's captured light, per lobe.
// See: context/lib/resource_management.md §5.2 · rendering_pipeline.md §7.3a (Live lighting)

use glam::Vec3;
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_LIGHTS_PER_PROBE, REFLECTION_PROBE_LOBE_COUNT, ReflectionProbeLightShare,
    reflection_probe_lobe_weights,
};

const LOBES: usize = REFLECTION_PROBE_LOBE_COUNT;
const K: usize = REFLECTION_PROBE_LIGHTS_PER_PROBE;

type LobeSums = [[f64; 3]; LOBES];

/// Lobe-weighted, solid-angle-weighted radiance sums over a probe's captured
/// samples in the upper hemisphere: the captured total, and each animated
/// light's part of it at its reference value. Every runtime lookup is clamped
/// to the horizon, so light only the lower hemisphere holds (a pool floor
/// below the capture point) never reaches the water and takes no share; the
/// tile itself still holds the full sphere. Rows sum independently and merge
/// in row order, so the result never depends on the worker count; no
/// per-texel per-light value is ever held.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ShareSums {
    captured: LobeSums,
    /// By ascending roster index.
    lights: Vec<(u32, LobeSums)>,
}

/// A probe's stored entries and what its log reports.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProbeShares {
    pub entries: [ReflectionProbeLightShare; K],
    pub report: ShareReport,
}

/// The probe's covered share for the log: how many animated lights the
/// capture saw, the fraction of their captured light the stored entries
/// hold, and each stored entry's fraction of the probe's whole captured light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ShareReport {
    pub animated_lights: u32,
    pub covered: f32,
    pub totals: [f32; K],
}

impl ShareReport {
    pub(crate) const BYTES: usize = 4 + 4 + 4 * K;

    pub(crate) fn to_bytes(self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.animated_lights.to_le_bytes());
        out.extend_from_slice(&self.covered.to_le_bytes());
        for total in self.totals {
            out.extend_from_slice(&total.to_le_bytes());
        }
    }

    pub(crate) fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::BYTES {
            return None;
        }
        let word = |i: usize| <[u8; 4]>::try_from(&bytes[i * 4..i * 4 + 4]).ok();
        let mut totals = [0.0; K];
        for (k, total) in totals.iter_mut().enumerate() {
            *total = f32::from_le_bytes(word(2 + k)?);
        }
        Some(Self {
            animated_lights: u32::from_le_bytes(word(0)?),
            covered: f32::from_le_bytes(word(1)?),
            totals,
        })
    }
}

impl ShareSums {
    /// One captured sample in `direction`, standing for `solid_angle`: its
    /// captured radiance and each animated light's unit term, which counts at
    /// the light's reference value. A sample below the horizon adds nothing.
    pub(crate) fn add(
        &mut self,
        direction: Vec3,
        solid_angle: f32,
        capture: Vec3,
        animated: &[(u32, Vec3)],
        references: &[[f32; 3]],
    ) {
        if direction.y < 0.0 {
            return;
        }
        let lobes = reflection_probe_lobe_weights(direction.to_array());
        let weights = lobes.map(|w| f64::from(w) * f64::from(solid_angle));
        accumulate(&mut self.captured, &weights, capture);
        for &(light, unit) in animated {
            let lit = unit * Vec3::from(reference(references, light));
            let at = match self.lights.binary_search_by_key(&light, |(l, _)| *l) {
                Ok(at) => at,
                Err(at) => {
                    self.lights.insert(at, (light, [[0.0; 3]; LOBES]));
                    at
                }
            };
            accumulate(&mut self.lights[at].1, &weights, lit);
        }
    }

    /// Add `other`'s sums; callers merge rows in row order.
    pub(crate) fn merge(&mut self, other: &ShareSums) {
        add_sums(&mut self.captured, &other.captured);
        for (light, sums) in &other.lights {
            match self.lights.binary_search_by_key(light, |(l, _)| *l) {
                Ok(at) => add_sums(&mut self.lights[at].1, sums),
                Err(at) => self.lights.insert(at, (*light, *sums)),
            }
        }
    }

    /// The K lights of largest total contribution (their lobe-weighted
    /// captured radiance summed over lobes and channels: the light's captured
    /// energy over the upper hemisphere), ties to the lower roster index, each
    /// with its per-lobe per-channel fraction of the captured light. A lobe
    /// that captured nothing in a channel stores 0 there. A light whose
    /// reference has a negative channel is never stored, since the section
    /// refuses a negative reference: it stays at reference, so the ratio is
    /// unaffected, and counts toward the uncovered share. The stage logs such
    /// lights once, through [`log_negative_references`].
    pub(crate) fn finish(&self, references: &[[f32; 3]]) -> ProbeShares {
        let measure = |sums: &LobeSums| sums.iter().flatten().sum::<f64>();
        let mut ranked: Vec<(u32, &LobeSums, f64)> = self
            .lights
            .iter()
            .map(|(light, sums)| (*light, sums, measure(sums)))
            .filter(|(_, _, m)| *m > 0.0)
            .collect();
        ranked.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));
        let animated_total: f64 = ranked.iter().map(|r| r.2).sum();
        let captured_total = measure(&self.captured);
        let storable = |light: u32| !negative(reference(references, light));

        let mut entries = [ReflectionProbeLightShare::UNUSED; K];
        let mut totals = [0.0f32; K];
        let mut kept = 0.0f64;
        let stored = ranked.iter().filter(|(light, ..)| storable(*light)).take(K);
        for (k, (light, sums, m)) in stored.enumerate() {
            let mut shares = [[0.0f32; 3]; LOBES];
            for (lobe, share) in shares.iter_mut().enumerate() {
                for channel in 0..3 {
                    let captured = self.captured[lobe][channel];
                    if captured > 0.0 {
                        share[channel] = (sums[lobe][channel] / captured).clamp(0.0, 1.0) as f32;
                    }
                }
            }
            entries[k] = ReflectionProbeLightShare {
                roster_index: *light,
                reference: reference(references, *light),
                shares,
            };
            totals[k] = if captured_total > 0.0 {
                (m / captured_total) as f32
            } else {
                0.0
            };
            kept += m;
        }
        ProbeShares {
            entries,
            report: ShareReport {
                animated_lights: ranked.len() as u32,
                covered: if animated_total > 0.0 {
                    (kept / animated_total) as f32
                } else {
                    1.0
                },
                totals,
            },
        }
    }
}

/// Log each animated light whose reference has a negative channel, once per
/// stage, in roster order. No probe stores such a light (see
/// [`ShareSums::finish`]), so this covers every probe and every rank.
pub(crate) fn log_negative_references(references: &[[f32; 3]]) {
    for (light, value) in references.iter().enumerate() {
        if negative(*value) {
            log::info!(
                "[ReflectionProbes] animated light {light} has a negative reference {value:?}; \
                 no probe stores it, so it stays at reference in reflections"
            );
        }
    }
}

fn negative(reference: [f32; 3]) -> bool {
    reference.iter().any(|c| *c < 0.0)
}

/// A light past the reference table adds nothing; the stage checks every
/// baked light index against the roster before capture.
fn reference(references: &[[f32; 3]], light: u32) -> [f32; 3] {
    references.get(light as usize).copied().unwrap_or([0.0; 3])
}

fn accumulate(sums: &mut LobeSums, weights: &[f64; LOBES], value: Vec3) {
    for (lobe, weight) in weights.iter().enumerate() {
        if *weight == 0.0 {
            continue;
        }
        for channel in 0..3 {
            sums[lobe][channel] += weight * f64::from(value[channel]);
        }
    }
}

fn add_sums(into: &mut LobeSums, from: &LobeSums) {
    for (a, b) in into.iter_mut().zip(from) {
        for (x, y) in a.iter_mut().zip(b) {
            *x += y;
        }
    }
}

/// The solid angle one sample at octahedral `(u, v)` in `[0, 1]²` stands for,
/// when an interior tile of `interior` texels per edge takes `samples` per
/// texel. The octahedral map sends the square onto the octahedron
/// `|x| + |y| + |z| = 1` with unit area scale in both folds, and a surface
/// point `p` subtends `dA (p · n̂) / |p|³` from the origin; on the octahedron
/// `p · (±1, ±1, ±1) = 1`, so the density is exactly `1 / |p|³` per unit of
/// the `[−1, 1]²` square. The sample's share of its texel's area is
/// `(2 / interior)² / samples`.
pub(crate) fn sample_solid_angle(u: f32, v: f32, interior: u32, samples: usize) -> f32 {
    let (ox, oy) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
    let z = 1.0 - ox.abs() - oy.abs();
    let (x, y) = if z < 0.0 {
        (
            (1.0 - oy.abs()) * ox.signum(),
            (1.0 - ox.abs()) * oy.signum(),
        )
    } else {
        (ox, oy)
    };
    let length = Vec3::new(x, y, z).length();
    let area = (2.0 / interior as f32).powi(2) / samples as f32;
    area / (length * length * length)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflection_probes::capture::{SUPERSAMPLE_OFFSETS, TILE_INTERIOR};

    /// The pinned Ω method integrates to the whole sphere over the capture's
    /// own sample pattern.
    #[test]
    fn sample_solid_angles_cover_the_sphere() {
        let interior = TILE_INTERIOR;
        let mut total = 0.0f64;
        for row in 0..interior {
            for column in 0..interior {
                for [dx, dy] in SUPERSAMPLE_OFFSETS {
                    let u = (column as f32 + 0.5 + dx) / interior as f32;
                    let v = (row as f32 + 0.5 + dy) / interior as f32;
                    total += f64::from(sample_solid_angle(
                        u,
                        v,
                        interior,
                        SUPERSAMPLE_OFFSETS.len(),
                    ));
                }
            }
        }
        let sphere = 4.0 * std::f64::consts::PI;
        assert!(
            (total - sphere).abs() / sphere < 1e-4,
            "{total} vs {sphere}"
        );
    }

    /// A sample: direction, captured radiance, animated unit terms.
    type Sample = (Vec3, Vec3, Vec<(u32, Vec3)>);

    fn sums_of(samples: &[Sample], references: &[[f32; 3]]) -> ShareSums {
        let mut sums = ShareSums::default();
        for (direction, capture, animated) in samples {
            sums.add(*direction, 1.0, *capture, animated, references);
        }
        sums
    }

    #[test]
    fn shares_are_each_lights_fraction_of_the_captured_light_per_lobe() {
        let references = [[2.0, 1.0, 0.0], [1.0, 1.0, 1.0]];
        // Straight up: light 0's unit term at reference (2, 1, 0) over a
        // static (1, 1, 1); +X: light 1 alone at (1, 1, 1).
        let up_unit = Vec3::new(0.5, 0.5, 0.5);
        let x_unit = Vec3::splat(1.0);
        let samples = vec![
            (
                Vec3::Y,
                Vec3::ONE + up_unit * Vec3::new(2.0, 1.0, 0.0),
                vec![(0, up_unit)],
            ),
            (Vec3::X, x_unit, vec![(1, x_unit)]),
        ];
        let shares = sums_of(&samples, &references).finish(&references);
        let a = shares.entries.iter().find(|e| e.roster_index == 0).unwrap();
        assert_eq!(a.reference, references[0]);
        assert!((a.shares[0][0] - 0.5).abs() < 1e-6, "{a:?}");
        assert!((a.shares[0][1] - 1.0 / 3.0).abs() < 1e-6, "{a:?}");
        assert_eq!(a.shares[0][2], 0.0);
        assert_eq!(a.shares[1], [0.0; 3]);
        let b = shares.entries.iter().find(|e| e.roster_index == 1).unwrap();
        assert_eq!(b.shares[1], [1.0; 3]);
        // −X captured nothing: every share there is 0, never NaN (P20).
        assert_eq!(b.shares[2], [0.0; 3]);
        assert!(
            shares
                .entries
                .iter()
                .flat_map(|e| e.shares)
                .flatten()
                .all(|v| v.is_finite())
        );
        assert_eq!(shares.report.animated_lights, 2);
        assert_eq!(shares.report.covered, 1.0);
    }

    // P6 and P22: past K lights the smallest stays out; at a tie for the
    // K-th entry the lower roster index is stored.
    #[test]
    fn top_k_keeps_the_largest_lights_and_breaks_ties_by_roster_index() {
        let references = [[1.0; 3]; 6];
        let units = [(4, 5.0), (0, 3.0), (5, 1.0), (1, 4.0), (3, 2.0), (2, 2.0)];
        let animated: Vec<(u32, Vec3)> = units.iter().map(|&(l, v)| (l, Vec3::splat(v))).collect();
        let capture = Vec3::splat(units.iter().map(|u| u.1).sum::<f32>());
        let shares = sums_of(&[(Vec3::Y, capture, animated)], &references).finish(&references);
        let stored: Vec<u32> = shares.entries.iter().map(|e| e.roster_index).collect();
        // 4 (5), 1 (4), 0 (3), then the tie at 2 between 2 and 3: 2 wins.
        assert_eq!(stored, [4, 1, 0, 2]);
        assert_eq!(shares.report.animated_lights, 6);
        assert!(
            (shares.report.covered - 14.0 / 17.0).abs() < 1e-6,
            "{:?}",
            shares.report
        );
        assert!((shares.report.totals[0] - 5.0 / 17.0).abs() < 1e-6);
    }

    // Row 14: the water reads only the upper hemisphere, so light below the
    // horizon (a lit pool floor) takes no share and no slot, and adds nothing
    // to a horizontal lobe's captured total.
    #[test]
    fn samples_below_the_horizon_count_toward_no_share_or_slot() {
        let references = [[1.0; 3], [1.0; 3]];
        let below = Vec3::new(1.0, -0.2, 0.0).normalize();
        let samples = vec![
            (Vec3::NEG_Y, Vec3::splat(4.0), vec![(0, Vec3::splat(4.0))]),
            (below, Vec3::splat(4.0), vec![(0, Vec3::splat(4.0))]),
            (Vec3::X, Vec3::ONE, vec![(1, Vec3::ONE)]),
        ];
        let shares = sums_of(&samples, &references).finish(&references);
        let stored: Vec<u32> = shares
            .entries
            .iter()
            .filter(|e| e.is_used())
            .map(|e| e.roster_index)
            .collect();
        assert_eq!(stored, [1]);
        // Light 1 is all the +X lobe captured above the horizon.
        assert_eq!(shares.entries[0].shares[1], [1.0; 3]);
        assert_eq!(shares.report.animated_lights, 1);
        assert_eq!(shares.report.covered, 1.0);
    }

    // The section refuses a negative reference: such a light is never
    // stored, stays at reference, and counts as uncovered.
    #[test]
    fn a_light_with_a_negative_reference_channel_is_never_stored() {
        let references = [[1.0, -0.5, 1.0], [1.0; 3]];
        let animated = vec![(0, Vec3::splat(2.0)), (1, Vec3::ONE)];
        let shares =
            sums_of(&[(Vec3::Y, Vec3::splat(3.0), animated)], &references).finish(&references);
        let stored: Vec<u32> = shares
            .entries
            .iter()
            .filter(|e| e.is_used())
            .map(|e| e.roster_index)
            .collect();
        assert_eq!(stored, [1]);
        assert!(
            shares
                .entries
                .iter()
                .all(|e| e.reference.iter().all(|c| *c >= 0.0))
        );
        assert_eq!(shares.report.animated_lights, 2);
        assert!(
            (shares.report.covered - 0.5).abs() < 1e-6,
            "{:?}",
            shares.report
        );
    }

    // Every negative-reference light is logged once, in roster order, the
    // one ranked past the K stored included; ranking itself logs nothing.
    #[test]
    fn negative_reference_lights_log_once_each_in_roster_order() {
        use postretro_test_log_capture::LogCapture;

        let mut references = [[1.0f32; 3]; K + 2];
        references[1] = [-0.25, 1.0, 1.0];
        references[K + 1] = [1.0, 1.0, -1.0];
        // Lower roster indices contribute more, so light K + 1 ranks last.
        let animated: Vec<(u32, Vec3)> = (0..K as u32 + 2)
            .map(|light| (light, Vec3::splat(10.0 - light as f32)))
            .collect();
        let capture = LogCapture::start();
        let sums = sums_of(&[(Vec3::Y, Vec3::splat(100.0), animated)], &references);
        for _ in 0..3 {
            sums.finish(&references);
        }
        let negative_logs = || -> Vec<String> {
            capture
                .records()
                .into_iter()
                .filter(|r| r.message.contains("negative reference"))
                .map(|r| r.message)
                .collect()
        };
        assert_eq!(
            negative_logs(),
            Vec::<String>::new(),
            "ranking logs nothing"
        );
        log_negative_references(&references);
        let logged = negative_logs();
        assert_eq!(logged.len(), 2, "{logged:?}");
        assert!(logged[0].contains("animated light 1 "), "{logged:?}");
        assert!(
            logged[1].contains(&format!("animated light {} ", K + 1)),
            "{logged:?}"
        );
    }

    #[test]
    fn a_probe_with_no_animated_light_stores_only_unused_entries() {
        let shares = sums_of(&[(Vec3::Y, Vec3::ONE, Vec::new())], &[]).finish(&[]);
        assert_eq!(shares.entries, [ReflectionProbeLightShare::UNUSED; K]);
        assert_eq!(shares.report.animated_lights, 0);
    }

    #[test]
    fn merging_rows_equals_one_pass_over_their_samples() {
        let references = [[1.0, 0.5, 0.25]; 3];
        let samples: Vec<Sample> = (0..12)
            .map(|i| {
                let d = Vec3::new((i as f32).sin(), (i as f32 * 0.7).cos(), 0.3).normalize();
                let animated = vec![((i % 3) as u32, Vec3::splat(0.1 * i as f32))];
                (d, Vec3::splat(1.0 + i as f32), animated)
            })
            .collect();
        let whole = sums_of(&samples, &references);
        let mut merged = ShareSums::default();
        for chunk in samples.chunks(5) {
            merged.merge(&sums_of(chunk, &references));
        }
        let (a, b) = (whole.finish(&references), merged.finish(&references));
        for (x, y) in a.entries.iter().zip(&b.entries) {
            assert_eq!(x.roster_index, y.roster_index);
            for (s, t) in x.shares.iter().flatten().zip(y.shares.iter().flatten()) {
                assert!((s - t).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn share_report_round_trips_its_bytes() {
        let report = ShareReport {
            animated_lights: 7,
            covered: 0.875,
            totals: [0.5, 0.25, 0.125, 0.0],
        };
        let mut bytes = Vec::new();
        report.to_bytes(&mut bytes);
        assert_eq!(ShareReport::from_bytes(&bytes), Some(report));
        assert_eq!(ShareReport::from_bytes(&bytes[1..]), None);
    }
}
