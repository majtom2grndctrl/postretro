//! Per-frame trip records and the residency gate's paired-run split.
//! See: context/lib/rendering_pipeline.md §4

// A pre-change run writes, per frame, whether its portal walk took the
// step-limit fallback, plus the pool and miss facts the gate compares. A
// later run on another build reads it and splits its frames (O28, O38):
// the recorded run's trip frames, and the frames after one until a pool
// growth begun on it, in either run, has retired, leave the gate. Frame i
// must be the same pose in both runs; a pair whose poses or locate
// mismatches differ is invalid.

use std::fmt::Write as _;
use std::io::Write as _;

use glam::Vec3;
use postretro_visibility::VisibilityPath;

use super::frame_driver::FrameOutcome;

const MAGIC: &str = "postretro-lightmap-walk-trip-record 1";

/// One frame of a run, after its last drain.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct FrameRecord {
    /// The pose: step, walk cell, eye, yaw, pitch and projection.
    pub(super) fingerprint: u64,
    /// The spawn step's frames, reported apart from the gate.
    pub(super) spawn: bool,
    /// The walk took `VisibilityPath::PortalStepLimitFallback`.
    pub(super) tripped: bool,
    pub(super) grew: bool,
    pub(super) retiring_at_start: bool,
    pub(super) deferred: u32,
    pub(super) left_behind: u32,
    pub(super) outside: u32,
}

impl FrameRecord {
    /// Frame `outcome`, whose walk took `path`, at the pose `fingerprint`.
    pub(super) fn of(
        fingerprint: u64,
        spawn: bool,
        path: VisibilityPath,
        outcome: &FrameOutcome,
    ) -> Self {
        Self {
            fingerprint,
            spawn,
            tripped: matches!(path, VisibilityPath::PortalStepLimitFallback { .. }),
            grew: outcome.grew,
            retiring_at_start: outcome.retiring_at_start,
            deferred: outcome.deferred,
            left_behind: outcome.left_behind,
            outside: outcome.outside,
        }
    }
}

/// Fingerprint of frame `i`'s pose. Bit-exact: both builds draw eyes from one
/// seeded stream over one cell layout.
pub(super) fn pose_fingerprint(
    step: usize,
    cell: u32,
    eye: Vec3,
    yaw: f32,
    pitch: f32,
    projection: (f32, f32),
) -> u64 {
    let mut bytes = Vec::with_capacity(40);
    bytes.extend((step as u64).to_le_bytes());
    bytes.extend(cell.to_le_bytes());
    for value in [eye.x, eye.y, eye.z, yaw, pitch, projection.0, projection.1] {
        bytes.extend(value.to_bits().to_le_bytes());
    }
    let hash = blake3::hash(&bytes);
    u64::from_le_bytes(hash.as_bytes()[..8].try_into().expect("8 bytes"))
}

/// One run: a walk at one set of levers and modes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct RunRecord {
    /// Walk, levers and modes; a pair matches on it.
    pub(super) label: String,
    /// Which PRL and id 51 the run read (O14).
    pub(super) bake: String,
    pub(super) locate_mismatches: u64,
    pub(super) frames: Vec<FrameRecord>,
}

pub(super) fn write_records(path: &str, runs: &[RunRecord]) -> std::io::Result<()> {
    let mut text = String::new();
    writeln!(text, "{MAGIC}").expect("string write");
    for run in runs {
        writeln!(text, "run {}", run.label).expect("string write");
        writeln!(text, "bake {}", run.bake).expect("string write");
        writeln!(text, "locate_mismatches {}", run.locate_mismatches).expect("string write");
        writeln!(text, "frames {}", run.frames.len()).expect("string write");
        for f in &run.frames {
            writeln!(
                text,
                "{:016x} {} {} {} {} {} {} {}",
                f.fingerprint,
                u8::from(f.spawn),
                u8::from(f.tripped),
                u8::from(f.grew),
                u8::from(f.retiring_at_start),
                f.deferred,
                f.left_behind,
                f.outside,
            )
            .expect("string write");
        }
    }
    std::fs::File::create(path)?.write_all(text.as_bytes())
}

pub(super) fn read_records(path: &str) -> Result<Vec<RunRecord>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let mut lines = text.lines();
    if lines.next() != Some(MAGIC) {
        return Err(format!("{path}: not a trip record"));
    }
    let mut runs = Vec::new();
    while let Some(line) = lines.next() {
        let field = |line: Option<&str>, key: &str| {
            line.and_then(|l| l.strip_prefix(key))
                .and_then(|l| l.strip_prefix(' '))
                .map(str::to_string)
                .ok_or_else(|| format!("{path}: expected `{key}`"))
        };
        let label = field(Some(line), "run")?;
        let bake = field(lines.next(), "bake")?;
        let number = |value: String| value.parse::<u64>().map_err(|e| format!("{path}: {e}"));
        let locate_mismatches = number(field(lines.next(), "locate_mismatches")?)?;
        let count = number(field(lines.next(), "frames")?)?;
        let mut frames = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let line = lines.next().ok_or_else(|| format!("{path}: truncated"))?;
            frames.push(parse_frame(line).ok_or_else(|| format!("{path}: bad frame {line:?}"))?);
        }
        runs.push(RunRecord {
            label,
            bake,
            locate_mismatches,
            frames,
        });
    }
    Ok(runs)
}

fn parse_frame(line: &str) -> Option<FrameRecord> {
    let mut parts = line.split_ascii_whitespace();
    let fingerprint = u64::from_str_radix(parts.next()?, 16).ok()?;
    let mut flag = || match parts.next()? {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    };
    let (spawn, tripped, grew, retiring_at_start) = (flag()?, flag()?, flag()?, flag()?);
    let mut count = || parts.next()?.parse::<u32>().ok();
    Some(FrameRecord {
        fingerprint,
        spawn,
        tripped,
        grew,
        retiring_at_start,
        deferred: count()?,
        left_behind: count()?,
        outside: count()?,
    })
}

/// Block-frames over a set of frames.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct SplitTotals {
    pub(super) frames: u64,
    pub(super) trip_frames: u64,
    pub(super) deferred: u64,
    pub(super) left_behind: u64,
    pub(super) outside: u64,
}

impl SplitTotals {
    fn add(&mut self, frame: &FrameRecord) {
        self.frames += 1;
        self.trip_frames += u64::from(frame.tripped);
        self.deferred += u64::from(frame.deferred);
        self.left_behind += u64::from(frame.left_behind);
        self.outside += u64::from(frame.outside);
    }

    pub(super) fn not_resident(&self) -> u64 {
        self.deferred + self.left_behind
    }
}

/// One run's frames split by a recorded run, both runs' totals side by side.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct GateSplit {
    /// Non-spawn frames the gate keeps: `[this run, recorded run]`.
    pub(super) gate: [SplitTotals; 2],
    /// Non-spawn frames outside the gate (O28).
    pub(super) apart: [SplitTotals; 2],
    /// Of `apart`, frames after a recorded trip, held until a growth retired.
    pub(super) held_after_trip: u64,
    pub(super) spawn_frames: u64,
}

/// Per frame, whether it leaves the gate: `recorded` tripped on it, or it
/// follows such a frame while a growth either run began there still retires.
pub(super) fn frames_apart(recorded: &[FrameRecord], this: &[FrameRecord]) -> Vec<bool> {
    let (mut hold_this, mut hold_recorded) = (false, false);
    recorded
        .iter()
        .zip(this)
        .map(|(recorded, this)| {
            hold_this &= this.retiring_at_start;
            hold_recorded &= recorded.retiring_at_start;
            let apart = recorded.tripped || hold_this || hold_recorded;
            if recorded.tripped {
                hold_this |= this.grew;
                hold_recorded |= recorded.grew;
            }
            apart
        })
        .collect()
}

/// Splits `this` run's frames by `recorded`'s trips. A run split by its own
/// record passes itself as both.
pub(super) fn split(recorded: &RunRecord, this: &RunRecord) -> Result<GateSplit, String> {
    if recorded.label != this.label {
        return Err(format!(
            "labels differ: recorded {:?}, this {:?}",
            recorded.label, this.label
        ));
    }
    if recorded.frames.len() != this.frames.len() {
        return Err(format!(
            "frame counts differ: recorded {}, this {}",
            recorded.frames.len(),
            this.frames.len()
        ));
    }
    if let Some(i) = (0..this.frames.len())
        .find(|&i| recorded.frames[i].fingerprint != this.frames[i].fingerprint)
    {
        return Err(format!("pose sequences differ from frame {i}"));
    }
    if recorded.locate_mismatches != this.locate_mismatches {
        return Err(format!(
            "locate mismatches differ: recorded {}, this {}",
            recorded.locate_mismatches, this.locate_mismatches
        ));
    }
    let apart = frames_apart(&recorded.frames, &this.frames);
    let mut out = GateSplit::default();
    for ((&apart, this), recorded) in apart.iter().zip(&this.frames).zip(&recorded.frames) {
        if this.spawn {
            out.spawn_frames += 1;
            continue;
        }
        let side = if apart { &mut out.apart } else { &mut out.gate };
        side[0].add(this);
        side[1].add(recorded);
        out.held_after_trip += u64::from(apart && !recorded.tripped);
    }
    Ok(out)
}
