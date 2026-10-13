// The mantle path a vault follows: a rise that the forward move overlaps as far
// as the lip and the probe's cleared volume allow, ending at the exit speed.
// See: context/lib/movement.md §4

use glam::Vec3;

use crate::collision::SKIN_DISTANCE;

/// Share of the path the forward move takes when the pawn arrives with no
/// horizontal speed. A faster entry shortens it (see [`CRUISE_END_SLOPE`]).
const FORWARD_SHARE: f32 = 0.4;

/// Share of the forward move's window that the rise still runs into. The
/// forward move starts while the rise is finishing, so the pawn never stops at
/// the corner; how far it gets before the rise ends is held to
/// [`lip_allowance`].
const RISE_OVERLAP: f32 = 0.2;

/// End slope of the forward cubic, in units of its window, at which it ends
/// with zero acceleration. A fast entry's window is sized to it, so the path's
/// last ticks already move at about the exit speed ([`end_slope`] then trims
/// the slope so the last tick matches it exactly).
const CRUISE_END_SLOPE: f32 = 1.5;

/// Largest end slope a cubic from rest can take without overshooting its end
/// or reversing. Only an entry too fast for the shortest window reaches it,
/// and then ends a little under the exit speed.
const MAX_END_SLOPE: f32 = 3.0;

/// Forward the allowance keeps back from the lip and the probe-proven bound,
/// metres, for float noise in the substrate's sweep.
const LIP_MARGIN: f32 = 0.25 * SKIN_DISTANCE;

/// Snap distance, in ms, under which an elapsed time counts as having reached
/// the path's end; elapsed accumulates by whole steps, so float drift stays far
/// below half a step.
fn end_snap(step_ms: f32) -> f32 {
    0.5 * step_ms
}

/// One vault's path, rebuilt each tick from its payload and tuning so replay
/// recomputes it exactly. Positions are feet positions in the vault's frame
/// (`VaultFrame`): `target` plus a shrinking share of `start_offset`.
///
/// - **Clock.** The path runs over whole ticks: `ceil(duration / step)` path
///   ticks, the last ending exactly on the target. A duration that is not a
///   multiple of the step would otherwise leave a short last tick that crawls.
/// - **Rise.** The vertical offset closes on a smoothstep that ends
///   `(1 − RISE_OVERLAP) · window` before the path does, at least one path tick
///   early, so the eye rises monotonically and stops rising before the move
///   over the top finishes.
/// - **Forward.** The horizontal offset closes on a cubic over the last
///   `window` of the path, from rest to the exit speed: `window` is
///   [`FORWARD_SHARE`] at low entry speed and shortens so the cubic ends at
///   `entry_speed` with zero acceleration ([`CRUISE_END_SLOPE`]). Its last
///   path tick moves at `entry_speed` ([`end_slope`]), and the exit tick then
///   restores that speed with no jump: no crawl at the end, and never past the
///   target.
/// - **Lip.** Each tick's forward is capped by [`lip_allowance`] at the height
///   the tick starts from. The allowance only shrinks as the feet sit deeper,
///   so a move that ends within it at its starting depth stays within it all
///   the way, and the whole tick's straight move stays clear of the lip and
///   inside the probe's cleared volume. The window shares keep the
///   cap from binding except on climbs near 2 m, where it trims centimetres at
///   most; at a very short duration it holds the forward back, and the
///   tick that would complete the path aims at the corner instead (full rise,
///   forward held), with one more path tick carrying the pawn onto the top.
#[derive(Debug, Clone, Copy)]
pub(super) struct VaultPath {
    target: Vec3,
    vertical: Vec3,
    horizontal: Vec3,
    /// Horizontal distance from start to target, metres.
    reach: f32,
    /// Height the feet climb to the resting height, metres; zero when the start
    /// is already level with it.
    climb: f32,
    path_ms: f32,
    step_ms: f32,
    rise_end: f32,
    forward_start: f32,
    end_slope: f32,
    radius: f32,
}

impl VaultPath {
    pub(super) fn new(
        target: Vec3,
        start_offset: Vec3,
        entry_speed: f32,
        duration_ms: f32,
        step_ms: f32,
        radius: f32,
    ) -> Self {
        let horizontal = Vec3::new(start_offset.x, 0.0, start_offset.z);
        let reach = horizontal.length();
        // Path ticks are the ticks the exit rule spends before `elapsed`
        // reaches the duration.
        let path_ticks = if step_ms > 0.0 {
            (duration_ms / step_ms).ceil().max(1.0)
        } else {
            1.0
        };
        let path_ms = if step_ms > 0.0 {
            path_ticks * step_ms
        } else {
            duration_ms
        };
        let path_s = path_ms / 1000.0;
        let speed_window = if entry_speed > 0.0 && reach > 0.0 {
            CRUISE_END_SLOPE * reach / (entry_speed * path_s)
        } else {
            f32::INFINITY
        };
        let min_window = 1.0 / ((1.0 - RISE_OVERLAP) * path_ticks);
        let window = speed_window.max(min_window).min(FORWARD_SHARE);
        let end_slope = end_slope(entry_speed, reach, window * path_ticks, step_ms);
        Self {
            target,
            vertical: Vec3::new(0.0, start_offset.y, 0.0),
            horizontal,
            reach,
            climb: (-start_offset.y).max(0.0),
            path_ms,
            step_ms,
            rise_end: 1.0 - (1.0 - RISE_OVERLAP) * window,
            forward_start: 1.0 - window,
            end_slope,
            radius,
        }
    }

    /// The feet point the path tick that advanced elapsed to `elapsed_ms` aimed
    /// at, and whether that point is the target. The vault intent calls this to
    /// aim, and next tick to judge the pawn against the point it aimed at, so
    /// the lip cap needs no stored state.
    pub(super) fn aimed(&self, elapsed_ms: f32) -> (Vec3, bool) {
        let next = self.progress(elapsed_ms);
        let previous = self.progress(elapsed_ms - self.step_ms);
        let rise = self.rise(next);
        let below = self.climb * (1.0 - self.rise(previous));
        let wanted = self.forward(next);
        let allowed = lip_allowance(below, self.radius);
        let forward = if allowed >= wanted * self.reach {
            wanted
        } else {
            allowed / self.reach
        };
        let feet = self.target + self.vertical * (1.0 - rise) + self.horizontal * (1.0 - forward);
        (feet, next >= 1.0 && forward >= 1.0)
    }

    /// Path progress, 0..=1, at `elapsed_ms`.
    fn progress(&self, elapsed_ms: f32) -> f32 {
        if elapsed_ms >= self.path_ms - end_snap(self.step_ms) {
            1.0
        } else {
            (elapsed_ms / self.path_ms).clamp(0.0, 1.0)
        }
    }

    /// Share of the climb done at progress `s`.
    fn rise(&self, s: f32) -> f32 {
        smoothstep((s / self.rise_end).clamp(0.0, 1.0))
    }

    /// Share of the horizontal offset closed at progress `s`: a cubic Hermite
    /// from rest to `end_slope`, monotonic and never past 1 for slopes up to
    /// [`MAX_END_SLOPE`].
    fn forward(&self, s: f32) -> f32 {
        let window = 1.0 - self.forward_start;
        if s <= self.forward_start {
            return 0.0;
        }
        let t = (s - self.forward_start) / window;
        if t >= 1.0 {
            return 1.0;
        }
        let m = self.end_slope;
        (3.0 - m) * t * t + (m - 2.0) * t * t * t
    }
}

/// End slope of the forward cubic whose last path tick moves exactly
/// `entry_speed · dt`, so the exit tick, which sets that speed, continues it.
/// `window_ticks` is the forward window in path ticks. A cubic from rest with
/// end slope `m` leaves `δ²(3 − 2δ) + m·δ(1 − δ)²` of the offset for its last
/// tick, `δ = 1 / window_ticks`; solving for `m` gives the slope. Under about
/// 2 m/s even `m = 0` moves faster than that over the last tick, so a slow
/// entry eases out and the exit settles to its speed. A window under one tick
/// (very short durations) keeps the cruise slope.
fn end_slope(entry_speed: f32, reach: f32, window_ticks: f32, step_ms: f32) -> f32 {
    if reach <= 0.0 || window_ticks <= 1.0 {
        return CRUISE_END_SLOPE;
    }
    let delta = 1.0 / window_ticks;
    let last_tick = entry_speed * step_ms / 1000.0 / reach;
    let from_rest = delta * delta * (3.0 - 2.0 * delta);
    let per_slope = delta * (1.0 - delta) * (1.0 - delta);
    ((last_tick - from_rest) / per_slope).clamp(0.0, MAX_END_SLOPE)
}

/// How far, metres, the capsule centre may move forward of where the vault
/// started while its feet are `below` metres under their resting height on the
/// top. Infinite once the feet reach that height.
///
/// Two bounds, the smaller kept, less [`LIP_MARGIN`]. Write `r` for the capsule
/// radius, `s` for [`SKIN_DISTANCE`] and `R = r + s`. The start is at least one
/// skin from the riser (the probe's first sweep stops a skin short of it), and
/// the top is one skin under the resting feet.
///
/// - **Lip.** The lower hemisphere's centre sits `R − below` above the lip edge.
///   Keeping a skin from the edge means keeping that centre `R` from it, so the
///   capsule may advance `R − sqrt(R² − (R − below)²)` (zero once
///   `below ≥ R`, when the edge is at or above the hemisphere's centre and the
///   riser itself stops it). This is the `r − sqrt(r² − (r − d)²)` bound of a
///   capsule against a riser, with the skin added.
/// - **Probe.** The probe proved clear the capsule grown by a skin, swept up the
///   start column and then forward one skin over the resting height
///   (`ledge_probe`). A capsule lowered `drop = below + s` under that forward
///   sweep and moved `forward` past the column stays inside that volume when
///   `forward · drop ≤ (R² − r²) / 2`, exactly so near the corner and with room
///   to spare further out (and for any forward once `drop ≤ s`). Its flanks are
///   what leave first: the forward sweep's floor curves up away from the centre
///   line.
///
/// With the dev capsule (`r = 0.2`) the lip bound binds until the feet are
/// 10 cm under their resting height and the probe bound after: no forward from
/// 22 cm under, 3.5 cm at 10 cm, 10.5 cm with the feet level with the top.
pub(super) fn lip_allowance(below: f32, radius: f32) -> f32 {
    if below <= 0.0 {
        return f32::INFINITY;
    }
    let skin = SKIN_DISTANCE;
    let grown = radius + skin;
    let probe = (grown * grown - radius * radius) / (2.0 * (below + skin));
    let lip = if below >= grown {
        0.0
    } else {
        let over_edge = grown - below;
        grown - (grown * grown - over_edge * over_edge).max(0.0).sqrt()
    };
    (probe.min(lip) - LIP_MARGIN).max(0.0)
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use glam::{Vec2, Vec3};

    use super::*;

    /// Distance from `point` to the quadrant `x ≥ corner.x, y ≤ corner.y`
    /// (negative inside it).
    fn distance_to_solid_below(point: Vec2, corner: Vec2) -> f32 {
        let ahead = corner.x - point.x;
        let above = point.y - corner.y;
        match (ahead > 0.0, above > 0.0) {
            (true, true) => Vec2::new(ahead, above).length(),
            (true, false) => ahead,
            (false, true) => above,
            (false, false) => -ahead.abs().min(above.abs()),
        }
    }

    // The probe's two clearance sweeps, grown by the skin they keep: the start
    // column, and the forward sweep whose lower hemisphere centre runs along
    // y = 0 (x forward of the column). `segment` is the capsule's centre segment
    // length. Returns whether `point` lies inside their union.
    fn inside_probed_volume(point: Vec3, radius: f32, segment: f32) -> bool {
        let grown = radius + SKIN_DISTANCE;
        let to_column = point.x.abs();
        let to_sweep = Vec2::new(
            (-point.x).max(0.0),
            (-point.y).max(0.0).max(point.y - segment),
        )
        .length();
        let planar = to_column.min(to_sweep);
        planar * planar + point.z * point.z <= grown * grown + 1.0e-6
    }

    #[test]
    fn the_lip_allowance_keeps_the_capsule_in_the_probed_volume_and_a_skin_off_the_lip() {
        for radius in [0.2_f32, 0.4] {
            let grown = radius + SKIN_DISTANCE;
            // The lip edge, in the frame of the forward sweep's lower
            // hemisphere centre over the start column: a grown radius ahead
            // (the start is a skin off the riser) and two skins plus a radius
            // under (the sweep runs a skin over the resting feet, which rest a
            // skin over the top).
            let edge = Vec2::new(grown, -(radius + 2.0 * SKIN_DISTANCE));
            for step in 1..=240 {
                let below = step as f32 * 0.0025;
                let forward = lip_allowance(below, radius);
                assert!(forward.is_finite() && forward >= 0.0);
                let centre = Vec2::new(forward, -(below + SKIN_DISTANCE));
                assert!(
                    distance_to_solid_below(centre, edge) >= grown - LIP_MARGIN - 1.0e-5,
                    "r {radius}, {below} m under: {forward} m forward reaches into the lip's skin"
                );
                // Sample the lower hemisphere ball, the part that leaves the
                // probed volume first.
                for shell in 0..=4 {
                    let rho = radius * shell as f32 / 4.0;
                    for a in 0..24 {
                        let polar = std::f32::consts::PI * a as f32 / 23.0;
                        for b in 0..24 {
                            let azimuth = std::f32::consts::TAU * b as f32 / 24.0;
                            let offset = Vec3::new(
                                polar.sin() * azimuth.cos(),
                                polar.cos(),
                                polar.sin() * azimuth.sin(),
                            ) * rho;
                            let point = centre.extend(0.0) + offset;
                            assert!(
                                inside_probed_volume(point, radius, 1.6),
                                "r {radius}, {below} m under, {forward} m forward: {point} \
                                 is outside the probed volume"
                            );
                        }
                    }
                }
            }
            assert_eq!(lip_allowance(0.0, radius), f32::INFINITY);
            assert_eq!(lip_allowance(grown, radius), 0.0);
        }
    }

    #[test]
    fn the_path_ends_on_the_target_at_the_exit_speed_without_overshoot() {
        let step_ms = 1000.0 / 60.0;
        let target = Vec3::new(5.42, 1.22, 0.0);
        let start_offset = Vec3::new(-0.86, -0.72, 0.0);
        // 0 and 1 m/s are slower than a cubic from rest can end its last tick
        // over this reach, so they ease out instead (checked below).
        for entry_speed in [0.0_f32, 1.0, 3.0, 7.0, 11.0, 20.0] {
            for duration_ms in [320.0_f32, 420.0] {
                let path =
                    VaultPath::new(target, start_offset, entry_speed, duration_ms, step_ms, 0.4);
                let ticks = (duration_ms / step_ms).ceil() as usize;
                let points: Vec<Vec3> = (0..=ticks)
                    .map(|k| path.aimed(k as f32 * step_ms).0)
                    .collect();
                assert!(
                    path.aimed(ticks as f32 * step_ms).1,
                    "{entry_speed} m/s: on the target"
                );
                for pair in points.windows(2) {
                    assert!(pair[1].x >= pair[0].x - 1.0e-6, "forward never reverses");
                    assert!(pair[1].y >= pair[0].y - 1.0e-6, "the rise never reverses");
                    assert!(pair[1].x <= target.x + 1.0e-5, "never past the target");
                }
                let last = points[ticks] - points[ticks - 1];
                let last_speed = last.x / (step_ms / 1000.0);
                if entry_speed >= 3.0 {
                    assert!(
                        (last_speed - entry_speed).abs() < 1.0e-3 * entry_speed,
                        "{entry_speed} m/s over {duration_ms} ms: last path tick moves {last_speed} m/s"
                    );
                } else {
                    let before = (points[ticks - 1] - points[ticks - 2]).x;
                    assert!(
                        last.x < before && last_speed > entry_speed,
                        "{entry_speed} m/s: the slow entry eases out toward its exit speed"
                    );
                }
                assert!(
                    last.y.abs() < 1.0e-6,
                    "the rise is done before the last tick"
                );
            }
        }
    }
}
