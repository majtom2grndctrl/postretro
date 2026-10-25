// Quantitative contracts use accepted host samples, including loss gaps. No
// expected rate is taken from the client's deliberately mismatched descriptors.
use super::fixtures::{ClientFrame, IntakeTrace, NAMES, TimedSlotHarness};

const TOLERANCE: f64 = 0.08;
const ARRIVAL_ALLOWANCE_TICKS: f64 = 0.060 * 60.0 + 1.0;

#[derive(Default, Debug)]
pub(super) struct FrameCounts {
    pub(super) onset: usize,
    pub(super) between: usize,
    pub(super) sample_or_feed: usize,
    pub(super) breaks: usize,
    pub(super) capped: usize,
    pub(super) bound: usize,
    pub(super) raw: usize,
    pub(super) recovery: usize,
    pub(super) advances: usize,
}

pub(super) fn assert_playback_slew(harness: &TimedSlotHarness) {
    for pair in harness.frames.windows(2) {
        if let (Some(before), Some(after)) = (pair[0].playback_tick, pair[1].playback_tick) {
            let dt_ticks = (pair[1].now_us - pair[0].now_us) as f64 * 60.0 / 1_000_000.0;
            let advance = after - before;
            assert!(
                advance >= 0.75 * dt_ticks - 1e-9 && advance <= 1.25 * dt_ticks + 1e-9,
                "playback changed {advance} in {dt_ticks} real ticks at {}",
                pair[1].now_us
            );
        }
    }
}

// Only contiguous numeric samples of the same committed host identity belong
// to a ramp. Switching away and back, or an absence, cannot join old intervals.
fn accepted_samples(
    harness: &TimedSlotHarness,
    index: usize,
    now_us: u64,
    host_slot: Option<usize>,
) -> Vec<&IntakeTrace> {
    let mut samples: Vec<_> = harness
        .intakes
        .iter()
        .rev()
        .skip_while(|sample| sample.now_us > now_us)
        .take_while(|sample| sample.host_slots[index] == host_slot && sample.raw[index].is_some())
        .collect();
    samples.reverse();
    samples
}

fn host_interval_slope(samples: &[&IntakeTrace], index: usize) -> Option<f64> {
    let pair = samples.windows(2).last()?;
    Some((pair[1].raw[index]? - pair[0].raw[index]?) / f64::from(pair[1].tick - pair[0].tick))
}

// The oracle decides agreement from host observations before reading any
// implementation slope. A partial first interval must age out on its own.
fn host_agreed_slope(samples: &[&IntakeTrace], index: usize) -> Option<f64> {
    let triple = samples.windows(3).last()?;
    let previous = host_interval_slope(&triple[..2], index)?;
    let newest = host_interval_slope(&triple[1..], index)?;
    let direction = if index == 0 || index == 3 { 1.0 } else { -1.0 };
    (direction * previous > 0.0
        && direction * newest > 0.0
        && (newest - previous).abs() <= TOLERANCE * newest.abs().min(previous.abs()))
    .then_some(newest)
}

fn assert_sample_agreement(harness: &TimedSlotHarness, index: usize) {
    // Check each intake in order, even when several snapshots drain on one
    // frame. Later samples must not hide a delayed onset at the earlier sample.
    for (position, sample) in harness.intakes.iter().enumerate() {
        let mut samples: Vec<_> = harness.intakes[..=position]
            .iter()
            .rev()
            .take_while(|s| {
                s.host_slots[index] == sample.host_slots[index] && s.raw[index].is_some()
            })
            .collect();
        samples.reverse();
        let expected = host_agreed_slope(&samples, index);
        assert_eq!(sample.ramps[index].host_slot, sample.host_slots[index]);
        if sample.raw[index].is_some() {
            assert_eq!(sample.ramps[index].bound, sample.bounds[index]);
        }
        assert_eq!(
            sample.ramps[index].slope.is_some(),
            expected.is_some(),
            "{} must adopt agreement at the first qualifying host sample: {sample:?}",
            NAMES[index]
        );
        if let Some(expected) = expected {
            let actual = sample.ramps[index].slope.unwrap();
            assert!((actual - expected).abs() <= 1.0e-7 * expected.abs().max(1.0));
        }
    }
}

pub(super) fn assert_quantitative_frames(harness: &TimedSlotHarness) -> FrameCounts {
    assert_playback_slew(harness);
    let index = harness.scenario.meter();
    assert_sample_agreement(harness, index);
    let direction = if index == 0 || index == 3 { 1.0 } else { -1.0 };
    let mut counts = FrameCounts::default();
    for pair in harness.frames.windows(2) {
        let before = &pair[0];
        let frame = &pair[1];
        let old = before.ramps[index];
        let current = frame.ramps[index];
        let span = ramp_span(harness, index, frame);
        let epsilon = span * 1.0e-4;
        let (Some(previous_target), Some(target)) = (before.targets[index], frame.targets[index])
        else {
            continue;
        };
        let bound = frame.bounds[index];
        assert!(target >= -epsilon, "{} below zero: {frame:?}", NAMES[index]);
        if index == 0 || index == 3 {
            assert!(
                target <= bound.unwrap_or(span) + epsilon,
                "{} above committed host bound: {frame:?}",
                NAMES[index]
            );
        }
        assert!(
            current
                .target
                .is_none_or(|debug| (debug - target).abs() <= epsilon),
            "UI snapshot differs from target"
        );
        let change = direction * (target - previous_target);
        let sample_window = accepted_samples(harness, index, frame.now_us, frame.host_slots[index]);
        let old_samples = accepted_samples(harness, index, before.now_us, before.host_slots[index]);
        let agreed = host_agreed_slope(&sample_window, index);
        let old_agreed = host_agreed_slope(&old_samples, index);
        assert_eq!(
            current.slope.is_some(),
            agreed.is_some(),
            "{} frame agreement differs from accepted host intervals: {frame:?}",
            NAMES[index]
        );
        let applies = frame
            .applied_ticks
            .iter()
            .any(|tick| sample_window.iter().any(|sample| sample.tick == *tick));
        let feeds = current.fed_tick != old.fed_tick;
        if frame.playback_tick.is_none() {
            assert_eq!(
                frame.targets[index], frame.raw[index],
                "before sync, show raw"
            );
            counts.raw += 1;
            continue;
        }
        if frame.host_slots[index] != before.host_slots[index] {
            assert_eq!(
                frame.targets[index], frame.raw[index],
                "new identity must show raw"
            );
            counts.raw += 1;
            continue;
        }
        if before.playback_tick.is_none() {
            if agreed.is_some() && bound.is_some() {
                assert!(
                    change >= -epsilon,
                    "first sync retreated past shown raw sample"
                );
            }
            counts.raw += 1;
            continue;
        }
        let newest = sample_window.last().copied();
        let at_bound = bound.is_some_and(|bound| (target - bound).abs() <= epsilon);
        let bound_sample_applied = newest.is_some_and(|sample| {
            frame.applied_ticks.contains(&sample.tick)
                && bound.is_some_and(|bound| {
                    (sample.raw[index].unwrap() - bound).abs() <= 1.0e-7 * span.max(1.0)
                })
        });
        // Only an independently observed bound sample permits an immediate
        // snap. Reaching the bound by projection keeps the normal frame budget.
        if bound_sample_applied {
            assert!(
                at_bound,
                "accepted bound sample must set the target this frame"
            );
            counts.bound += 1;
            continue;
        }
        let interval = sample_window
            .windows(2)
            .last()
            .map_or(0.0, |s| f64::from(s[1].tick - s[0].tick));
        let slope = agreed
            .or(old_agreed)
            .or(host_interval_slope(&sample_window, index))
            .unwrap_or(0.0)
            .abs();
        let dt_ticks = (frame.now_us - before.now_us) as f64 * 60.0 / 1_000_000.0;
        let base = slope * (1.0 + TOLERANCE) * 1.25 * dt_ticks + epsilon;
        let step = slope * (1.0 + TOLERANCE) * (interval + ARRIVAL_ALLOWANCE_TICKS) + epsilon;
        let old_cap_expired = before
            .playback_tick
            .zip(old_samples.last())
            .is_some_and(|(timeline, sample)| timeline > f64::from(sample.tick) + 16.0);
        let cap_expired = frame
            .playback_tick
            .zip(newest)
            .is_some_and(|(timeline, sample)| timeline > f64::from(sample.tick) + 16.0);
        if old_cap_expired && applies {
            counts.recovery += 1;
            if agreed.is_none() {
                assert_eq!(frame.targets[index], frame.raw[index]);
            }
            continue;
        }
        if agreed.is_none() || bound.is_none() {
            assert_eq!(
                frame.targets[index], frame.raw[index],
                "unagreed or unbounded sample must be raw: {frame:?}"
            );
            if at_bound {
                counts.bound += 1;
            } else if old_agreed.is_some() && applies {
                counts.breaks += 1;
                if harness.scenario == super::fixtures::Scenario::SlowingHeat {
                    assert!(
                        change >= -step,
                        "slope break retreat {change} exceeds {step}: {frame:?}"
                    );
                }
            } else {
                counts.raw += 1;
            }
        } else if old_agreed.is_none() || before.bounds[index].is_none() {
            counts.onset += 1;
            assert!(
                current.fed_tick.is_some(),
                "first agreeing frame failed to establish projection"
            );
            assert!(
                change >= -epsilon && change <= step,
                "onset {change} exceeds [0,{step}]: {frame:?}"
            );
        } else if cap_expired {
            counts.capped += 1;
            if old_cap_expired && !applies {
                assert!(change.abs() <= epsilon, "expired cap moved");
            }
        } else if applies || feeds {
            counts.sample_or_feed += 1;
            let fed_interval = current
                .fed_tick
                .and_then(|tick| {
                    sample_window
                        .windows(2)
                        .find(|s| f64::from(s[1].tick) == tick)
                        .map(|s| f64::from(s[1].tick - s[0].tick))
                })
                .unwrap_or(interval);
            let limit =
                base + TOLERANCE * slope * (fed_interval.max(interval) + ARRIVAL_ALLOWANCE_TICKS);
            assert!(
                change >= -epsilon && change <= limit,
                "sample/feed {change} exceeds [0,{limit}]: {frame:?}"
            );
        } else {
            if at_bound {
                counts.bound += 1;
            } else {
                counts.between += 1;
            }
            assert!(
                change >= -epsilon && change <= base,
                "between-sample {change} exceeds [0,{base}]: {frame:?}"
            );
            if change <= epsilon * 0.01 && !at_bound {
                let fed_tick = current.fed_tick.expect("an agreeing hold has a fed line");
                let fed = sample_window
                    .windows(2)
                    .find(|samples| f64::from(samples[1].tick) == fed_tick)
                    .expect("fed line must name an independently observed accepted host sample");
                let value = fed[1].raw[index].unwrap();
                let host_rate =
                    (value - fed[0].raw[index].unwrap()) / f64::from(fed[1].tick - fed[0].tick);
                let projected_tick = frame
                    .playback_tick
                    .unwrap()
                    .min(f64::from(newest.unwrap().tick) + 16.0);
                let projected = value + host_rate * (projected_tick - fed_tick).max(0.0);
                let projected = match bound {
                    Some(bound) if direction > 0.0 => projected.min(bound),
                    Some(bound) => projected.max(bound),
                    None => unreachable!("bounded ramp checked above"),
                };
                assert!(
                    direction * (projected - previous_target) <= epsilon,
                    "{} held although accepted host line advanced ahead: predicted {projected}, previous {previous_target}, frame {frame:?}",
                    NAMES[index]
                );
            }
            if change > epsilon * 0.01 {
                counts.advances += 1;
            }
        }
    }
    counts
}

/// On a jittered jump scenario, extrapolation before the discontinuity is
/// allowed to be wrong by at most one sample interval plus transit allowance.
/// Derive that budget from the two latest accepted samples, not local tuning.
pub(super) fn assert_error_within_newest_sample_step(harness: &TimedSlotHarness) {
    let index = harness.scenario.meter();
    let mut checked = 0;
    for frame in &harness.frames {
        let span = ramp_span(harness, index, frame);
        let accepted: Vec<_> = harness
            .intakes
            .iter()
            .filter(|sample| {
                sample.now_us <= frame.now_us
                    && sample.host_slots[index] == frame.host_slots[index]
                    && sample.raw[index].is_some()
            })
            .collect();
        let Some(pair) = accepted.windows(2).last() else {
            continue;
        };
        let Some(target) = frame.targets[index] else {
            continue;
        };
        let interval = f64::from(pair[1].tick - pair[0].tick);
        let newest = pair[1].raw[index].unwrap();
        let slope = (newest - pair[0].raw[index].unwrap()) / interval;
        let step =
            slope.abs() * (1.0 + TOLERANCE) * (interval + ARRIVAL_ALLOWANCE_TICKS) + span * 1e-4;
        assert!(
            (target - newest).abs() <= step,
            "{} projection error {} exceeds newest-sample step {step} at frame {frame:?}",
            NAMES[index],
            (target - newest).abs()
        );
        checked += 1;
    }
    assert!(checked > 100, "insufficient pre-jump error observations");
}

// The acceptance budget is in units of this host ramp, including a threshold
// changed by a real descriptor refresh. Never substitute installed client tuning.
fn ramp_span(harness: &TimedSlotHarness, index: usize, frame: &ClientFrame) -> f64 {
    match index {
        0 => 1.0,
        1 => {
            let mut first = 0.0;
            let mut previous = None;
            for value in harness
                .intakes
                .iter()
                .filter(|sample| {
                    sample.now_us <= frame.now_us
                        && sample.host_slots[index] == frame.host_slots[index]
                })
                .filter_map(|sample| sample.raw[index])
            {
                if previous.is_none_or(|last| value >= last) {
                    first = value;
                }
                previous = Some(value);
            }
            first
        }
        2 => frame.overheat_at.unwrap_or(0.0),
        3 => frame.bounds[index].unwrap_or(0.0),
        _ => unreachable!(),
    }
}
