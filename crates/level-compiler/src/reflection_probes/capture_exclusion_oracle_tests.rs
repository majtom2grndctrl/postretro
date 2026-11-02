// Capture exclusion no-leak oracle: generated box layouts in and near contact, rays into their gaps, each read checked against kept solid from the brush planes.
// See: context/lib/resource_management.md §5.2 (Exclusion)

use glam::{DVec3, Vec3};

use super::capture_bounds::HELD_DEPTH_SLACK_M;
use super::capture_exclusion::{CONTACT_TOLERANCE_M, ProbeExclusion, RayOutcome};
use super::capture_exclusion_test_fixtures::{
    CaptureFixture, ROOM_MAX, ROOM_MIN, TRIM_TEXTURE, exclusion, pool_entity, textured_room,
};
use super::capture_exclusion_tests::{UNITS_PER_METER, boxed};
use super::hit::RAY_EPSILON;
use super::placement_test_fixtures::{engine, textured_box_brush};
use super::shading::CaptureScene;
use crate::map_data::BrushVolume;

/// Map units per millimetre.
const MM: f64 = 0.001 * UNITS_PER_METER;

/// The generator's fixed seeds, one layout each.
const SEEDS: std::ops::Range<u64> = 0..160;

/// Rays per generated layout.
const RAYS_PER_LAYOUT: usize = 24;

/// The platform's nominal top, in map units.
const PLATFORM_TOP: f64 = 8.0;

/// SplitMix64: a fixed, dependency-free stream per seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[lo, hi)`.
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn chance(&mut self, p: f64) -> bool {
        self.range(0.0, 1.0) < p
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[(self.next() % items.len() as u64) as usize]
    }
}

/// One box brush of a layout, in map units, and whether the probe excludes it.
#[derive(Debug, Clone, Copy)]
struct Piece {
    min: [f64; 3],
    max: [f64; 3],
    excluded: bool,
}

/// A layout: the room's slabs (kept) plus `pieces`, and the rays sent into it.
#[derive(Debug, Clone)]
struct Layout {
    name: String,
    pieces: Vec<Piece>,
    /// Each ray's crossing of map x = 100, in map units, and its descent in
    /// map z per map x; its origin lies 1 m before the crossing.
    rays: Vec<([f64; 3], f64)>,
}

/// A block over a platform split into pieces along the ray, the shape every
/// case of the held-depth class takes: the block from map x 100 to 116, its
/// bottom a contact or near-contact gap over the nominal platform top; under
/// it two to four pieces butted, in contact or just past it, each topped a
/// fraction of a millimetre off nominal; an optional kept-or-excluded near
/// brush before x 100. Rays cross the block's front around its bottom edge,
/// descending at grazing slopes into the gaps.
fn generated(seed: u64) -> Layout {
    let mut rng = Rng(seed);
    let gaps_mm = [0.3, 0.8, 1.2, 1.6];
    let block_bottom = PLATFORM_TOP + rng.pick(&gaps_mm) * MM;
    let mut pieces = vec![Piece {
        min: [100.0, -8.0, block_bottom],
        max: [116.0, 8.0, 40.0],
        excluded: rng.chance(0.85),
    }];
    if rng.chance(0.6) {
        pieces.push(Piece {
            min: [84.0, -8.0, 0.0],
            max: [100.0, 8.0, PLATFORM_TOP + rng.pick(&[-0.5, 0.0, 0.3]) * MM],
            excluded: rng.chance(0.3),
        });
    }
    let count = 2 + (rng.next() % 3) as usize;
    let mut x = 100.0 + rng.pick(&[0.0, 0.0, 0.5, -0.5]);
    let tops_mm = [-0.5, 0.0, 0.0, 0.3, 0.5, 0.7];
    for k in 0..count {
        let end = if k + 1 == count {
            116.0 + rng.pick(&[0.0, 2.0])
        } else {
            (x + rng.range(1.0, 6.0)).min(115.0)
        };
        let top = (PLATFORM_TOP + rng.pick(&tops_mm) * MM).min(block_bottom - 0.05 * MM);
        pieces.push(Piece {
            min: [x, -8.0, 0.0],
            max: [end, 8.0, top],
            excluded: rng.chance(0.5),
        });
        // Butted, in CSG contact, past the capture's contact, or open.
        x = end + rng.pick(&[0.0, 0.0, 0.8, 1.6]) * MM + rng.pick(&[0.0, 0.0, 0.0, 0.5]);
        if x >= 116.0 {
            break;
        }
    }
    if !pieces.iter().any(|piece| piece.excluded) {
        pieces[0].excluded = true;
    }
    let rays = (0..RAYS_PER_LAYOUT)
        .map(|_| {
            let over = rng.range(-0.3, 2.2) * MM;
            let slope = (rng.range(0.001f64.ln(), 0.03f64.ln())).exp();
            ([100.0, rng.range(-6.0, 6.0), PLATFORM_TOP + over], slope)
        })
        .collect();
    Layout {
        name: format!("seed {seed}"),
        pieces,
        rays,
    }
}

/// R6 (`research.md` §R6 by reading) as a fixed case: the block, an
/// excluded N topped 0.5 mm over the platform and a kept K, R6's ray.
fn r6() -> Layout {
    let h = PLATFORM_TOP;
    Layout {
        name: "R6".to_string(),
        pieces: vec![
            Piece {
                min: [100.0, -8.0, h + 0.8 * MM],
                max: [116.0, 8.0, 40.0],
                excluded: true,
            },
            Piece {
                min: [100.0, -8.0, 0.0],
                max: [104.0, 8.0, h + 0.5 * MM],
                excluded: true,
            },
            Piece {
                min: [104.0, -8.0, 0.0],
                max: [116.0, 8.0, h],
                excluded: false,
            },
        ],
        rays: vec![([100.0, 0.5, h + 1.1 * MM], 0.008)],
    }
}

/// A read the oracle refuses: the trace's outcome lies past the first point
/// the ray is inside kept solid that touches an excluded hull it met.
#[derive(Debug)]
struct Leak {
    layout: String,
    ray: ([f64; 3], f64),
    outcome: RayOutcome,
    /// From the ray origin, in metres.
    bound: f64,
    read: f64,
    /// The excluded hull met and its kept contact that set `bound`.
    met: u32,
    kept: u32,
    read_brush: Option<u32>,
}

impl std::fmt::Display for Leak {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.outcome {
            RayOutcome::Hit(_) => "hit",
            RayOutcome::Fill(_) => "fill",
            RayOutcome::Miss => "miss",
            RayOutcome::LeakGuard(_) => "guard",
        };
        write!(
            f,
            "{}: ray over {:.3} mm y {:.2} slope {:.5}: {kind} on brush {:?} at {:.5} m, past {:.5} m (excluded {} met, kept {})",
            self.layout,
            (self.ray.0[2] - PLATFORM_TOP) / MM,
            self.ray.0[1],
            self.ray.1,
            self.read_brush,
            self.read,
            self.bound,
            self.met,
            self.kept,
        )
    }
}

/// Where the ray enters and leaves a convex hull, from its planes.
fn hull_span(hull: &BrushVolume, origin: DVec3, direction: DVec3) -> (f64, f64) {
    let (mut enters, mut leaves) = (f64::NEG_INFINITY, f64::INFINITY);
    for plane in &hull.planes {
        let rate = plane.normal.dot(direction);
        let inside = plane.distance - plane.normal.dot(origin);
        if rate == 0.0 {
            if inside < 0.0 {
                return (f64::INFINITY, f64::NEG_INFINITY);
            }
        } else if rate > 0.0 {
            leaves = leaves.min(inside / rate);
        } else {
            enters = enters.max(inside / rate);
        }
    }
    (enters, leaves)
}

/// The first hit the ray takes within `reach` on a face of any of `brushes`,
/// a real entry or a tolerance hit beside it, every other face set aside;
/// its distance. A ray that takes none on a brush reads the same with that
/// brush kept or excluded, so its read is no exclusion of it.
fn first_hit_on(
    room: &CaptureFixture,
    scene: &CaptureScene<'_>,
    brushes: &[u32],
    (origin, direction): (Vec3, Vec3),
    reach: f64,
) -> Option<f64> {
    let faces = room.world.face_brushes.len() as u32;
    let every = scene.tracer.face_mask(0..faces);
    let others = scene.tracer.face_mask(
        (0..faces).filter(|&face| room.face_brush(face).is_none_or(|b| !brushes.contains(&b))),
    );
    scene
        .tracer
        .closest_hit_masked(
            origin,
            direction,
            RAY_EPSILON,
            reach as f32,
            &every,
            &others,
        )
        .skipped
        .map(|hit| f64::from(hit.distance))
}

/// Whether two hulls' bounds lie within the capture's contact of each other.
fn in_contact(a: &BrushVolume, b: &BrushVolume) -> bool {
    (0..3).all(|axis| {
        a.aabb.min[axis] <= b.aabb.max[axis] + CONTACT_TOLERANCE_M
            && b.aabb.min[axis] <= a.aabb.max[axis] + CONTACT_TOLERANCE_M
    })
}

/// The no-leak bound for one ray from the brush planes: of the excluded
/// hulls `met`, every kept brush in contact with one; the first point the
/// ray is inside any of them. Kept solid the ray is already inside where it
/// first meets an excluded hull, at `first_met`, does not count: up to that
/// hit the ray is the unmasked one, so it entered that solid through a face
/// CSG removed against a kept brush, which the unmasked capture reads
/// through too (`research.md` §Audit). `None` when there is none.
fn kept_solid_bound(
    volumes: &[BrushVolume],
    excluded: &[u32],
    (met, first_met): (&[u32], f64),
    (origin, direction): (DVec3, DVec3),
) -> Option<(f64, u32, u32)> {
    let mut bound: Option<(f64, u32, u32)> = None;
    for &e in met {
        let hull = &volumes[e as usize];
        for (k, kept) in volumes.iter().enumerate() {
            if excluded.contains(&(k as u32)) || !in_contact(hull, kept) {
                continue;
            }
            let (enters, leaves) = hull_span(kept, origin, direction);
            if enters < leaves && leaves > 0.0 && enters >= first_met - HELD_DEPTH_SLACK_M {
                let first = enters.max(0.0);
                if bound.is_none_or(|(b, ..)| first < b) {
                    bound = Some((first, e, k as u32));
                }
            }
        }
    }
    bound
}

/// Every ray of `layout` the oracle refuses, carried depth on or off.
fn leaks(layout: &Layout, carried: bool) -> Vec<Leak> {
    let mut world = textured_room();
    for piece in &layout.pieces {
        world.push(textured_box_brush(piece.min, piece.max, TRIM_TEXTURE));
    }
    let room = CaptureFixture::compile(&world, &[pool_entity()]);
    let excluded: Vec<u32> = layout
        .pieces
        .iter()
        .filter(|piece| piece.excluded)
        .map(|piece| {
            let centre = [0, 1, 2].map(|axis| (piece.min[axis] + piece.max[axis]) / 2.0);
            room.brush_at(centre)
        })
        .collect();
    room.with_scene(|scene, probes| {
        let probe = boxed(&probes[0], ROOM_MIN, ROOM_MAX);
        let filter = exclusion(scene, &probe, &excluded);
        let filter = if carried {
            filter
        } else {
            filter.ignoring_held_depth()
        };
        layout
            .rays
            .iter()
            .filter_map(|&ray| refused(layout, scene, &filter, &excluded, ray, &room))
            .collect()
    })
}

/// The oracle's verdict on one ray.
fn refused(
    layout: &Layout,
    scene: &CaptureScene<'_>,
    filter: &ProbeExclusion<'_>,
    excluded: &[u32],
    (crossing, slope): ([f64; 3], f64),
    room: &CaptureFixture,
) -> Option<Leak> {
    let direction = engine([1.0, 0.0, -slope]).normalize();
    let origin = (engine(crossing) - direction).as_vec3();
    let direction: Vec3 = direction.as_vec3();
    let outcome = scene.trace(origin, direction, filter);
    let read = match outcome {
        RayOutcome::LeakGuard(_) => return None,
        RayOutcome::Hit(hit) => f64::from(hit.distance),
        RayOutcome::Fill(fill) => f64::from(fill.distance),
        RayOutcome::Miss => f64::from(scene.max_distance),
    };
    // The excluded hulls the ray crosses or passes beside before its read:
    // those whose faces it takes a hit on.
    let met: Vec<u32> = excluded
        .iter()
        .copied()
        .filter(|&e| first_hit_on(room, scene, &[e], (origin, direction), read).is_some())
        .collect();
    let first_met = first_hit_on(room, scene, &met, (origin, direction), read)?;
    let ray = (origin.as_dvec3(), direction.as_dvec3());
    let (bound, met, kept) =
        kept_solid_bound(&room.world.brushes, excluded, (&met, first_met), ray)?;
    let read_brush = match outcome {
        RayOutcome::Hit(hit) => room.face_brush(hit.face),
        RayOutcome::Fill(fill) => Some(fill.brush),
        _ => None,
    };
    (read > bound + HELD_DEPTH_SLACK_M).then(|| Leak {
        layout: layout.name.clone(),
        ray: (crossing, slope),
        outcome,
        bound,
        read,
        met,
        kept,
        read_brush,
    })
}

// No-leak oracle (`resource_management.md` §5.2 No leaks): over generated
// layouts of boxes in contact and just past it, some excluded, every ray
// into their gaps reads at or before the first point it is inside kept solid
// touching an excluded hull it met, or takes the leak guard. The bound comes
// from the brush planes, not from the trace's exit classification. R6 rides
// along as a fixed case.
#[test]
fn generated_layouts_never_read_through_kept_solid_touching_a_met_excluded_hull() {
    let found: Vec<Leak> = SEEDS
        .map(generated)
        .chain([r6()])
        .flat_map(|layout| leaks(&layout, true))
        .collect();
    let report: Vec<String> = found.iter().map(ToString::to_string).collect();
    assert!(
        found.is_empty(),
        "{} leaks:\n{}",
        found.len(),
        report.join("\n")
    );
}

// The oracle sees the class: with the carried depth ignored, it refuses R6
// and at least one generated case, so the generator reaches the class and
// the oracle above does not pass vacuously.
#[test]
fn the_no_leak_oracle_refuses_the_trace_without_the_carried_depth() {
    assert!(
        !leaks(&r6(), false).is_empty(),
        "the oracle refuses R6 uncarried"
    );
    let generated: usize = SEEDS
        .map(generated)
        .map(|layout| leaks(&layout, false).len())
        .sum();
    assert!(generated > 0, "no generated case reaches the class");
}
