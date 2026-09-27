// Alternative bounded-cost portal traversals (prototype). Throwaway.
//
// Region-per-cell propagation: each reached cell holds a grow-only screen
// region (a k-DOP in NDC: A=2 -> axis-aligned rect, A=4 -> 8-DOP with the
// x+y / x-y diagonals). Coordinates are quantized OUTWARD to a 1/Q grid so the
// lattice is finite and propagation terminates. A cell is re-processed only
// when its region grows.
//
// Facing filter: a straight ray from the camera crosses a portal plane at most
// once, from the camera side to the far side, so a chain may cross portal P
// src->dst only if the camera is on src's side of P's plane. The exact walk
// enforces the same thing implicitly (portal-plane near clip + path cycle
// check); making it explicit turns the per-frame portal graph into a DAG.
//
// Camera within APEX_EPS of a portal plane: mirror the exact walk (which
// bypasses clipping entirely there) -> accept in both directions, contribute
// the full-screen region.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

use glam::{Mat4, Vec3, Vec4};
use postretro_level_loader::LevelWorld;
use postretro_visibility::{Frustum, FrustumPlane};

pub const CLIP_EPSILON: f32 = 1e-4;
pub const APEX_EPS: f32 = 1e-3;
pub const PAD: f32 = 1e-4; // NDC pad before outward quantization
pub const W_MIN: f32 = 1e-3; // below this clip-w, projection is unstable -> use source region

const AXES: [(f32, f32); 4] = [(1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, -1.0)];
const SPAN: [i32; 4] = [1, 1, 2, 2];

/// Traversal configuration.
#[derive(Clone, Copy, Debug)]
pub struct Cfg {
    pub q: f32,          // grid steps per NDC unit
    pub facing: bool,    // camera-side facing filter
    pub ordered: bool,   // front-to-back priority worklist (else FIFO)
    pub eye_parent: bool, // portal polygon within delta of / crossing the eye plane -> parent region
    pub delta: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dop<const A: usize> {
    pub lo: [i32; A],
    pub hi: [i32; A],
}

impl<const A: usize> Dop<A> {
    pub fn full(q: f32) -> Self {
        let mut lo = [0; A];
        let mut hi = [0; A];
        for k in 0..A {
            lo[k] = -(q as i32) * SPAN[k];
            hi[k] = (q as i32) * SPAN[k];
        }
        Dop { lo, hi }
    }
    fn intersect(&self, o: &Self) -> (Self, bool) {
        let mut r = *self;
        let mut empty = false;
        for k in 0..A {
            r.lo[k] = r.lo[k].max(o.lo[k]);
            r.hi[k] = r.hi[k].min(o.hi[k]);
            if r.lo[k] > r.hi[k] {
                empty = true;
            }
        }
        (r, empty)
    }
    /// Union in place; returns true if grew.
    fn grow(&mut self, o: &Self) -> bool {
        let mut g = false;
        for k in 0..A {
            if o.lo[k] < self.lo[k] {
                self.lo[k] = o.lo[k];
                g = true;
            }
            if o.hi[k] > self.hi[k] {
                self.hi[k] = o.hi[k];
                g = true;
            }
        }
        g
    }
    pub fn push_planes(&self, rows: &[Vec4; 4], q: f32, out: &mut Vec<FrustumPlane>) {
        for k in 0..A {
            let (cx, cy) = AXES[k];
            let m = rows[0] * cx + rows[1] * cy;
            let lo = self.lo[k] as f32 / q;
            let hi = self.hi[k] as f32 / q;
            for v in [m - rows[3] * lo, rows[3] * hi - m] {
                let n = v.truncate();
                let l = n.length();
                if l > 0.0 {
                    out.push(FrustumPlane { normal: n / l, dist: v.w / l });
                }
            }
        }
    }
    /// Outward-quantized bound of projected points; None if any w < W_MIN.
    fn from_points(pts: &[Vec3], rows: &[Vec4; 4], q: f32) -> Option<Self> {
        let mut mn = [f32::MAX; A];
        let mut mx = [f32::MIN; A];
        for p in pts {
            let h = p.extend(1.0);
            let w = rows[3].dot(h);
            if !(w >= W_MIN) {
                return None;
            }
            let x = rows[0].dot(h) / w;
            let y = rows[1].dot(h) / w;
            for k in 0..A {
                let (cx, cy) = AXES[k];
                let v = cx * x + cy * y;
                mn[k] = mn[k].min(v);
                mx[k] = mx[k].max(v);
            }
        }
        let mut d = Dop { lo: [0; A], hi: [0; A] };
        for k in 0..A {
            let f = (q as i32) * SPAN[k] + 1;
            d.lo[k] = (((mn[k] - PAD) * q).floor() as i64).clamp(-f as i64, f as i64) as i32;
            d.hi[k] = (((mx[k] + PAD) * q).ceil() as i64).clamp(-f as i64, f as i64) as i32;
        }
        Some(d)
    }
}

pub fn rect_of_points(pts: &[Vec3], rows: &[Vec4; 4], q: f32) -> Option<Dop<2>> {
    Dop::<2>::from_points(pts, rows, q)
}

#[derive(Clone, Copy)]
pub struct PortalGeo {
    pub normal: Vec3,
    pub centroid: Vec3,
    /// +1 if `normal` points from front_cell toward back_cell.
    pub front_neg: f32,
}

pub struct Geo {
    pub portals: Vec<PortalGeo>,
    pub orient_fallbacks: u32,
    pub orient_bad: u32,
}

pub fn build_geo(world: &LevelWorld) -> Geo {
    let mut fallbacks = 0;
    let mut bad = 0;
    // interior points: mean of portal centroids per cell
    let n = world.cell_count();
    let mut acc = vec![(Vec3::ZERO, 0u32); n];
    for p in &world.portals {
        let c = p.polygon.iter().copied().sum::<Vec3>() / p.polygon.len().max(1) as f32;
        for cell in [p.front_cell, p.back_cell] {
            if cell < n {
                acc[cell].0 += c;
                acc[cell].1 += 1;
            }
        }
    }
    let portals = world
        .portals
        .iter()
        .map(|p| {
            let m = p.polygon.len();
            let centroid = p.polygon.iter().copied().sum::<Vec3>() / m.max(1) as f32;
            let mut nrm = Vec3::ZERO;
            for i in 0..m {
                let cur = p.polygon[i];
                let nxt = p.polygon[(i + 1) % m];
                nrm.x += (cur.y - nxt.y) * (cur.z + nxt.z);
                nrm.y += (cur.z - nxt.z) * (cur.x + nxt.x);
                nrm.z += (cur.x - nxt.x) * (cur.y + nxt.y);
            }
            let normal = nrm.normalize_or_zero();
            // primary: which cell lies on +normal side (BSP locate)
            let plus = world.locate_cell(centroid + normal * 0.005);
            let minus = world.locate_cell(centroid - normal * 0.005);
            let front_neg = if plus == p.back_cell && minus == p.front_cell {
                1.0
            } else if plus == p.front_cell && minus == p.back_cell {
                -1.0
            } else {
                fallbacks += 1;
                let ip = |c: usize| if acc[c].1 > 1 { acc[c].0 / acc[c].1 as f32 } else { (world.cells[c].bounds_min + world.cells[c].bounds_max) * 0.5 };
                let df = normal.dot(ip(p.front_cell) - centroid);
                let db = normal.dot(ip(p.back_cell) - centroid);
                if (df < 0.0) == (db < 0.0) {
                    bad += 1;
                }
                if df < db { 1.0 } else { -1.0 }
            };
            PortalGeo { normal, centroid, front_neg }
        })
        .collect();
    Geo { portals, orient_fallbacks: fallbacks, orient_bad: bad }
}

// ---------------------------------------------------------------- clipping
fn clip_to_plane(input: &[Vec3], plane: &FrustumPlane, output: &mut Vec<Vec3>) {
    let n = input.len();
    if n < 3 {
        return;
    }
    let classify = |d: f32| -> i8 {
        if d > CLIP_EPSILON {
            1
        } else if d < -CLIP_EPSILON {
            -1
        } else {
            0
        }
    };
    for i in 0..n {
        let p1 = input[i];
        let d1 = plane.normal.dot(p1) + plane.dist;
        let s1 = classify(d1);
        if s1 >= 0 {
            output.push(p1);
        }
        if s1 == 0 {
            continue;
        }
        let p2 = input[(i + 1) % n];
        let d2 = plane.normal.dot(p2) + plane.dist;
        let s2 = classify(d2);
        if s2 == 0 || s2 == s1 {
            continue;
        }
        let (f, b, df, db) = if d1 >= 0.0 { (p1, p2, d1, d2) } else { (p2, p1, d2, d1) };
        let t = df / (df - db);
        output.push(f + (b - f) * t);
    }
}

pub fn clip_poly<'a>(poly: &[Vec3], planes: &[FrustumPlane], a: &'a mut Vec<Vec3>, b: &'a mut Vec<Vec3>) -> &'a [Vec3] {
    a.clear();
    b.clear();
    if poly.len() < 3 {
        return &a[..];
    }
    a.extend_from_slice(poly);
    let mut in_a = true;
    for pl in planes {
        let (i, o) = if in_a { (&*a, &mut *b) } else { (&*b, &mut *a) };
        if i.is_empty() {
            break;
        }
        o.clear();
        clip_to_plane(i, pl, o);
        in_a = !in_a;
    }
    if in_a { &a[..] } else { &b[..] }
}

// ---------------------------------------------------------------- context
pub struct Cam {
    pub pos: Vec3,
    pub rows: [Vec4; 4],
    pub near: FrustumPlane, // slid to apex
    pub far: FrustumPlane,
}

impl Cam {
    pub fn new(pos: Vec3, vp: Mat4, fr: &Frustum) -> Self {
        let mut near = fr.planes[4];
        near.dist = -near.normal.dot(pos);
        Cam { pos, rows: [vp.row(0), vp.row(1), vp.row(2), vp.row(3)], near, far: fr.planes[5] }
    }
}

#[derive(Default, Clone, Copy, Debug)]
pub struct AltStats {
    pub steps: u32,         // portal tests (outbound portals iterated)
    pub accepted: u32,      // contributions produced
    pub processed: u32,     // cell processings (dequeues)
    pub reprop_total: u32,  // processings beyond the first per cell
    pub reprop_max: u32,    // max re-processings of any one cell
    pub camplane: u32,      // camera-on-portal-plane bypasses
    pub wmin_fallback: u32, // parent region used (eye plane / unstable projection)
    pub empty_intersect: u32,
    pub exact_steps: u32, // hybrid: exact-phase steps
    pub tripped: bool,    // hybrid: budget hit
    pub seeds: u32,
}

pub struct Scratch<const A: usize> {
    pub cfg: Cfg,
    epoch: u32,
    stamp: Vec<u32>,
    queued: Vec<u32>,
    region: Vec<Dop<A>>,
    visits: Vec<u32>,
    pub touched: Vec<usize>,
    fifo: VecDeque<usize>,
    heap: BinaryHeap<Reverse<(u32, usize)>>,
    ca: Vec<Vec3>,
    cb: Vec<Vec3>,
    planes: Vec<FrustumPlane>,
}

impl<const A: usize> Scratch<A> {
    pub fn new(n: usize, cfg: Cfg) -> Self {
        Scratch {
            cfg,
            epoch: 0,
            stamp: vec![0; n],
            queued: vec![0; n],
            region: vec![Dop::full(cfg.q); n],
            visits: vec![0; n],
            touched: Vec::new(),
            fifo: VecDeque::new(),
            heap: BinaryHeap::new(),
            ca: Vec::new(),
            cb: Vec::new(),
            planes: Vec::new(),
        }
    }
    fn begin(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.stamp.iter_mut().for_each(|s| *s = 0);
            self.queued.iter_mut().for_each(|s| *s = 0);
            self.epoch = 1;
        }
        self.touched.clear();
        self.fifo.clear();
        self.heap.clear();
    }
    pub fn reached(&self, c: usize) -> bool {
        self.stamp[c] == self.epoch
    }
    pub fn region(&self, c: usize) -> Dop<A> { self.region[c] }
    pub fn visits(&self, c: usize) -> u32 {
        if self.reached(c) { self.visits[c] } else { 0 }
    }
    fn push(&mut self, c: usize, key: f32) {
        self.queued[c] = self.epoch;
        if self.cfg.ordered {
            self.heap.push(Reverse((key.max(0.0).to_bits(), c)));
        } else {
            self.fifo.push_back(c);
        }
    }
    fn pop(&mut self) -> Option<usize> {
        if self.cfg.ordered { self.heap.pop().map(|Reverse((_, c))| c) } else { self.fifo.pop_front() }
    }
    fn offer(&mut self, c: usize, d: &Dop<A>, key: f32) {
        if self.stamp[c] != self.epoch {
            self.stamp[c] = self.epoch;
            self.region[c] = *d;
            self.visits[c] = 0;
            self.touched.push(c);
            self.push(c, key);
        } else if self.region[c].grow(d) && self.queued[c] != self.epoch {
            self.push(c, key);
        }
    }
}

/// Test portals [start..) of `cell` against `r`, offering contributions.
fn process_cell<const A: usize>(
    world: &LevelWorld,
    geo: &Geo,
    cam: &Cam,
    cell: usize,
    r: Dop<A>,
    start: usize,
    s: &mut Scratch<A>,
    st: &mut AltStats,
) {
    let cfg = s.cfg;
    let n = world.cell_count();
    s.planes.clear();
    s.planes.push(cam.near);
    r.push_planes(&cam.rows, cfg.q, &mut s.planes);
    s.planes.push(cam.far);
    let cnt = world.cell_portal_count(cell);
    for i in start..cnt {
        st.steps += 1;
        let Some(pi) = world.cell_portal_index(cell, i) else { continue };
        let p = &world.portals[pi];
        let nb = if p.front_cell == cell { p.back_cell } else { p.front_cell };
        if nb >= n || world.cells[nb].is_solid {
            continue;
        }
        let g = &geo.portals[pi];
        // signed camera distance to the portal plane
        let raw = g.normal.dot(cam.pos - g.centroid);
        // normal points front->back when front_neg=+1, so front side is negative
        let s_front = -raw * g.front_neg;
        let s_src = if p.front_cell == cell { s_front } else { -s_front };
        let (contrib, key) = if raw.abs() < APEX_EPS {
            // Mirrors the exact walk's apex-on-portal-plane bypass.
            st.camplane += 1;
            (Dop::<A>::full(cfg.q), 0.0)
        } else {
            if cfg.facing && s_src < 0.0 {
                continue;
            }
            let (planes, ca, cb) = (&s.planes, &mut s.ca, &mut s.cb);
            let clipped = clip_poly(&p.polygon, planes, ca, cb);
            if clipped.len() < 3 {
                continue;
            }
            let key = clipped.iter().map(|v| (*v - cam.pos).length_squared()).fold(f32::MAX, f32::min).sqrt();
            let eye_hit = cfg.eye_parent && p.polygon.iter().any(|v| cam.rows[3].dot(v.extend(1.0)) < cfg.delta);
            if eye_hit {
                st.wmin_fallback += 1;
                (r, key)
            } else {
                match Dop::<A>::from_points(clipped, &cam.rows, cfg.q) {
                    None => {
                        st.wmin_fallback += 1;
                        (r, key)
                    }
                    Some(d) => {
                        let (x, empty) = d.intersect(&r);
                        if empty {
                            st.empty_intersect += 1;
                            (d, key)
                        } else {
                            (x, key)
                        }
                    }
                }
            }
        };
        st.accepted += 1;
        s.offer(nb, &contrib, key);
    }
}

fn drain<const A: usize>(world: &LevelWorld, geo: &Geo, cam: &Cam, s: &mut Scratch<A>, st: &mut AltStats) {
    while let Some(c) = s.pop() {
        if s.queued[c] != s.epoch {
            continue; // stale heap entry
        }
        s.queued[c] = 0;
        s.visits[c] += 1;
        st.processed += 1;
        if s.visits[c] > 1 {
            st.reprop_total += 1;
            st.reprop_max = st.reprop_max.max(s.visits[c] - 1);
        }
        let r = s.region[c];
        process_cell(world, geo, cam, c, r, 0, s, st);
    }
}

/// Region-propagation traversal. Reached cells are in `s.touched` / `s.reached`.
pub fn region_traverse<const A: usize>(world: &LevelWorld, geo: &Geo, cam: &Cam, cam_cell: usize, s: &mut Scratch<A>) -> AltStats {
    let mut st = AltStats::default();
    s.begin();
    let full = Dop::full(s.cfg.q);
    s.offer(cam_cell, &full, 0.0);
    drain(world, geo, cam, s, &mut st);
    st
}

// ---------------------------------------------------------------- hybrid
// Exact chain walk (copy of portal_vis::flood semantics, no trace) up to a
// step budget; on trip, every unexplored remainder of the DFS stack is seeded
// into rect propagation with the rect bounding that frame's narrowed frustum.

const MAX_DEPTH: usize = 256;

struct Frame {
    cell: usize,
    rect: Dop<2>,
    cursor: usize,
}

pub struct HybScratch {
    pub visible: Vec<bool>,
    pub entries: Vec<u32>, // exact-phase flood() entries per cell (high-degree stats)
    pub ent_touched: Vec<usize>,
    frames: Vec<Frame>,
    seeds: Vec<(usize, Dop<2>, usize)>,
    path: Vec<usize>,
    ca: Vec<Vec3>,
    cb: Vec<Vec3>,
    pub rs: Scratch<2>,
}

impl HybScratch {
    pub fn new(n: usize, cfg: Cfg) -> Self {
        HybScratch {
            visible: vec![false; n],
            entries: vec![0; n],
            ent_touched: Vec::new(),
            frames: Vec::new(),
            seeds: Vec::new(),
            path: Vec::new(),
            ca: Vec::new(),
            cb: Vec::new(),
            rs: Scratch::new(n, cfg),
        }
    }
}

struct HybState<'a> {
    world: &'a LevelWorld,
    cam: &'a Cam,
    hideg: Option<&'a [bool]>,
    q: f32,
    budget: u32,
    considered: u32,
    tripped: bool,
}

fn snapshot(h: &mut HybScratch) {
    h.seeds.clear();
    let last = h.frames.len() - 1;
    for (k, f) in h.frames.iter().enumerate() {
        let start = if k == last { f.cursor } else { f.cursor + 1 };
        h.seeds.push((f.cell, f.rect, start));
    }
}

fn hyb_flood(st: &mut HybState, h: &mut HybScratch, cell: usize, frustum: &Frustum, rect: Dop<2>) {
    h.visible[cell] = true;
    if let Some(hd) = st.hideg {
        if hd[cell] {
            if h.entries[cell] == 0 {
                h.ent_touched.push(cell);
            }
            h.entries[cell] += 1;
        }
    }
    h.frames.push(Frame { cell, rect, cursor: 0 });
    if st.considered >= st.budget {
        st.tripped = true;
        snapshot(h);
        h.frames.pop();
        return;
    }
    if h.path.len() >= MAX_DEPTH {
        h.frames.pop();
        return;
    }
    let world = st.world;
    let n = world.cell_count();
    let outbound = world.cell_portal_count(cell);
    for i in 0..outbound {
        h.frames.last_mut().unwrap().cursor = i;
        if st.considered >= st.budget {
            st.tripped = true;
            snapshot(h);
            h.frames.pop();
            return;
        }
        let Some(pi) = world.cell_portal_index(cell, i) else { continue };
        let portal = &world.portals[pi];
        let nb = if portal.front_cell == cell { portal.back_cell } else { portal.front_cell };
        st.considered += 1;
        if nb >= n {
            continue;
        }
        if h.path.contains(&pi) {
            continue;
        }
        if world.cells[nb].is_solid {
            continue;
        }
        let res = {
            if crate::portal_vis::camera_on_polygon_plane(st.cam.pos, &portal.polygon) {
                crate::portal_vis::narrow_frustum(st.cam.pos, &portal.polygon, frustum).map(|f| (f, Dop::<2>::full(st.q)))
            } else {
                let (ca, cb) = (&mut h.ca, &mut h.cb);
                let clipped = crate::portal_vis::clip_polygon_to_frustum(&portal.polygon, frustum, ca, cb);
                if clipped.len() < 3 {
                    None
                } else {
                    crate::portal_vis::narrow_frustum(st.cam.pos, clipped, frustum).map(|f| {
                        let r = match rect_of_points(clipped, &st.cam.rows, st.q) {
                            None => rect,
                            Some(d) => {
                                let (x, e) = d.intersect(&rect);
                                if e { d } else { x }
                            }
                        };
                        (f, r)
                    })
                }
            }
        };
        let Some((narrowed, child_rect)) = res else { continue };
        h.path.push(pi);
        hyb_flood(st, h, nb, &narrowed, child_rect);
        h.path.pop();
        if st.tripped {
            h.frames.pop();
            return;
        }
    }
    h.frames.pop();
}

/// Returns stats; visible = h.visible[c] || h.rs.reached(c) (when tripped).
pub fn hybrid_traverse(
    world: &LevelWorld,
    geo: &Geo,
    cam: &Cam,
    fr: &Frustum,
    cam_cell: usize,
    budget: u32,
    hideg: Option<&[bool]>,
    h: &mut HybScratch,
) -> AltStats {
    h.visible.iter_mut().for_each(|v| *v = false);
    for &c in &h.ent_touched {
        h.entries[c] = 0;
    }
    h.ent_touched.clear();
    h.frames.clear();
    h.path.clear();
    let mut vf = fr.clone();
    vf.planes[4] = cam.near;
    let q = h.rs.cfg.q;
    let mut hs = HybState { world, cam, hideg, q, budget, considered: 0, tripped: false };
    hyb_flood(&mut hs, h, cam_cell, &vf, Dop::full(q));
    let mut st = AltStats { exact_steps: hs.considered, steps: hs.considered, tripped: hs.tripped, ..Default::default() };
    h.rs.begin();
    if hs.tripped {
        let seeds = std::mem::take(&mut h.seeds);
        st.seeds = seeds.len() as u32;
        for &(c, r, start) in &seeds {
            process_cell(world, geo, cam, c, r, start, &mut h.rs, &mut st);
        }
        h.seeds = seeds;
        drain(world, geo, cam, &mut h.rs, &mut st);
    }
    st
}
