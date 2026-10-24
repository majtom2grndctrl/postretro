// Quantitative contracts use accepted host samples, including loss gaps. No
// expected rate is taken from the client's deliberately mismatched descriptors.
use super::fixtures::{ClientFrame, NAMES, TimedSlotHarness};

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

pub(super) fn assert_quantitative_frames(harness: &TimedSlotHarness) -> FrameCounts {
    assert_playback_slew(harness);
    let index = harness.scenario.meter();
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
        assert!(target >= -epsilon, "{} below zero: {frame:?}", NAMES[index]);
        if index == 0 || index == 3 {
            assert!(
                target <= current.bound.unwrap_or(span) + epsilon,
                "{} above bound: {frame:?}",
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
        let applies = current.samples != old.samples;
        let feeds = current.fed_tick != old.fed_tick;
        if frame.playback_tick.is_none() {
            assert_eq!(
                frame.targets[index], frame.raw[index],
                "before sync, show raw"
            );
            counts.raw += 1;
            continue;
        }
        if current.host_slot != old.host_slot || before.playback_tick.is_none() {
            counts.raw += 1;
            continue;
        }
        if current
            .bound
            .is_some_and(|bound| (target - bound).abs() < epsilon)
        {
            counts.bound += 1;
            continue;
        }
        let sample_window: Vec<_> = harness
            .intakes
            .iter()
            .filter(|s| {
                s.now_us <= frame.now_us
                    && s.ramps[index].host_slot == current.host_slot
                    && s.raw[index].is_some()
            })
            .collect();
        let interval = sample_window
            .windows(2)
            .last()
            .map_or(0.0, |s| f64::from(s[1].tick - s[0].tick));
        let host_slope = sample_window.windows(2).last().map(|s| {
            (s[1].raw[index].unwrap() - s[0].raw[index].unwrap()) / f64::from(s[1].tick - s[0].tick)
        });
        if let Some(slope) = current.slope {
            assert!(
                direction * slope > 0.0,
                "projecting against timer direction"
            );
            assert!(
                (slope - host_slope.unwrap()).abs() <= 1.0e-7 * span,
                "{} derived {slope} from something other than host samples {:?}",
                NAMES[index],
                host_slope
            );
        }
        let slope = current
            .slope
            .or(old.slope)
            .or(host_slope)
            .unwrap_or(0.0)
            .abs();
        let dt_ticks = (frame.now_us - before.now_us) as f64 * 60.0 / 1_000_000.0;
        let base = slope * (1.0 + TOLERANCE) * 1.25 * dt_ticks + epsilon;
        let step = slope * (1.0 + TOLERANCE) * (interval + ARRIVAL_ALLOWANCE_TICKS) + epsilon;
        if old.cap_expired && applies {
            counts.recovery += 1;
            if current.slope.is_none() {
                assert_eq!(frame.targets[index], frame.raw[index]);
            }
            continue;
        }
        if current.slope.is_none() {
            assert_eq!(
                frame.targets[index], frame.raw[index],
                "unagreed sample must be raw: {frame:?}"
            );
            if old.slope.is_some() && applies {
                counts.breaks += 1;
                // A smooth slope change may retreat by at most one interval
                // plus arrival allowance. Discrete jumps are checked separately.
                if harness.scenario == super::fixtures::Scenario::SlowingHeat {
                    assert!(
                        change >= -step,
                        "slope break retreat {change} exceeds {step}: {frame:?}"
                    );
                }
            } else {
                counts.raw += 1;
            }
        } else if old.slope.is_none() {
            counts.onset += 1;
            assert!(
                change >= -epsilon && change <= step,
                "onset {change} exceeds [0,{step}]: {frame:?}"
            );
        } else if current.cap_expired {
            counts.capped += 1;
            if old.cap_expired && !applies {
                assert!(change.abs() <= epsilon, "expired cap moved");
            }
        } else if applies || feeds {
            counts.sample_or_feed += 1;
            // A pending sample can feed before a newer applied sample. Use its
            // own interval for the rounding/slope tolerance budget.
            let fed_interval = current
                .fed_tick
                .and_then(|tick| {
                    sample_window
                        .windows(2)
                        .find(|s| f64::from(s[1].tick) == tick)
                        .map(|s| f64::from(s[1].tick - s[0].tick))
                })
                .unwrap_or(interval);
            let bound =
                base + TOLERANCE * slope * (fed_interval.max(interval) + ARRIVAL_ALLOWANCE_TICKS);
            assert!(
                change >= -epsilon && change <= bound,
                "sample/feed {change} exceeds [0,{bound}]: {frame:?}"
            );
        } else {
            counts.between += 1;
            assert!(
                change >= -epsilon && change <= base,
                "between-sample {change} exceeds [0,{base}]: {frame:?}"
            );
            if change <= epsilon * 0.01 {
                let fed_tick = current.fed_tick.expect("an agreeing hold has a fed line");
                let fed = sample_window
                    .windows(2)
                    .find(|samples| f64::from(samples[1].tick) == fed_tick)
                    .expect(
                        "the fed line must name an independently observed accepted host sample",
                    );
                let value = fed[1].raw[index].unwrap();
                let host_rate =
                    (value - fed[0].raw[index].unwrap()) / f64::from(fed[1].tick - fed[0].tick);
                let projected_tick = frame
                    .playback_tick
                    .unwrap()
                    .min(current.newest_tick.unwrap() + 16.0);
                let projected = value + host_rate * (projected_tick - fed_tick).max(0.0);
                let projected = if direction > 0.0 {
                    projected.min(current.bound.unwrap())
                } else {
                    projected.max(current.bound.unwrap())
                };
                assert!(
                    direction * (projected - previous_target) <= epsilon,
                    "{} held although the accepted host line advanced ahead: predicted {projected}, previous {previous_target}, frame {frame:?}",
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
                    && sample.ramps[index].host_slot == frame.ramps[index].host_slot
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
                        && sample.ramps[index].host_slot == frame.ramps[index].host_slot
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
        3 => frame.ramps[index].bound.unwrap_or(0.0),
        _ => unreachable!(),
    }
}
