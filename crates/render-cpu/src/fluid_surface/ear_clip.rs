// Fluid surface ear clipping: triangulates a grid part, or a whole face with
// a collinear vertex, over its own vertices.
// See: context/lib/rendering_pipeline.md §7.3a

use std::collections::BTreeSet;

use glam::DVec3;

use super::COLLINEAR_DISTANCE;
use super::ear_index::ReflexIndex;

/// A ring turns at a vertex when the turn's sine exceeds this. f64 rounding
/// of the turn stays far below it. A vertex whose turn falls under it, such
/// as an f32 position off a long edge by an ulp near the origin, is treated
/// as collinear and later clipped into a corner, so nothing is lost.
const CORNER_SINE: f64 = 1.0e-12;

/// Triangulate a polygon within about 0.1 mm of convex over its own
/// vertices: a grid part, or a whole face with a collinear vertex. Collinear
/// vertices lie on its edges (the compiler's T-junction inserts, and planes
/// passing near a corner); each stays a triangle corner. Each step clips an
/// ear beside a vertex that is no corner, which turns a collinear one into a
/// corner, so three corners always remain; with none, any ear. Within either
/// class it takes the ear whose chord, the segment between its tip's two live
/// neighbours, is shortest as packed, then the earliest slot, so the order is
/// deterministic. Corners are judged on the positions as packed, in f32, so a
/// part keeps its drawn area however far from the origin, down to a final
/// triangle judged at one vertex: area under about 1e-12 of its longer sides'
/// product can be dropped. A part with no corner left has no area and draws
/// nothing. A vertex repeating the one before it, as packed, joins the ring
/// once, its twin covering the position: the repeat draws no triangle, and
/// its zero-length edge turns nowhere, so it would block every clip beside
/// it.
///
/// An ear is a corner whose triangle with its two live neighbours holds no
/// reflex vertex (one turning against the polygon past the corner threshold),
/// on its edges included. A compiled face is simple but need not be convex:
/// an insert a hair inside an edge is a reflex dent, and at an acute corner
/// a clip unchecked for it can reach past the dent, draw a strip as long as
/// the clip's chord twice, and wind its last triangle backward. On a simple ring
/// a corner whose triangle holds no reflex vertex holds no vertex strictly
/// inside it, so no clip draws area twice or loses any and the clips tile the
/// polygon exactly. It can still hold one on its chord: in the ring (0, 0),
/// (1, 0), (2, 0), (1, 1), which has no reflex vertex, the ear at (1, 1) holds
/// (1, 0), and clipping it first would leave (1, 0) no corner and its two
/// segments bordering no triangle. Clipping a corner beside a non-corner
/// first, (0, 0) or (2, 0) there, turns (1, 0) into a corner; that order is
/// load-bearing for every vertex staying a corner, so the class comes before
/// the chord. A ring that is not simple can leave no ear; the clip then takes
/// the earliest corner in ring order beside a non-corner, else the earliest
/// corner, as if every corner were an ear, so it still terminates.
///
/// Bounded in n log n of the part's vertices, which the compiler's T-junction
/// inserts can number in thousands. A clip changes the turn and the chord of
/// the two vertices beside the tip alone, so the ring keeps its candidates
/// ordered, each ear under the key its current chord gives it, and re-files
/// only those near the tip, log n each. Clipping an ear narrows the polygon's
/// angle at each neighbour by the ear's angle there, so in exact arithmetic a
/// vertex's turn only grows: a reflex vertex can stop being reflex, and no
/// vertex starts, collinear ones included. The live reflex vertices are kept
/// in a static tree over the ring's packed positions ([`ReflexIndex`]), built
/// once when the first appears, n log n; each change is a root-to-leaf walk,
/// log n, at most once per vertex. A vertex that rounding at the threshold or
/// a fallback clip turns reflex later joins the tree the same way. An ear is
/// searched when it comes up to be clipped; one found blocked is set aside
/// under the reflex vertex inside it until that vertex turns or the ear's
/// neighbours change, so a fixed ear is searched again at most once per reflex
/// vertex inside it, which on a compiled face is rarely more than one. A
/// search skips every node holding no live reflex vertex, or whose box misses
/// the ear or lies wholly past one of its edges, so it costs the nodes whose
/// boxes straddle the ear. On a ring within 0.1 mm of convex the reflex
/// vertices lie along its boundary, and a median tree over points along a
/// curve has a bounded number of nodes per level near any one point, so a
/// search whose ear meets the boundary only near its own corners visits order
/// log n nodes. The chord order is what keeps ears that local. An ear lies
/// within half its chord of the segment from its tip to the chord's midpoint,
/// and shortest chords first merge a dented run locally, neighbouring dents
/// before longer spans, and cross an acute tip one step at a time, so on the
/// dented fixtures no search takes more than a few dozen counted steps.
/// Clipping the earliest slot first instead fanned from a pivot near such a
/// tip to vertices tens of millimetres out: each fan triangle ran within a
/// dent's depth of live dents along its whole length, no box could be told
/// apart from it, and each search grew with the dented run. The class can
/// still force a fan: at an acute tip where one edge has merged into a run of
/// collinear vertices, the corner at that run's end stays beside a
/// non-corner, the corners across the tip need not, and its ears fan from
/// there across the other edge's live dents. That is an argument for compiled
/// faces, checked by count on dented fixtures, not a worst-case bound for any
/// simple ring.
pub(super) fn triangulate_convex(
    polygon: &[u32],
    positions: &[DVec3],
    normal: DVec3,
    triangles: &mut Vec<[u32; 3]>,
) {
    let packed = |i: u32| positions[i as usize].as_vec3().as_dvec3();
    super::count_install_steps(polygon.len());
    let mut vertices: Vec<u32> = Vec::with_capacity(polygon.len());
    for &vertex in polygon {
        if vertices
            .last()
            .is_none_or(|&last| packed(last) != packed(vertex))
        {
            vertices.push(vertex);
        }
    }
    while let [first, .., last] = vertices[..]
        && packed(first) == packed(last)
    {
        vertices.pop();
    }
    let count = vertices.len();
    if count < 3 {
        return;
    }
    // A ring turning at every vertex, each further than the collinear
    // tolerance from its neighbours' chord, is convex and no fan diagonal
    // passes near a vertex (in a convex ring that chord is the nearest any
    // diagonal comes): it fans from its first, as most grid parts do, with
    // none of the clip bookkeeping.
    super::count_install_steps(count);
    if (0..count).all(|i| {
        let at = |slot: usize| packed(vertices[slot % count]);
        let (prev, here, next) = (at(i + count - 1), at(i), at(i + 1));
        let chord = next - prev;
        ring_turn(prev, here, next, normal) == Turn::Corner
            && (here - prev).cross(chord).length() > COLLINEAR_DISTANCE * chord.length()
    }) {
        for i in 1..count - 1 {
            triangles.push([vertices[0], vertices[i], vertices[i + 1]]);
        }
        return;
    }
    let mut ring = ClipRing::new(vertices.iter().map(|&v| packed(v)).collect(), normal);
    let (mut live, mut any_live) = (count, 0);
    while live > 3 {
        let ear = ring
            .ears_beside
            .first()
            .or_else(|| ring.ears.first())
            .map(|&(_, slot)| slot);
        let tip = match ear {
            Some(tip) => {
                if ring.blocked_by_reflex(tip) {
                    continue;
                }
                tip
            }
            // No ear clear of reflex vertices: a ring that is not simple.
            None => {
                let corner = ring.corners_beside.first().or_else(|| ring.corners.first());
                match corner.copied() {
                    Some(tip) => tip,
                    None => return,
                }
            }
        };
        let (before, after) = (ring.prev[tip], ring.next[tip]);
        triangles.push([vertices[before], vertices[tip], vertices[after]]);
        ring.next[before] = after;
        ring.prev[after] = before;
        ring.set_turn(tip, Turn::Collinear);
        ring.file(tip);
        live -= 1;
        any_live = after;
        for slot in [before, after] {
            let turn = ring.turn_at(slot);
            ring.set_turn(slot, turn);
            ring.blocked[slot] = false;
        }
        let released = std::mem::take(&mut ring.released);
        for &slot in &released {
            ring.blocked[slot] = false;
        }
        for slot in [ring.prev[before], before, after, ring.next[after]]
            .into_iter()
            .chain(released.iter().copied())
        {
            ring.file(slot);
        }
        ring.released = released;
        ring.released.clear();
    }
    // The last three in ring order, from the earliest slot.
    let second = ring.next[any_live];
    let mut last = [any_live, second, ring.next[second]];
    let first = (0..3).min_by_key(|&i| last[i]).expect("three slots");
    last.rotate_left(first);
    let [a, b, c] = last;
    if ring_turn(ring.points[a], ring.points[b], ring.points[c], normal) == Turn::Corner {
        triangles.push([a, b, c].map(|slot| vertices[slot]));
    }
}

/// How a ring turns at a vertex, about the polygon's normal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Turn {
    /// Toward the polygon's inside, the turn's sine past [`CORNER_SINE`].
    Corner,
    /// Within [`CORNER_SINE`] either way: on the line through its
    /// neighbours. A clipped slot reads this too.
    Collinear,
    /// Away from the inside past [`CORNER_SINE`]: a dent.
    Reflex,
}

/// How the ring turns at `at`, entered from `prev` and left toward `next`.
fn ring_turn(prev: DVec3, at: DVec3, next: DVec3, normal: DVec3) -> Turn {
    super::count_install_steps(1);
    let (into, out) = (at - prev, next - at);
    let sine = into.cross(out).dot(normal);
    let threshold = CORNER_SINE * into.length() * out.length();
    if sine > threshold {
        Turn::Corner
    } else if sine < -threshold {
        Turn::Reflex
    } else {
        Turn::Collinear
    }
}

/// An ear's place in clip order: the squared length of its chord, the
/// segment between its tip's two live neighbours as packed, as bits, then its
/// slot. Non-negative f64 bit patterns order as the values do.
type EarKey = (u64, usize);

/// A part's ring under ear clipping: each live slot linked to its live
/// neighbours, and the slots a clip may take, the ears by [`EarKey`] and the
/// fallback corners by slot.
struct ClipRing {
    /// Each slot's packed position.
    points: Vec<DVec3>,
    normal: DVec3,
    prev: Vec<usize>,
    next: Vec<usize>,
    /// How the ring turns at each slot, judged on its live neighbours.
    turn: Vec<Turn>,
    /// Corners whose ear a reflex vertex was found inside, until it turns or
    /// the corner's neighbours change.
    blocked: Vec<bool>,
    /// The corners set aside under each reflex slot, by [`Self::blocked`].
    waiting: Vec<Vec<usize>>,
    /// Corners set aside under reflex slots that just turned, to re-file.
    released: Vec<usize>,
    /// The live reflex slots, from the first one onward.
    reflex: Option<ReflexIndex>,
    /// Corners not set aside, beside a slot that is no corner: clipping one
    /// grows that slot's turn, making a collinear one a corner.
    ears_beside: BTreeSet<EarKey>,
    /// Corners not set aside.
    ears: BTreeSet<EarKey>,
    /// Each slot's key while it is filed among the ears, so re-filing it
    /// finds the entry its old chord made.
    ear_key: Vec<Option<EarKey>>,
    /// Every corner beside a slot that is no corner, and every corner: the
    /// fallback when no ear is left.
    corners_beside: BTreeSet<usize>,
    corners: BTreeSet<usize>,
}

impl ClipRing {
    /// The ring over `points` in order, every slot judged and filed.
    fn new(points: Vec<DVec3>, normal: DVec3) -> Self {
        let count = points.len();
        let mut ring = Self {
            points,
            normal,
            prev: (0..count).map(|slot| (slot + count - 1) % count).collect(),
            next: (0..count).map(|slot| (slot + 1) % count).collect(),
            turn: vec![Turn::Collinear; count],
            blocked: vec![false; count],
            waiting: vec![Vec::new(); count],
            released: Vec::new(),
            reflex: None,
            ears_beside: BTreeSet::new(),
            ears: BTreeSet::new(),
            ear_key: vec![None; count],
            corners_beside: BTreeSet::new(),
            corners: BTreeSet::new(),
        };
        for slot in 0..count {
            let turn = ring.turn_at(slot);
            ring.set_turn(slot, turn);
        }
        for slot in 0..count {
            ring.file(slot);
        }
        ring
    }

    fn turn_at(&self, slot: usize) -> Turn {
        ring_turn(
            self.points[self.prev[slot]],
            self.points[slot],
            self.points[self.next[slot]],
            self.normal,
        )
    }

    /// Records `slot`'s turn, keeping the reflex index to the live reflex
    /// slots: a slot that stops being reflex releases the corners set aside
    /// under it.
    fn set_turn(&mut self, slot: usize, turn: Turn) {
        let was = std::mem::replace(&mut self.turn[slot], turn);
        if (was == Turn::Reflex) == (turn == Turn::Reflex) {
            return;
        }
        if turn == Turn::Reflex {
            let points = &self.points;
            self.reflex
                .get_or_insert_with(|| ReflexIndex::new(points))
                .set_active(slot, true);
        } else {
            if let Some(reflex) = &mut self.reflex {
                reflex.set_active(slot, false);
            }
            self.released.append(&mut self.waiting[slot]);
        }
    }

    /// Whether a live reflex slot lies in `tip`'s ear; if so, sets the tip
    /// aside under it and re-files it.
    fn blocked_by_reflex(&mut self, tip: usize) -> bool {
        super::count_install_steps(1);
        let Some(reflex) = self.reflex.as_mut().filter(|reflex| reflex.any_active()) else {
            return false;
        };
        let ear = [self.prev[tip], tip, self.next[tip]];
        let Some(inside) = reflex.inside(ear, &self.points, self.normal) else {
            return false;
        };
        super::count_blocked_ear();
        self.blocked[tip] = true;
        self.waiting[inside].push(tip);
        self.file(tip);
        true
    }

    /// Re-file `slot` among the candidates from its and its neighbours'
    /// turns and its chord. Called whenever its neighbours change, so its
    /// key tracks its chord.
    fn file(&mut self, slot: usize) {
        super::count_install_steps(1);
        let corner = self.turn[slot] == Turn::Corner;
        let beside = corner
            && !(self.turn[self.prev[slot]] == Turn::Corner
                && self.turn[self.next[slot]] == Turn::Corner);
        for (set, member) in [
            (&mut self.corners, corner),
            (&mut self.corners_beside, beside),
        ] {
            if member {
                set.insert(slot);
            } else {
                set.remove(&slot);
            }
        }
        if let Some(key) = self.ear_key[slot].take() {
            self.ears.remove(&key);
            self.ears_beside.remove(&key);
        }
        if corner && !self.blocked[slot] {
            let (before, after) = (self.points[self.prev[slot]], self.points[self.next[slot]]);
            let key = (after.distance_squared(before).to_bits(), slot);
            self.ears.insert(key);
            if beside {
                self.ears_beside.insert(key);
            }
            self.ear_key[slot] = Some(key);
        }
    }
}
