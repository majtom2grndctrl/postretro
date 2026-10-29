//! The bake-gap gate across cell partitions: the residency walks' frames
//! recorded in world space, replayed in another build, compared in deferred
//! texel area.
//! See: context/lib/rendering_pipeline.md §2, §4

// The before build materialises each walk's frames from its own cells and
// eye stream, as `run` does, and writes every frame's eye, yaw and pitch.
// The after build replays those frames: the camera cell is wherever the eye
// locates in its own partition. Blocks are per cell, so the deferred block
// count stops comparing once cells change; their texel area does (§2).

use std::fmt::Write as _;

use glam::Vec3;
use postretro_level_loader::LevelWorld;
use postretro_visibility::{TimingGate, VisibilityPath};

use super::super::baked_set::bake_identity;
use super::super::frame_driver::FrameInput;
use super::super::modes::{ReadsMode, WalkModes, render_camera};
use super::super::paths::{SplitMix64, WALK_SEED, WalkKind};
use super::super::report::pool_layers_line;
use super::super::walk::{Walk, eye_point};
use super::super::{DEFAULT_LEVERS, DEFAULT_STEPS, FRAME_SECONDS, Rig, run_label};
use super::poses::{WorldPose, read_poses};
use super::{FILE_ENV, STEPS_ENV, env};
use crate::render_preparation::VisibleRenderPreparation;

const MAGIC: &str = "postretro-bake-gap-world-record 2";

/// The pool policy identity this build runs under, recorded so a pair never
/// compares builds sized differently (rendering_pipeline.md §2: both builds of
/// a paired comparison run the same pool policy). Mirrors the demand-sized
/// growth in `crates/render-cpu/src/lightmap_pool/policy/growth.rs`:
/// `GROWTH_HEADROOM` is 4/3 there but `pub(super)`, so it is restated here.
/// Change this string whenever that policy or ratio changes.
const POOL_POLICY: &str = "demand-sized growth headroom 4/3";
/// Frames the pose tour holds each pose: drain 0's reads land in drain 1 of
/// the first frame; the second shows what a retiring generation still holds.
const POSE_TOUR_DWELL: u32 = 2;

/// One frame's camera in world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct FramePose {
    pub(super) eye: Vec3,
    /// Radians, engine convention; stored exactly so a replay is bit-equal.
    pub(super) yaw: f32,
    pub(super) pitch: f32,
}

impl FramePose {
    fn same(&self, other: &FramePose) -> bool {
        self.eye.to_array().map(f32::to_bits) == other.eye.to_array().map(f32::to_bits)
            && self.yaw.to_bits() == other.yaw.to_bits()
            && self.pitch.to_bits() == other.pitch.to_bits()
    }
}

/// What one frame left missing after its last drain.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct FrameResult {
    pub(super) tripped: bool,
    pub(super) deferred: u32,
    pub(super) deferred_texels: u64,
    pub(super) left_behind: u32,
    pub(super) left_behind_texels: u64,
    /// Drawn blocks outside the camera cell's baked set.
    pub(super) outside: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Sequence {
    /// The walk kind and the levers it ran at; a pair matches on it.
    pub(super) label: String,
    /// The settled-entry preload's eye.
    pub(super) spawn: Vec3,
    pub(super) frames: Vec<(FramePose, FrameResult)>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct BakeGapRecord {
    pub(super) bake: String,
    /// Projection, pitch and read mode; a pair must match it.
    pub(super) modes: String,
    /// Pool sizing policy and headroom ratio; a pair must match it.
    pub(super) pool_policy: String,
    pub(super) sequences: Vec<Sequence>,
}

/// Runs the gate's sequences on `world`: `before`'s frames when given, else
/// this build's walks plus the pose tour. `None` when the level does not
/// stream its lightmap.
pub(super) fn run(
    world: &LevelWorld,
    path: &str,
    before: Option<&BakeGapRecord>,
) -> Option<BakeGapRecord> {
    super::super::LightmapLevelView::of(world)?;
    let mut modes = WalkModes::from_env();
    modes.reads = ReadsMode::CompletedReads;
    modes.planar = false;
    let plans: Vec<(String, Vec3, Vec<FramePose>)> = match before {
        Some(record) => record
            .sequences
            .iter()
            .map(|seq| {
                let frames = seq.frames.iter().map(|(pose, _)| *pose).collect();
                (seq.label.clone(), seq.spawn, frames)
            })
            .collect(),
        None => materialise(world, &modes),
    };
    let mut sequences = Vec::new();
    for (label, spawn, frames) in plans {
        let started = std::time::Instant::now();
        let (results, pool) = replay(world, spawn, &frames, &modes);
        let seq = Sequence {
            label,
            spawn,
            frames: frames.into_iter().zip(results).collect(),
        };
        let totals = Totals::of(&seq);
        println!(
            "{}: {} frames in {:.1} s; deferred {} blocks / {} texels, left behind {} blocks / \
             {} texels, {} trip frames",
            seq.label,
            seq.frames.len(),
            started.elapsed().as_secs_f64(),
            totals.deferred,
            totals.deferred_texels,
            totals.left_behind,
            totals.left_behind_texels,
            totals.trips,
        );
        println!("   pool: {pool}");
        sequences.push(seq);
    }
    Some(BakeGapRecord {
        bake: bake_identity(path, world),
        modes: modes.label(),
        pool_policy: POOL_POLICY.to_string(),
        sequences,
    })
}

/// This build's walks as the residency harness runs them at the default
/// levers (same paths, eye stream and headings), plus a tour of the pose
/// file's poses when `POSTRETRO_WORLD_POSES_FILE` is set.
fn materialise(world: &LevelWorld, modes: &WalkModes) -> Vec<(String, Vec3, Vec<FramePose>)> {
    let steps = env(STEPS_ENV).map_or(DEFAULT_STEPS, |v| v.trim().parse().expect("steps"));
    let mut plans = Vec::new();
    for kind in WalkKind::ALL {
        let walk = Walk::build(world, kind, steps);
        if walk.cells.is_empty() {
            continue;
        }
        // `run`'s eye stream: the spawn eye first, then one eye per frame.
        let mut rng = SplitMix64(WALK_SEED ^ 0xE7E5);
        let spawn = eye_point(world, walk.cells[0], &mut rng);
        let mut frames = Vec::with_capacity(walk.frames() as usize);
        for (step, &cell) in walk.cells.iter().enumerate() {
            for frame in 0..walk.dwell[step] {
                frames.push(FramePose {
                    eye: eye_point(world, cell, &mut rng),
                    yaw: walk.travel_yaw[step] + frame as f32 * std::f32::consts::FRAC_PI_2,
                    pitch: modes.pitch,
                });
            }
        }
        plans.push((run_label(&walk, DEFAULT_LEVERS, modes), spawn, frames));
    }
    if let Some(file) = env(FILE_ENV) {
        let set = read_poses(&file).unwrap_or_else(|why| panic!("{why}"));
        if let Some(first) = set.poses.first() {
            let name = std::path::Path::new(&file)
                .file_name()
                .map_or(file.clone(), |name| name.to_string_lossy().into_owned());
            plans.push((
                format!("pose tour {name}, {POSE_TOUR_DWELL} frames per pose"),
                first.eye,
                pose_tour(&set.poses),
            ));
        }
    }
    plans
}

/// Each pose in file order, held `POSE_TOUR_DWELL` frames.
fn pose_tour(poses: &[WorldPose]) -> Vec<FramePose> {
    poses
        .iter()
        .flat_map(|pose| {
            let frame = FramePose {
                eye: pose.eye,
                yaw: pose.yaw(),
                pitch: pose.pitch(),
            };
            std::iter::repeat_n(frame, POSE_TOUR_DWELL as usize)
        })
        .collect()
}

/// One sequence through a fresh rig: preload at the cell `spawn` locates
/// to, then every frame's walk from its eye and the completed-reads drains.
/// Returns each frame's result and the pool's layer line.
fn replay(
    world: &LevelWorld,
    spawn: Vec3,
    frames: &[FramePose],
    modes: &WalkModes,
) -> (Vec<FrameResult>, String) {
    let mut rig = Rig::new(world, DEFAULT_LEVERS, modes.reads);
    let spawn_cell = world.locate_cell(spawn);
    assert!(
        super::is_interior(world, spawn_cell),
        "spawn eye {spawn} locates to cell {spawn_cell}, which is solid or exterior here"
    );
    rig.preload(spawn_cell as u32);
    let mut scratch = postretro_visibility::PortalWalkScratch::new();
    let mut seconds = 0.0f64;
    let mut results = Vec::with_capacity(frames.len());
    for pose in frames {
        let camera = render_camera(pose.eye, pose.yaw, pose.pitch, modes.projection);
        let main = VisibleRenderPreparation::for_level(
            world,
            pose.eye,
            camera.view_projection,
            &[],
            false,
            &mut scratch,
            TimingGate::OFF,
        );
        let outcome = rig.driver.frame(
            FrameInput {
                visible_cells: &main.visible_cells,
                sh_visible_cells: &main.visible_cells,
                camera_cell: main.stats.camera_cell,
                path: main.stats.path,
                seconds,
            },
            rig.view.residency_set,
        );
        results.push(FrameResult {
            tripped: matches!(
                main.stats.path,
                VisibilityPath::PortalStepLimitFallback { .. }
            ),
            deferred: outcome.deferred,
            deferred_texels: outcome.deferred_texels,
            left_behind: outcome.left_behind,
            left_behind_texels: outcome.left_behind_texels,
            outside: outcome.outside,
        });
        seconds += FRAME_SECONDS;
        if let postretro_visibility::VisibleCells::Culled(cells) = main.visible_cells {
            scratch.recycle_cells(cells);
        }
    }
    let (_, _, mirror) = rig.driver.finish();
    let pool = format!(
        "{}, growths {}",
        pool_layers_line(&mirror.stats, mirror.layer_bytes()),
        mirror.stats.growths
    );
    (results, pool)
}

// --- Record ---

pub(super) fn format_record(record: &BakeGapRecord) -> String {
    let mut text = String::new();
    writeln!(text, "{MAGIC}").expect("string write");
    writeln!(text, "bake {}", record.bake).expect("string write");
    writeln!(text, "modes {}", record.modes).expect("string write");
    writeln!(text, "pool {}", record.pool_policy).expect("string write");
    for seq in &record.sequences {
        writeln!(text, "seq {}", seq.label).expect("string write");
        writeln!(
            text,
            "spawn {} {} {} frames {}",
            seq.spawn.x,
            seq.spawn.y,
            seq.spawn.z,
            seq.frames.len()
        )
        .expect("string write");
        for (pose, result) in &seq.frames {
            writeln!(
                text,
                "{} {} {} {} {} {} {} {} {} {} {}",
                pose.eye.x,
                pose.eye.y,
                pose.eye.z,
                pose.yaw,
                pose.pitch,
                u8::from(result.tripped),
                result.deferred,
                result.deferred_texels,
                result.left_behind,
                result.left_behind_texels,
                result.outside,
            )
            .expect("string write");
        }
    }
    text
}

pub(super) fn parse_record(text: &str) -> Result<BakeGapRecord, String> {
    let mut lines = text.lines();
    if lines.next() != Some(MAGIC) {
        return Err(format!(
            "not a bake-gap world record of this format (expected `{MAGIC}`); records written \
             under an older pool policy must be re-recorded"
        ));
    }
    let field = |line: Option<&str>, key: &str| {
        line.and_then(|l| l.strip_prefix(key))
            .and_then(|l| l.strip_prefix(' '))
            .map(str::to_string)
            .ok_or_else(|| format!("expected `{key}`"))
    };
    let bake = field(lines.next(), "bake")?;
    let modes = field(lines.next(), "modes")?;
    let pool_policy = field(lines.next(), "pool")?;
    let mut sequences = Vec::new();
    while let Some(line) = lines.next() {
        if line.trim().is_empty() {
            continue;
        }
        let label = field(Some(line), "seq")?;
        let spawn_line = field(lines.next(), "spawn")?;
        let bad = |line: &str| format!("bad line {line:?}");
        let mut parts = spawn_line.split_ascii_whitespace();
        let mut float = || parts.next()?.parse::<f32>().ok();
        let spawn = Vec3::new(
            float().ok_or_else(|| bad(&spawn_line))?,
            float().ok_or_else(|| bad(&spawn_line))?,
            float().ok_or_else(|| bad(&spawn_line))?,
        );
        let count: usize = spawn_line
            .split_once(" frames ")
            .and_then(|(_, n)| n.trim().parse().ok())
            .ok_or_else(|| bad(&spawn_line))?;
        let mut frames = Vec::with_capacity(count);
        for _ in 0..count {
            let line = lines.next().ok_or("truncated record")?;
            frames.push(parse_frame(line).ok_or_else(|| bad(line))?);
        }
        sequences.push(Sequence {
            label,
            spawn,
            frames,
        });
    }
    Ok(BakeGapRecord {
        bake,
        modes,
        pool_policy,
        sequences,
    })
}

fn parse_frame(line: &str) -> Option<(FramePose, FrameResult)> {
    let mut parts = line.split_ascii_whitespace();
    let mut float = || parts.next()?.parse::<f32>().ok();
    let pose = FramePose {
        eye: Vec3::new(float()?, float()?, float()?),
        yaw: float()?,
        pitch: float()?,
    };
    let tripped = match parts.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let mut int = || parts.next()?.parse::<u64>().ok();
    let result = FrameResult {
        tripped,
        deferred: u32::try_from(int()?).ok()?,
        deferred_texels: int()?,
        left_behind: u32::try_from(int()?).ok()?,
        left_behind_texels: int()?,
        outside: u32::try_from(int()?).ok()?,
    };
    Some((pose, result))
}

pub(super) fn write_record(path: &str, record: &BakeGapRecord) -> std::io::Result<()> {
    std::fs::write(path, format_record(record))
}

pub(super) fn read_record(path: &str) -> Result<BakeGapRecord, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    parse_record(&text).map_err(|why| format!("{path}: {why}"))
}

// --- Verdict ---

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Totals {
    pub(super) frames: u64,
    pub(super) trips: u64,
    pub(super) deferred: u64,
    pub(super) deferred_texels: u64,
    pub(super) left_behind: u64,
    pub(super) left_behind_texels: u64,
    pub(super) outside: u64,
}

impl Totals {
    pub(super) fn of(seq: &Sequence) -> Self {
        let mut out = Self::default();
        for (_, r) in &seq.frames {
            out.frames += 1;
            out.trips += u64::from(r.tripped);
            out.deferred += u64::from(r.deferred);
            out.deferred_texels += r.deferred_texels;
            out.left_behind += u64::from(r.left_behind);
            out.left_behind_texels += r.left_behind_texels;
            out.outside += u64::from(r.outside);
        }
        out
    }
}

#[derive(Debug, Default)]
pub(super) struct BakeGapVerdict {
    pub(super) invalid: Option<String>,
    /// Per sequence: label, before (when paired), this run, frames where
    /// this run defers more texels than before.
    pub(super) rows: Vec<(String, Option<Totals>, Totals, u64)>,
}

impl BakeGapVerdict {
    /// More deferred texel area than before; with before at zero, any.
    fn row_fails(before: Option<&Totals>, this: &Totals) -> bool {
        before.is_some_and(|before| this.deferred_texels > before.deferred_texels)
    }

    pub(super) fn passed(&self) -> bool {
        self.invalid.is_none()
            && !self
                .rows
                .iter()
                .any(|(_, before, this, _)| Self::row_fails(before.as_ref(), this))
    }

    pub(super) fn print(&self) {
        println!("\n=== bake-gap guard: deferred lightmap texel area, summed per sequence");
        if let Some(why) = &self.invalid {
            println!("INVALID pair: {why}");
            return;
        }
        for (label, before, this, worse_frames) in &self.rows {
            match before {
                Some(before) => println!(
                    "{label}\n  deferred texels: before {} after {} ({})\n  deferred blocks: \
                     before {} after {}; left-behind texels: before {} after {}; trip frames: \
                     before {} after {}; frames deferring more texels after: {worse_frames} of {}",
                    before.deferred_texels,
                    this.deferred_texels,
                    if Self::row_fails(Some(before), this) {
                        "FAIL"
                    } else {
                        "ok"
                    },
                    before.deferred,
                    this.deferred,
                    before.left_behind_texels,
                    this.left_behind_texels,
                    before.trips,
                    this.trips,
                    this.frames,
                ),
                None => println!(
                    "{label}\n  deferred texels {} (blocks {}); left-behind texels {} (blocks \
                     {}); outside-baked-set blocks {}; trip frames {} of {}",
                    this.deferred_texels,
                    this.deferred,
                    this.left_behind_texels,
                    this.left_behind,
                    this.outside,
                    this.trips,
                    this.frames,
                ),
            }
        }
        let paired = self.rows.iter().any(|(_, before, _, _)| before.is_some());
        println!(
            "verdict: {}",
            match (paired, self.passed()) {
                (false, _) => "UNPAIRED (record only)",
                (true, true) => "PASS",
                (true, false) => "FAIL",
            }
        );
    }
}

/// The gate over `this` run, paired with `before` when given. Sequences pair
/// by label and frame index and must hold bit-identical world poses.
pub(super) fn verdict(before: Option<&BakeGapRecord>, this: &BakeGapRecord) -> BakeGapVerdict {
    let mut out = BakeGapVerdict::default();
    if let Some(before) = before {
        if before.modes != this.modes {
            out.invalid = Some(format!(
                "modes differ: before {:?}, this {:?}",
                before.modes, this.modes
            ));
            return out;
        }
        if before.pool_policy != this.pool_policy {
            out.invalid = Some(format!(
                "pool policy differs: before {:?}, this {:?}",
                before.pool_policy, this.pool_policy
            ));
            return out;
        }
        let labels = |r: &BakeGapRecord| -> Vec<String> {
            r.sequences.iter().map(|s| s.label.clone()).collect()
        };
        if labels(before) != labels(this) {
            out.invalid = Some(format!(
                "sequences differ: before {:?}, this {:?}",
                labels(before),
                labels(this)
            ));
            return out;
        }
        for (b, t) in before.sequences.iter().zip(&this.sequences) {
            if b.frames.len() != t.frames.len()
                || b.spawn.to_array().map(f32::to_bits) != t.spawn.to_array().map(f32::to_bits)
            {
                out.invalid = Some(format!("{}: spawn or frame count differs", t.label));
                return out;
            }
            if let Some(i) = (0..t.frames.len()).find(|&i| !b.frames[i].0.same(&t.frames[i].0)) {
                out.invalid = Some(format!("{}: world poses differ from frame {i}", t.label));
                return out;
            }
        }
    }
    for (i, seq) in this.sequences.iter().enumerate() {
        let earlier = before.map(|b| &b.sequences[i]);
        let worse = earlier.map_or(0, |b| {
            b.frames
                .iter()
                .zip(&seq.frames)
                .filter(|((_, b), (_, t))| t.deferred_texels > b.deferred_texels)
                .count() as u64
        });
        out.rows.push((
            seq.label.clone(),
            earlier.map(Totals::of),
            Totals::of(seq),
            worse,
        ));
    }
    out
}
