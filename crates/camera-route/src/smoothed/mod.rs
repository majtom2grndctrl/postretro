// Smoothed routes: the walk's cells in order, at run speed with a capped turn rate.
// See: context/lib/rendering_pipeline.md §12

// The eye flies a polyline. Walk step i's leg runs from a point in cell i to
// a point just inside cell i+1, past their shared portal, and every leg
// segment is proven against the cell locator's planes to stay within those
// two cells, so every sample on it locates in the walk's current or next
// cell. A leg whose straight line leaves them detours through seeded in-cell
// points (non-convex cells). Teleports, and a leg no proven line makes, are
// cuts. Yaw follows the travel direction at most the turn cap per pose; a
// turn the cap cannot make at once slows the eye, down to turning in place.

use glam::Vec3;
use postretro_level_loader::LevelWorld;

use crate::FRAME_SECONDS;
use crate::exact::{RouteError, RoutePose, walkable};
use crate::path::{SplitMix64, WalkKind};
use crate::walk::{RUN_SPEED, Walk};

mod leg;

use leg::{in_cell_point, leg};

/// The default eye speed: the walk's run speed, metres per second.
pub const DEFAULT_SPEED: f32 = RUN_SPEED;
/// The default yaw rate cap, degrees per second.
pub const DEFAULT_TURN_CAP_DEGREES: f32 = 270.0;
/// XORed into the path seed to seed the detour stream.
pub const DETOUR_STREAM_SALT: u64 = 0x5300_7E11;

/// A smoothed route's motion limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Smoothing {
    /// Eye speed, metres per second.
    pub speed: f32,
    /// Yaw rate cap, degrees per second.
    pub turn_cap_degrees: f32,
}

impl Default for Smoothing {
    fn default() -> Self {
        Self {
            speed: DEFAULT_SPEED,
            turn_cap_degrees: DEFAULT_TURN_CAP_DEGREES,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SmoothedRoute {
    /// Each pose's `walk` is (walk step, the cell its eye locates in).
    pub poses: Vec<RoutePose>,
    /// Cuts where the walk teleports.
    pub teleport_cuts: usize,
    /// Cuts where no proven line reached the walk's next cell.
    pub locate_cuts: usize,
}

/// The smoothed route over the walk of `kind` seeded `seed`, at `pitch`
/// (radians). Speed and turn cap must be finite and positive.
pub fn smoothed_route_seeded(
    world: &LevelWorld,
    kind: WalkKind,
    steps: usize,
    pitch: f32,
    seed: u64,
    smoothing: Smoothing,
) -> Result<SmoothedRoute, RouteError> {
    if !(smoothing.speed.is_finite() && smoothing.speed > 0.0) {
        return Err(RouteError::NonPositiveSpeed);
    }
    if !(smoothing.turn_cap_degrees.is_finite() && smoothing.turn_cap_degrees > 0.0) {
        return Err(RouteError::NonPositiveTurnCap);
    }
    walkable(world, steps)?;
    let walk = Walk::build_seeded(world, kind, steps, seed);
    Ok(SmoothedRoute::of_walk(world, &walk, pitch, smoothing))
}

/// A polyline vertex; the segment leaving it belongs to walk step `step`.
/// A boundary vertex (the first of its step) locates in that step's cell.
#[derive(Debug, Clone, Copy)]
struct Vertex {
    at: Vec3,
    step: u32,
}

/// Polyline flown without a jump. Every piece after the first starts on a cut.
struct Piece {
    vertices: Vec<Vertex>,
}

impl SmoothedRoute {
    pub fn of_walk(world: &LevelWorld, walk: &Walk, pitch: f32, smoothing: Smoothing) -> Self {
        let mut rng = SplitMix64(walk.seed ^ DETOUR_STREAM_SALT);
        let mut pieces: Vec<Piece> = Vec::new();
        let (mut teleport_cuts, mut locate_cuts) = (0, 0);
        // Whether the last piece takes the next leg, and whether a teleport
        // lies between the last piece and the next one.
        let (mut open, mut teleported) = (false, false);
        for (step, &cell) in walk.cells.iter().enumerate() {
            let step32 = step as u32;
            if open && !walk.teleport[step] {
                let piece = pieces.last_mut().expect("an open piece");
                let last = piece.vertices.last_mut().expect("a piece has a vertex");
                let from = walk.cells[step - 1];
                if from == cell {
                    last.step = step32;
                    continue;
                }
                if let Some(leg) = leg(world, last.at, from as usize, cell as usize, &mut rng) {
                    let (end, detour) = leg.split_last().expect("a leg ends in the next cell");
                    let detour = detour.iter().map(|&at| Vertex {
                        at,
                        step: step32 - 1,
                    });
                    piece.vertices.extend(detour);
                    piece.vertices.push(Vertex {
                        at: *end,
                        step: step32,
                    });
                    continue;
                }
            }
            // A new piece: the route's start, a teleport, or an unproven leg.
            teleported |= walk.teleport[step];
            open = match in_cell_point(world, cell as usize, &mut rng) {
                Some(at) => {
                    if !pieces.is_empty() {
                        if teleported {
                            teleport_cuts += 1;
                        } else {
                            locate_cuts += 1;
                        }
                    }
                    pieces.push(Piece {
                        vertices: vec![Vertex { at, step: step32 }],
                    });
                    teleported = false;
                    true
                }
                // Nothing locates in this cell, so no pose can stand in it.
                None => false,
            };
        }
        let mut flight = Flight::new(world, &walk.cells, pitch, smoothing);
        for (index, piece) in pieces.iter().enumerate() {
            flight.fly(piece, index > 0);
        }
        Self {
            poses: flight.poses,
            teleport_cuts,
            locate_cuts: locate_cuts + flight.locate_misses,
        }
    }
}

/// Heading of travel along `delta`, engine yaw (0 faces −Z); none for a
/// near-vertical segment, which keeps the current yaw.
fn heading(delta: Vec3) -> Option<f32> {
    let level = delta.x.hypot(delta.z);
    (level > 1e-6 && level >= 0.1 * delta.length()).then(|| (-delta.x).atan2(-delta.z))
}

/// An angle wrapped into [−π, π).
fn wrap(angle: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    angle - TAU * ((angle + PI) / TAU).floor()
}

/// Flies pieces into poses at the pose cadence.
struct Flight<'a> {
    world: &'a LevelWorld,
    cells: &'a [u32],
    pitch: f32,
    /// Metres per pose at full speed.
    stride: f32,
    /// Radians per pose at the cap.
    turn: f32,
    yaw: f32,
    /// The first walk step whose cell no pose has located yet.
    need: usize,
    poses: Vec<RoutePose>,
    /// Poses that left their two cells despite the proof; emitted as cuts.
    locate_misses: usize,
}

impl<'a> Flight<'a> {
    fn new(world: &'a LevelWorld, cells: &'a [u32], pitch: f32, smoothing: Smoothing) -> Self {
        Self {
            world,
            cells,
            pitch,
            stride: smoothing.speed * FRAME_SECONDS as f32,
            turn: smoothing.turn_cap_degrees.to_radians() * FRAME_SECONDS as f32,
            yaw: 0.0,
            need: 0,
            poses: Vec::new(),
            locate_misses: 0,
        }
    }

    fn fly(&mut self, piece: &Piece, cut: bool) {
        let vertices = &piece.vertices;
        let lengths: Vec<f32> = vertices
            .windows(2)
            .map(|w| w[0].at.distance(w[1].at))
            .collect();
        let last = vertices.len() - 1;
        let segment_heading = |j: usize| heading(vertices[j + 1].at - vertices[j].at);
        if (cut || self.poses.is_empty())
            && let Some(yaw) = (0..last).find_map(segment_heading)
        {
            self.yaw = yaw;
        }
        let (mut j, mut u) = (0, 0.0f32);
        self.emit(vertices, j, u, Some(cut));
        loop {
            while j < last && u >= lengths[j] {
                u -= lengths[j];
                j += 1;
            }
            if j == last {
                return;
            }
            // Turn toward the travel heading by at most the cap; the turn
            // still owed slows the eye, to a stop from a quarter turn.
            let mut remaining = 0.0f32;
            if let Some(target) = segment_heading(j) {
                let error = wrap(target - self.yaw);
                if error.abs() <= self.turn {
                    self.yaw = target;
                } else {
                    self.yaw = wrap(self.yaw + self.turn.copysign(error));
                    remaining = error.abs() - self.turn;
                }
            }
            let (from, mut ahead) = (j, self.stride * remaining.cos().max(0.0));
            while j < last && u + ahead >= lengths[j] {
                ahead -= lengths[j] - u;
                (j, u) = (j + 1, 0.0);
                // A corner sharper than one pose's turn: stop on it and turn
                // there, rather than round it facing the old way.
                let sharp = |heading: f32| wrap(heading - self.yaw).abs() > self.turn;
                if j < last && segment_heading(j).is_some_and(sharp) {
                    ahead = 0.0;
                }
            }
            u = if j == last { 0.0 } else { u + ahead };
            // Never pass a step whose cell no pose has located: stop on its
            // boundary vertex, which locates there.
            let need = self.need;
            if vertices[j].step as usize >= need
                && let Some(back) = (from + 1..=j).find(|&k| vertices[k].step as usize >= need)
            {
                (j, u) = (back, 0.0);
            }
            self.emit(vertices, j, u, None);
        }
    }

    /// Emits the pose at `u` along segment `j`. `start` is `Some(cut)` on a
    /// piece's first pose.
    fn emit(&mut self, vertices: &[Vertex], j: usize, u: f32, start: Option<bool>) {
        let cut = start == Some(true);
        let eye = match vertices.get(j + 1) {
            Some(next) if u > 0.0 => {
                let length = vertices[j].at.distance(next.at);
                vertices[j].at.lerp(next.at, u / length)
            }
            _ => vertices[j].at,
        };
        let step = vertices[j].step as usize;
        let cell = self.world.locate_cell(eye);
        let allowed = [Some(self.cells[step]), self.cells.get(step + 1).copied()];
        let miss = !allowed.contains(&Some(cell as u32));
        self.locate_misses += usize::from(miss && !cut);
        if start.is_some() {
            self.need = step;
        }
        if self.cells.get(self.need) == Some(&(cell as u32)) && self.need <= step + 1 {
            self.need += 1;
            while self.need < self.cells.len() && self.cells[self.need] == self.cells[self.need - 1]
            {
                self.need += 1;
            }
        }
        self.poses.push(RoutePose {
            eye,
            yaw: self.yaw,
            pitch: self.pitch,
            walk: Some((step as u32, cell as u32)),
            cut: cut || miss,
        });
    }
}
