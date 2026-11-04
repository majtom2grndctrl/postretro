// Capture exclusion no-leak oracle: generated box layouts in and near contact, rays into their gaps, each read checked against kept solid from the brush planes.
// See: context/lib/resource_management.md §5.2 (Exclusion)

use glam::{DVec3, Vec3};

use super::capture_bounds::HELD_DEPTH_SLACK_M;
use super::capture_exclusion::{CONTACT_TOLERANCE_M, RayOutcome};
use super::capture_exclusion_test_fixtures::{
    CaptureFixture, ROOM_MAX, ROOM_MIN, TRIM_TEXTURE, exclusion, pool_entity, textured_room,
};
use super::capture_exclusion_tests::{UNITS_PER_METER, boxed};
use super::placement_test_fixtures::{engine, textured_box_brush};
use super::shading::CaptureScene;
use crate::map_data::BrushVolume;
use crate::ray_traversal::TRACER_TOLERANCE_METERS;

/// Map units per millimetre.
const MM: f64 = 0.001 * UNITS_PER_METER;

/// The generator's fixed seeds, one layout each.
const SEEDS: std::ops::Range<u64> = 0..160;

/// Rays per generated layout.
const RAYS_PER_LAYOUT: usize = 24;

/// The fixed case [`kept_behind`]'s name.
const KEPT_BEHIND: &str = "kept behind";

/// The platform's nominal top, in map units.
const PLATFORM_TOP: f64 = 8.0;

/// A leak outside the held-depth class, recorded in §5.2 (Known leaks) and not
/// fixed. All are grazing rays in sub-millimetre gaps. Each trace reaches its
/// read carrying no depth, which the oracle asserts: that keeps them outside
/// the class. The variant records the construction found by tracing; no test
/// reads it.
#[derive(Debug, Clone, Copy)]
enum KnownLimit {
    /// An excluded near brush's back and the excluded block's front are
    /// coplanar. The ray's tolerance hit on the block's front lands one `f32`
    /// step before the near brush's exit. The re-trace restarts from that
    /// exit, past the hit, so the pass beside is never classified.
    CoplanarPassSteppedOver,
    /// The ray takes hits on the block and an excluded piece at one `f32`
    /// distance, at least one a real entry. Hit order takes the piece, so the
    /// block's exit or pass beside, held by that piece and a kept one, is
    /// never classified.
    TiedEntryNeverClassified,
    /// Two excluded pieces hold the block's exit; the nearer is taken. The
    /// ray then enters the other through a top CSG removed, unseen, and
    /// leaves it into kept solid through a face CSG removed against it.
    /// The excluded-depth sibling of the held-depth class.
    ExcludedNeighbourPassedOver,
    /// The ray enters an excluded piece through a face CSG removed, taking no
    /// hit, so the trace never classifies it, and leaves it into kept solid
    /// through a face CSG removed against it.
    UnseenExcludedHull,
    /// With an excluded brush gone, the ray reaches a kept face CSG removed
    /// against an excluded piece it passes without a hit, and reads through.
    ExposedCsgHole,
}

/// The rays the oracle skips: layout, index into its rays, and why. Named per
/// ray, not per layout, so every other ray of these layouts stays checked.
const KNOWN_LIMITS: &[(&str, usize, KnownLimit)] = &[
    ("seed 20", 0, KnownLimit::UnseenExcludedHull),
    ("seed 20", 7, KnownLimit::CoplanarPassSteppedOver),
    ("seed 20", 8, KnownLimit::CoplanarPassSteppedOver),
    ("seed 20", 18, KnownLimit::UnseenExcludedHull),
    ("seed 20", 23, KnownLimit::CoplanarPassSteppedOver),
    ("seed 23", 16, KnownLimit::ExposedCsgHole),
    ("seed 29", 0, KnownLimit::TiedEntryNeverClassified),
    ("seed 29", 4, KnownLimit::TiedEntryNeverClassified),
    ("seed 29", 9, KnownLimit::TiedEntryNeverClassified),
    ("seed 29", 15, KnownLimit::TiedEntryNeverClassified),
    ("seed 29", 16, KnownLimit::TiedEntryNeverClassified),
    ("seed 43", 4, KnownLimit::CoplanarPassSteppedOver),
    ("seed 43", 5, KnownLimit::UnseenExcludedHull),
    ("seed 43", 10, KnownLimit::UnseenExcludedHull),
    ("seed 43", 15, KnownLimit::UnseenExcludedHull),
    ("seed 43", 18, KnownLimit::UnseenExcludedHull),
    ("seed 43", 22, KnownLimit::CoplanarPassSteppedOver),
    ("seed 48", 10, KnownLimit::UnseenExcludedHull),
    ("seed 57", 8, KnownLimit::ExcludedNeighbourPassedOver),
    ("seed 64", 7, KnownLimit::ExposedCsgHole),
    ("seed 150", 16, KnownLimit::CoplanarPassSteppedOver),
];

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

/// A block over a platform split into pieces along the ray: the block from map x 100 to 116, its
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

/// R6 as a fixed case: the block, an
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

/// The kept-behind row as a fixed case: kept solid passed over at the block's
/// exit whose front stands at the carried depth, so the ray reads it. A kept
/// near brush and a 1 u kept sliver, the excluded brush from map x 101 to 105,
/// a kept brush from 106.
fn kept_behind() -> Layout {
    let h = PLATFORM_TOP;
    let platform = |x0: f64, x1: f64, excluded: bool| Piece {
        min: [x0, -8.0, 0.0],
        max: [x1, 8.0, h],
        excluded,
    };
    Layout {
        name: KEPT_BEHIND.to_string(),
        pieces: vec![
            Piece {
                min: [100.0, -8.0, h + 0.8 * MM],
                max: [116.0, 8.0, 40.0],
                excluded: true,
            },
            platform(84.0, 100.0, false),
            platform(100.0, 101.0, false),
            platform(101.0, 105.0, true),
            platform(106.0, 116.0, false),
        ],
        rays: vec![([100.0, 0.5, h + 0.6 * MM], 0.006)],
    }
}

/// A read the oracle refuses: the trace's outcome lies past the first point
/// the ray is inside kept solid that touches an excluded hull it crossed or
/// passed beside.
#[derive(Debug)]
struct Leak {
    layout: String,
    /// Index into the layout's rays.
    index: usize,
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

/// Whether `KNOWN_LIMITS` names ray `index` of `layout`.
fn is_known_limit(layout: &str, index: usize) -> bool {
    KNOWN_LIMITS
        .iter()
        .any(|&(named, ray, _)| named == layout && ray == index)
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
            "{} ray {}: over {:.3} mm y {:.2} slope {:.5}: {kind} on brush {:?} at {:.5} m, past {:.5} m (excluded {} met, kept {})",
            self.layout,
            self.index,
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

/// Where the ray enters and leaves a convex hull, from its planes, each moved
/// `grow` outward.
fn hull_span(hull: &BrushVolume, (origin, direction): (DVec3, DVec3), grow: f64) -> (f64, f64) {
    let (mut enters, mut leaves) = (f64::NEG_INFINITY, f64::INFINITY);
    for plane in &hull.planes {
        let rate = plane.normal.dot(direction);
        let inside = plane.distance + grow - plane.normal.dot(origin);
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

/// Whether two hulls' bounds lie within the capture's contact of each other.
fn in_contact(a: &BrushVolume, b: &BrushVolume) -> bool {
    (0..3).all(|axis| {
        a.aabb.min[axis] <= b.aabb.max[axis] + CONTACT_TOLERANCE_M
            && b.aabb.min[axis] <= a.aabb.max[axis] + CONTACT_TOLERANCE_M
    })
}

/// The no-leak bound for one ray, from the brush planes alone: of the excluded
/// hulls it crosses or passes within τ of before `read`, every kept brush in
/// contact with one; the first point the ray is inside any of them. Kept
/// solid entered before `unmasked`, the unmasked capture's read, does not
/// count: up to there the ray is the unmasked one, so it entered that solid
/// through a face CSG removed, which the unmasked capture reads through too.
/// `None` when there is none.
fn kept_solid_bound(
    volumes: &[BrushVolume],
    excluded: &[u32],
    (read, unmasked): (f64, f64),
    ray: (DVec3, DVec3),
) -> Option<(f64, u32, u32)> {
    let mut bound: Option<(f64, u32, u32)> = None;
    for &e in excluded {
        let hull = &volumes[e as usize];
        let (enters, leaves) = hull_span(hull, ray, f64::from(TRACER_TOLERANCE_METERS));
        if !(enters < leaves && leaves > 0.0 && enters < read) {
            continue;
        }
        for (k, kept) in volumes.iter().enumerate() {
            if excluded.contains(&(k as u32)) || !in_contact(hull, kept) {
                continue;
            }
            let (enters, leaves) = hull_span(kept, ray, 0.0);
            if enters < leaves && leaves > 0.0 && enters >= unmasked - HELD_DEPTH_SLACK_M {
                let first = enters.max(0.0);
                if bound.is_none_or(|(b, ..)| first < b) {
                    bound = Some((first, e, k as u32));
                }
            }
        }
    }
    bound
}

/// One ray's traces and the oracle's verdict on each.
struct Verdict {
    layout: String,
    index: usize,
    carried: RayOutcome,
    uncarried: RayOutcome,
    /// Whether the trace carried a depth to its read. A read the depth
    /// refused is a leak guard either way, so this holds only of a read.
    carried_a_depth: bool,
    leak: Option<Leak>,
    uncarried_leak: Option<Leak>,
}

/// Every ray of `layout`, traced with the carried depth on and off.
fn judged(layout: &Layout) -> Vec<Verdict> {
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
        let control = exclusion(scene, &probe, &excluded).ignoring_held_depth();
        let refusing = exclusion(scene, &probe, &excluded).refusing_any_held_depth();
        let plain = exclusion(scene, &probe, &[]);
        let judge = |index: usize, (crossing, slope): ([f64; 3], f64)| {
            let direction = engine([1.0, 0.0, -slope]).normalize();
            let origin = (engine(crossing) - direction).as_vec3();
            let direction: Vec3 = direction.as_vec3();
            let unmasked = scene.trace(origin, direction, &plain);
            let refuses = |outcome: RayOutcome| {
                refused(
                    (layout, index),
                    (&room, scene, &excluded),
                    (origin, direction),
                    (outcome, unmasked),
                )
            };
            let carried = scene.trace(origin, direction, &filter);
            let uncarried = scene.trace(origin, direction, &control);
            Verdict {
                layout: layout.name.clone(),
                index,
                carried,
                uncarried,
                carried_a_depth: scene.trace(origin, direction, &refusing) != carried,
                leak: refuses(carried),
                uncarried_leak: refuses(uncarried),
            }
        };
        layout
            .rays
            .iter()
            .enumerate()
            .map(|(index, &ray)| judge(index, ray))
            .collect()
    })
}

/// How far along the ray `outcome` reads; `None` for the leak guard.
fn read_at(outcome: RayOutcome, scene: &CaptureScene<'_>) -> Option<f64> {
    match outcome {
        RayOutcome::LeakGuard(_) => None,
        RayOutcome::Hit(hit) => Some(f64::from(hit.distance)),
        RayOutcome::Fill(fill) => Some(f64::from(fill.distance)),
        RayOutcome::Miss => Some(f64::from(scene.max_distance)),
    }
}

/// The oracle's verdict on one trace of one ray. A read the unmasked capture
/// makes too is no exclusion's doing.
fn refused(
    (layout, index): (&Layout, usize),
    (room, scene, excluded): (&CaptureFixture, &CaptureScene<'_>, &[u32]),
    (origin, direction): (Vec3, Vec3),
    (outcome, unmasked): (RayOutcome, RayOutcome),
) -> Option<Leak> {
    let read = read_at(outcome, scene)?;
    if outcome == unmasked {
        return None;
    }
    let reads = (read, read_at(unmasked, scene)?);
    let ray = (origin.as_dvec3(), direction.as_dvec3());
    let (bound, met, kept) = kept_solid_bound(&room.world.brushes, excluded, reads, ray)?;
    let read_brush = match outcome {
        RayOutcome::Hit(hit) => room.face_brush(hit.face),
        RayOutcome::Fill(fill) => Some(fill.brush),
        _ => None,
    };
    (read > bound + HELD_DEPTH_SLACK_M).then(|| Leak {
        layout: layout.name.clone(),
        index,
        ray: layout.rays[index],
        outcome,
        bound,
        read,
        met,
        kept,
        read_brush,
    })
}

/// Every generated ray's verdict, and the fixed cases'.
fn all_verdicts() -> Vec<Verdict> {
    SEEDS
        .map(generated)
        .chain([r6(), kept_behind()])
        .flat_map(|layout| judged(&layout))
        .collect()
}

// No-leak oracle (`resource_management.md` §5.2 No leaks): over generated
// layouts of boxes in contact and just past it, some excluded, every ray
// into their gaps reads at or before the first point it is inside kept solid
// touching an excluded hull it crossed or passed beside, or takes the leak
// guard. The bound comes from the brush planes, not from the trace's exit
// classification. R6 rides along as a fixed case. `KNOWN_LIMITS` are skipped.
// Each must still leak, and reach its read carrying no depth, which keeps it
// outside the class: one that stops is closed, and leaves the table and §5.2.
#[test]
fn generated_layouts_never_read_through_kept_solid_touching_a_met_excluded_hull() {
    let verdicts = all_verdicts();
    let unnamed: Vec<String> = verdicts
        .iter()
        .filter_map(|verdict| verdict.leak.as_ref())
        .filter(|leak| !is_known_limit(&leak.layout, leak.index))
        .map(ToString::to_string)
        .collect();
    assert!(
        unnamed.is_empty(),
        "{} leaks:\n{}",
        unnamed.len(),
        unnamed.join("\n")
    );
    let stale: Vec<_> = KNOWN_LIMITS
        .iter()
        .filter(|&&(layout, index, _)| {
            !verdicts.iter().any(|verdict| {
                verdict.layout == layout
                    && verdict.index == index
                    && verdict.leak.is_some()
                    && !verdict.carried_a_depth
            })
        })
        .collect();
    assert!(
        stale.is_empty(),
        "no longer leak, or now carry a depth; drop them here and from §5.2: {stale:?}"
    );
}

// The oracle sees the class: with the carried depth ignored, it refuses R6
// and at least one generated case the known limits do not name, so the
// generator reaches the class and the oracle above does not pass vacuously.
#[test]
fn the_no_leak_oracle_refuses_the_trace_without_the_carried_depth() {
    let refused: Vec<(String, usize)> = all_verdicts()
        .iter()
        .filter_map(|verdict| verdict.uncarried_leak.as_ref())
        .filter(|leak| !is_known_limit(&leak.layout, leak.index))
        .map(|leak| (leak.layout.clone(), leak.index))
        .collect();
    assert!(
        refused.iter().any(|(layout, _)| layout == "R6"),
        "the oracle refuses R6 uncarried"
    );
    assert!(
        refused.iter().any(|(layout, _)| layout.starts_with("seed")),
        "no generated case reaches the class"
    );
}

// The carried depth agrees with the oracle: over the same rays, it changes only
// reads the oracle refuses without it. The oracle above accepts every leak
// guard, so this is what holds a depth to the rule from the other side. It
// shares the slack with the trace, so it says nothing finer than that; the
// kept-behind row is the proof at the depth. Rays that carry a depth and read
// are the ones a too-tight depth would guard: kept behind is pinned among them.
#[test]
fn the_carried_depth_changes_only_reads_the_oracle_refuses_without_it() {
    let verdicts = all_verdicts();
    let unrefused: Vec<String> = verdicts
        .iter()
        .filter(|verdict| verdict.carried != verdict.uncarried && verdict.uncarried_leak.is_none())
        .map(|verdict| format!("{} ray {}", verdict.layout, verdict.index))
        .collect();
    assert!(unrefused.is_empty(), "{unrefused:?}");
    assert!(
        verdicts.iter().any(|verdict| verdict.layout == KEPT_BEHIND
            && verdict.carried_a_depth
            && matches!(verdict.carried, RayOutcome::Hit(_))),
        "kept behind carries a depth and reads its front"
    );
}
