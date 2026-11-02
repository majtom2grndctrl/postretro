// Scratch analysis: portal/cell stats and rect-vs-exact-vs-seen decomposition. Not part of the engine.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::fs::File;

use glam::{Mat4, Vec3};

use crate::region_walk::oracle::oracle_from_camera;
use crate::region_walk::{MAX_PORTAL_WALK_STEPS, PortalWalkScratch, WalkView, WalkWorld, walk_from_camera};
use crate::visibility::{Frustum, extract_frustum_planes, is_aabb_outside_frustum};
use postretro_level_format::bvh::BvhSection;
use postretro_level_format::cell_locator::{CellLocatorChild as FormatChild, CellLocatorSection};
use postretro_level_format::cells::CellsSection;
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::portals::PortalsSection;
use postretro_level_format::{SectionId, read_container, read_section_data};
use postretro_level_loader::{CellData, CellLocatorChild, CellLocatorNodeData, LevelWorld, PortalData};

const M: f32 = 0.0254;
const HFOV_DEG: f32 = 100.0;
const ASPECT: f32 = 16.0 / 9.0;
const EYE_M: f32 = 1.5;

// ---- coordinate swizzle (verified against the PRL's world bounds by `bounds`)
// Quake (x, y, z) units -> engine (x, y, z) meters.
fn q2e(q: [f32; 3]) -> Vec3 {
    Vec3::new(-q[1] * M, q[2] * M, -q[0] * M)
}
fn e2q(e: Vec3) -> [f32; 3] {
    [-e.z / M, -e.x / M, e.y / M]
}

pub fn load_world(path: &str) -> LevelWorld {
    let mut file = File::open(path).unwrap();
    let meta = read_container(&mut file).expect("container");
    let mut section = |id: SectionId| {
        read_section_data(&mut file, &meta, id as u32)
            .expect("section read")
            .unwrap_or_else(|| panic!("{path}: no {id:?} section"))
    };
    let cells_raw = section(SectionId::Cells);
    let portals_raw = section(SectionId::Portals);
    let locator_raw = section(SectionId::CellLocator);
    let cells = CellsSection::from_bytes(&cells_raw).expect("cells");
    let portals = PortalsSection::from_bytes(&portals_raw).expect("portals");
    let locator = CellLocatorSection::from_bytes(&locator_raw, cells.cells.len() as u32).expect("locator");
    let child = |c: FormatChild| match c {
        FormatChild::Cell(i) => CellLocatorChild::Cell(i as usize),
        FormatChild::Node(i) => CellLocatorChild::Node(i as usize),
    };
    let cell_data = cells
        .cells
        .iter()
        .map(|c| CellData {
            bounds_min: Vec3::from(c.bounds_min),
            bounds_max: Vec3::from(c.bounds_max),
            face_start: c.face_start,
            face_count: c.face_count,
            portal_ref_start: c.portal_ref_start,
            portal_ref_count: c.portal_ref_count,
            is_solid: c.is_solid(),
            is_exterior: c.is_exterior(),
            is_drawable: c.is_drawable(),
        })
        .collect();
    let nodes = locator
        .nodes
        .iter()
        .map(|n| CellLocatorNodeData {
            plane_normal: Vec3::from(n.plane_normal),
            plane_distance: n.plane_distance,
            front: child(n.front),
            back: child(n.back),
        })
        .collect();
    let portal_data: Vec<PortalData> = portals
        .portals
        .iter()
        .map(|p| PortalData {
            polygon: portals.vertices[p.vertex_start as usize..(p.vertex_start + p.vertex_count) as usize]
                .iter()
                .map(|v| Vec3::from(*v))
                .collect(),
            front_cell: p.front_leaf as usize,
            back_cell: p.back_leaf as usize,
        })
        .collect();
    let has_portals = !portal_data.is_empty();
    LevelWorld::new_visibility_only(cell_data, cells.portal_refs, child(locator.root), nodes, portal_data, has_portals)
        .expect("world")
}

// ---------------------------------------------------------------- stats

struct Shape {
    area: f32,
    normal: Vec3,
    diameter: f32,
    min_width: f32,
    vert_extent: f32,
    horiz_extent: f32,
}

fn shape(poly: &[Vec3]) -> Shape {
    let mut s = Vec3::ZERO;
    for i in 0..poly.len() {
        s += poly[i].cross(poly[(i + 1) % poly.len()]);
    }
    let area = 0.5 * s.length();
    let n = s.normalize_or_zero();
    let mut diameter = 0.0f32;
    for i in 0..poly.len() {
        for j in i + 1..poly.len() {
            diameter = diameter.max((poly[i] - poly[j]).length());
        }
    }
    let mut min_width = f32::MAX;
    for i in 0..poly.len() {
        let e = (poly[(i + 1) % poly.len()] - poly[i]).normalize_or_zero();
        if e == Vec3::ZERO {
            continue;
        }
        let p = n.cross(e);
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for v in poly {
            let d = v.dot(p);
            lo = lo.min(d);
            hi = hi.max(d);
        }
        min_width = min_width.min(hi - lo);
    }
    let (mut ylo, mut yhi) = (f32::MAX, f32::MIN);
    for v in poly {
        ylo = ylo.min(v.y);
        yhi = yhi.max(v.y);
    }
    let t = n.cross(Vec3::Y).normalize_or_zero();
    let (mut hlo, mut hhi) = (f32::MAX, f32::MIN);
    for v in poly {
        let d = v.dot(t);
        hlo = hlo.min(d);
        hhi = hhi.max(d);
    }
    Shape { area, normal: n, diameter, min_width, vert_extent: yhi - ylo, horiz_extent: hhi - hlo }
}

fn hist(vals: &[f32], edges: &[f32], unit: &str) -> String {
    let mut counts = vec![0usize; edges.len() + 1];
    for &v in vals {
        let mut k = 0;
        while k < edges.len() && v >= edges[k] {
            k += 1;
        }
        counts[k] += 1;
    }
    let mut out = String::new();
    for k in 0..counts.len() {
        let lab = if k == 0 {
            format!("<{}", edges[0])
        } else if k == edges.len() {
            format!(">={}", edges[k - 1])
        } else {
            format!("{}-{}", edges[k - 1], edges[k])
        };
        out += &format!("  {lab}{unit}: {}\n", counts[k]);
    }
    out
}

fn pct(v: &mut Vec<f32>, p: f64) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

pub fn stats(path: &str, sliver_n: usize) {
    let w = load_world(path);
    let name = path.rsplit('/').next().unwrap();
    let empty: Vec<usize> = (0..w.cells.len()).filter(|&i| !w.cells[i].is_solid && !w.cells[i].is_exterior).collect();
    let drawable = w.cells.iter().filter(|c| c.is_drawable).count();
    println!("=== {name}: cells={} solid={} exterior={} empty_interior={} drawable={} portals={}",
        w.cells.len(), w.cells.iter().filter(|c| c.is_solid).count(), w.cells.iter().filter(|c| c.is_exterior).count(),
        empty.len(), drawable, w.portals.len());
    let (mut bmin, mut bmax) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for c in w.cells.iter().filter(|c| !c.is_solid && !c.is_exterior) {
        bmin = bmin.min(c.bounds_min);
        bmax = bmax.max(c.bounds_max);
    }
    println!("empty-cell world bounds engine m: {bmin:?} .. {bmax:?}");
    let walkable = |p: &PortalData| {
        let ok = |c: usize| !w.cells[c].is_solid && !w.cells[c].is_exterior;
        ok(p.front_cell) && ok(p.back_cell)
    };
    let all_n = w.portals.len();
    let keep: Vec<usize> = (0..all_n).filter(|&i| walkable(&w.portals[i])).collect();
    println!("portals total={all_n}; walkable (both cells empty interior)={}; the rest touch solid/exterior cells (excluded below)", keep.len());
    let shapes: Vec<Shape> = w.portals.iter().map(|p| shape(&p.polygon)).collect();
    let shapes_all = shapes;
    let kmask: Vec<bool> = (0..all_n).map(|i| keep.binary_search(&i).is_ok()).collect();
    let shapes: Vec<Shape> = keep.iter().map(|&i| shape(&w.portals[i].polygon)).collect();
    let areas: Vec<f32> = shapes.iter().map(|s| s.area).collect();
    println!("portal area m^2 histogram (log decades):\n{}", hist(&areas, &[0.01, 0.1, 1.0, 10.0, 100.0, 1000.0], " m2"));
    let aspects: Vec<f32> = shapes.iter().map(|s| s.diameter / s.min_width.max(1e-6)).collect();
    println!("portal aspect (diameter / min bounding width) histogram:\n{}", hist(&aspects, &[1.5, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0], ""));
    let tall3 = shapes.iter().filter(|s| s.normal.y.abs() < 0.3 && s.vert_extent > 3.0 * s.horiz_extent).count();
    let vert = shapes.iter().filter(|s| s.normal.y.abs() < 0.3).count();
    let tall3_len = shapes.iter().filter(|s| s.normal.y.abs() < 0.3 && s.vert_extent > 3.0 * s.horiz_extent && s.vert_extent > 1.0).count();
    println!("vertical-ish portals (|ny|<0.3): {vert}; taller than 3x horizontal width: {tall3} (and >1 m tall: {tall3_len})");
    let tall_slivers = shapes.iter().filter(|s| s.normal.y.abs() < 0.3 && s.vert_extent >= 6.5 && s.min_width <= 0.7).count();
    let tall_slivers2 = shapes.iter().filter(|s| s.normal.y.abs() < 0.3 && s.vert_extent >= 3.0 && s.min_width <= 1.0).count();
    println!("vertical portals >=6.5 m tall and <=0.7 m wide: {tall_slivers}; >=3 m tall and <=1 m wide: {tall_slivers2}");
    let (mut h, mut v, mut o) = (0, 0, 0);
    for s in &shapes {
        if s.normal.y.abs() > 0.99 { h += 1 } else if s.normal.y.abs() < 0.01 { v += 1 } else { o += 1 }
    }
    println!("portal normals: horizontal={h} vertical={v} oblique={o}");
    let mut mw: Vec<f32> = shapes.iter().map(|s| s.min_width).collect();
    println!("min width m: p5={:.3} p50={:.3} p95={:.3}", pct(&mut mw, 0.05), pct(&mut mw, 0.5), pct(&mut mw, 0.95));

    let vols: Vec<f32> = empty.iter().map(|&i| { let d = w.cells[i].bounds_max - w.cells[i].bounds_min; d.x * d.y * d.z }).collect();
    let mind: Vec<f32> = empty.iter().map(|&i| { let d = w.cells[i].bounds_max - w.cells[i].bounds_min; d.min_element() }).collect();
    println!("empty-cell AABB volume m^3 histogram:\n{}", hist(&vols, &[0.1, 1.0, 10.0, 100.0, 1000.0, 10000.0], " m3"));
    println!("empty-cell AABB smallest dimension m histogram:\n{}", hist(&mind, &[0.1, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0], " m"));
    let thin = mind.iter().filter(|&&d| d < 0.5).count();
    println!("cells with smallest dimension <0.5 m: {thin}");

    if sliver_n > 0 {
        let mut idx: Vec<usize> = (0..shapes.len()).filter(|&i| shapes[i].area > 1e-4).collect();
        idx.sort_by(|&a, &b| aspects[b].partial_cmp(&aspects[a]).unwrap());
        println!("--- {sliver_n} highest-aspect portals (area>1e-4 m2). cols: idx front->back area aspect | engine bounds | quake bounds | quake normal, d");
        for &i in idx.iter().take(sliver_n) {
            let p = &w.portals[keep[i]];
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for v in &p.polygon { lo = lo.min(*v); hi = hi.max(*v); }
            let ql = e2q(lo); let qh = e2q(hi);
            let qa = [ql[0].min(qh[0]), ql[1].min(qh[1]), ql[2].min(qh[2])];
            let qb = [ql[0].max(qh[0]), ql[1].max(qh[1]), ql[2].max(qh[2])];
            let n = shapes[i].normal;
            let d = n.dot(p.polygon[0]);
            let qn = [-n.z, -n.x, n.y]; // swizzle of a direction: e2q without the 1/M
            let qd = d / M; // plane distance in Quake units (|n|=1; swizzle is a rotation+reflection with det... d unchanged in sign if no origin shift)
            println!("PORTAL {} {}->{} area={:.4} aspect={:.1} w={:.3} h={:.2} | E[{:.2},{:.2},{:.2}]..[{:.2},{:.2},{:.2}] | Q[{:.0},{:.0},{:.0}]..[{:.0},{:.0},{:.0}] | n=[{:.3},{:.3},{:.3}] d={:.1}",
                keep[i], p.front_cell, p.back_cell, shapes[i].area, aspects[i], shapes[i].min_width, shapes[i].vert_extent,
                lo.x, lo.y, lo.z, hi.x, hi.y, hi.z, qa[0], qa[1], qa[2], qb[0], qb[1], qb[2], qn[0], qn[1], qn[2], qd);
        }
    }
}

pub fn bounds(path: &str) {
    let w = load_world(path);
    let (mut bmin, mut bmax) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for c in w.cells.iter().filter(|c| !c.is_solid && !c.is_exterior) {
        bmin = bmin.min(c.bounds_min);
        bmax = bmax.max(c.bounds_max);
    }
    println!("empty engine bounds {bmin:?} .. {bmax:?}");
    println!("as quake: {:?} .. {:?}", e2q(bmin), e2q(bmax));
    let spawn = q2e([592.0, 912.0, 56.0]);
    let c = w.locate_cell(spawn);
    println!("spawn cell {c} solid={} bounds {:?}..{:?}", w.cells[c].is_solid, w.cells[c].bounds_min, w.cells[c].bounds_max);
}

// ---------------------------------------------------------------- geometry + rays

struct Tri {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    cell: u32,
    leaf: u32,
}

struct Node {
    lo: Vec3,
    hi: Vec3,
    // internal: left=l, right=r ; leaf: start..start+count into order
    l: u32,
    r: u32,
    start: u32,
    count: u32,
}

struct TriBvh {
    tris: Vec<Tri>,
    order: Vec<u32>,
    nodes: Vec<Node>,
}

impl TriBvh {
    fn build(tris: Vec<Tri>) -> Self {
        let mut order: Vec<u32> = (0..tris.len() as u32).collect();
        let cent: Vec<Vec3> = tris.iter().map(|t| (t.a + t.b + t.c) / 3.0).collect();
        let mut nodes = Vec::new();
        fn rec(tris: &[Tri], cent: &[Vec3], order: &mut [u32], base: usize, nodes: &mut Vec<Node>) -> u32 {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            let (mut clo, mut chi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for &i in order.iter() {
                let t = &tris[i as usize];
                lo = lo.min(t.a).min(t.b).min(t.c);
                hi = hi.max(t.a).max(t.b).max(t.c);
                clo = clo.min(cent[i as usize]);
                chi = chi.max(cent[i as usize]);
            }
            let id = nodes.len() as u32;
            nodes.push(Node { lo, hi, l: 0, r: 0, start: base as u32, count: order.len() as u32 });
            if order.len() <= 4 {
                return id;
            }
            let ext = chi - clo;
            let ax = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
            if ext[ax] <= 0.0 {
                return id;
            }
            order.sort_by(|&a, &b| cent[a as usize][ax].partial_cmp(&cent[b as usize][ax]).unwrap());
            let mid = order.len() / 2;
            let (lo_s, hi_s) = order.split_at_mut(mid);
            let l = rec(tris, cent, lo_s, base, nodes);
            let r = rec(tris, cent, hi_s, base + mid, nodes);
            nodes[id as usize].l = l;
            nodes[id as usize].r = r;
            nodes[id as usize].count = 0;
            id
        }
        rec(&tris, &cent, &mut order, 0, &mut nodes);
        TriBvh { tris, order, nodes }
    }

    fn hit(&self, o: Vec3, d: Vec3, tmin: f32) -> Option<(f32, u32)> {
        let inv = Vec3::new(1.0 / d.x, 1.0 / d.y, 1.0 / d.z);
        let mut best = f32::MAX;
        let mut best_i = u32::MAX;
        let mut stack = [0u32; 64];
        let mut sp = 1;
        stack[0] = 0;
        while sp > 0 {
            sp -= 1;
            let n = &self.nodes[stack[sp] as usize];
            let t1 = (n.lo - o) * inv;
            let t2 = (n.hi - o) * inv;
            let tn = t1.min(t2).max_element();
            let tf = t1.max(t2).min_element();
            if tf < tn.max(tmin) || tn > best {
                continue;
            }
            if n.count > 0 {
                for k in n.start..n.start + n.count {
                    let ti = self.order[k as usize];
                    let t = &self.tris[ti as usize];
                    if let Some(tt) = moller(o, d, t.a, t.b, t.c) {
                        if tt >= tmin && tt < best {
                            best = tt;
                            best_i = ti;
                        }
                    }
                }
            } else if n.l != 0 || n.r != 0 {
                stack[sp] = n.l;
                stack[sp + 1] = n.r;
                sp += 2;
            }
        }
        (best_i != u32::MAX).then_some((best, best_i))
    }
}

fn moller(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    Some(e2.dot(q) * inv)
}

struct Leaf {
    lo: Vec3,
    hi: Vec3,
    tris: u32,
    cell: u32,
}

struct Geo {
    bvh: TriBvh,
    leaves: Vec<Leaf>,
    cell_leaves: Vec<Vec<u32>>,
}

fn load_geo(path: &str, cell_count: usize) -> Geo {
    let mut file = File::open(path).unwrap();
    let meta = read_container(&mut file).expect("container");
    let g = GeometrySection::from_bytes(&read_section_data(&mut file, &meta, SectionId::Geometry as u32).unwrap().unwrap()).unwrap();
    let b = BvhSection::from_bytes(&read_section_data(&mut file, &meta, SectionId::Bvh as u32).unwrap().unwrap()).unwrap();
    let mut tris = Vec::new();
    let mut leaves = Vec::new();
    let mut cell_leaves = vec![Vec::new(); cell_count];
    for (li, l) in b.leaves.iter().enumerate() {
        let idx = &g.indices[l.index_offset as usize..(l.index_offset + l.index_count) as usize];
        for t in idx.chunks_exact(3) {
            let p = |i: u32| Vec3::from(g.vertices[i as usize].position);
            tris.push(Tri { a: p(t[0]), b: p(t[1]), c: p(t[2]), cell: l.cell_id, leaf: li as u32 });
        }
        leaves.push(Leaf { lo: Vec3::from(l.aabb_min), hi: Vec3::from(l.aabb_max), tris: l.index_count / 3, cell: l.cell_id });
        if (l.cell_id as usize) < cell_count {
            cell_leaves[l.cell_id as usize].push(li as u32);
        }
    }
    eprintln!("geometry: {} verts {} tris {} leaves", g.vertices.len(), tris.len(), leaves.len());
    Geo { bvh: TriBvh::build(tris), leaves, cell_leaves }
}

// ---------------------------------------------------------------- poses

fn view_proj(eye: Vec3, look: Vec3) -> Mat4 {
    let up = if look.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let view = glam::camera::rh::view::look_at_mat4(eye, eye + look, up);
    let vfov = 2.0 * ((HFOV_DEG.to_radians() / 2.0).tan() / ASPECT).atan();
    glam::camera::rh::proj::directx::perspective(vfov, ASPECT, 0.1, 4096.0) * view
}

struct PoseSpec {
    label: String,
    qpos: [f32; 2],
    yaw_deg: f32, // Quake: 0 = +x, 90 = +y
    pitch_deg: f32,
}

fn pose_specs() -> Vec<PoseSpec> {
    // (label, x, y, path-direction yaw in Quake degrees)
    let positions: &[(&str, f32, f32, f32)] = &[
        ("small", 40.0, 128.0, 0.0),
        ("small", 128.0, 40.0, 0.0),
        ("small", 128.0, 128.0, 0.0),
        ("small", 128.0, 220.0, 0.0),
        ("small", 220.0, 128.0, 0.0),
        ("small", 220.0, 200.0, 0.0),
        ("corr", 300.0, 128.0, 0.0),
        ("corr", 380.0, 128.0, 0.0),
        ("room2", 450.0, 60.0, 90.0),
        ("room2", 450.0, 300.0, 90.0),
        ("room2", 544.0, 100.0, 90.0),
        ("room2", 544.0, 300.0, 90.0),
        ("room2", 544.0, 500.0, 90.0),
        ("room2", 620.0, 300.0, 90.0),
        ("pass", 544.0, 560.0, 90.0),
        ("pass", 544.0, 640.0, 90.0),
        ("hall-s", 544.0, 700.0, 90.0),
        ("hall-s", 544.0, 900.0, 90.0),
        ("hall-s", 300.0, 900.0, 90.0),
        ("hall-s", 800.0, 900.0, 90.0),
        ("hall-s", 100.0, 1100.0, 90.0),
        ("hall-s", 544.0, 1100.0, 90.0),
    ];
    let target = [400.0f32, 1600.0];
    let mut out = Vec::new();
    for &(l, x, y, path_yaw) in positions {
        let aim = (target[1] - y).atan2(target[0] - x).to_degrees();
        let yaws = [aim - 35.0, aim, aim + 35.0, path_yaw];
        for (k, yaw) in yaws.iter().enumerate() {
            let pitches: &[f32] = if k == 1 || k == 3 { &[0.0, 15.0, -15.0] } else { &[0.0] };
            for &p in pitches {
                out.push(PoseSpec { label: format!("{l}({x:.0},{y:.0})"), qpos: [x, y], yaw_deg: *yaw, pitch_deg: p });
            }
        }
    }
    out
}

fn look_dir_quake(yaw_deg: f32, pitch_deg: f32) -> [f32; 3] {
    let (y, p) = (yaw_deg.to_radians(), pitch_deg.to_radians());
    [y.cos() * p.cos(), y.sin() * p.cos(), p.sin()]
}

fn q2e_dir(q: [f32; 3]) -> Vec3 {
    Vec3::new(-q[1], q[2], -q[0])
}

#[derive(Default, Clone)]
struct Row {
    label: String,
    r: usize,
    e: usize,
    s: usize,
    r_minus_e: usize,
    e_minus_r: usize,
    e_minus_s: usize,
    s_minus_e: usize,
    s_minus_r: usize,
    wr: u64,
    we: u64,
    ws: u64,
    tris_hit: usize,
    oracle_trip: bool,
    rect_trip: bool,
    e_clean: usize,
    pillar_e_minus_s: usize,
    r_minus_e_pillar: usize,
    steps: u64,
}

pub fn decomp(path: &str, out_csv: Option<&str>, grid: (usize, usize)) {
    let world = load_world(path);
    let geo = load_geo(path, world.cells.len());
    let specs = pose_specs();
    // pillar field box, Quake units (x 128..728, y 1312..1912, z 0..144), small margin
    let pq = ([127.0f32, 1312.5, -1.0], [729.0f32, 1911.5, 145.0]);
    let (pa, pb) = (q2e(pq.0), q2e(pq.1));
    let (plo, phi) = (pa.min(pb), pa.max(pb));
    let is_pillar_cell = |c: usize| {
        let cd = &world.cells[c];
        cd.bounds_min.x < phi.x && cd.bounds_max.x > plo.x && cd.bounds_min.y < phi.y && cd.bounds_max.y > plo.y && cd.bounds_min.z < phi.z && cd.bounds_max.z > plo.z
    };
    let input = WalkWorld { world: &world, blocked_portals: &[] };
    let mut scratch = PortalWalkScratch::new();
    let mut rows: Vec<Row> = Vec::new();
    let mut es_freq: HashMap<usize, usize> = HashMap::new();
    let mut re_freq: HashMap<usize, usize> = HashMap::new();
    // E-S instances by Quake-y band: south of wall / wall band / gap behind wall / pillar field / beyond / not in hall
    let mut es_band = [0usize; 6];
    let mut re_band = [0usize; 6];
    let band = |c: usize| -> usize {
        let cd = &world.cells[c];
        let a = e2q(cd.bounds_min); let b = e2q(cd.bounds_max);
        let (ylo, yhi) = (a[1].min(b[1]), a[1].max(b[1]));
        let ymid = 0.5 * (ylo + yhi);
        if ymid < 672.0 { 5 } else if ymid < 1184.0 { 0 } else if ymid < 1216.0 { 1 } else if ymid < 1312.0 { 2 } else if ymid < 1912.0 { 3 } else { 4 }
    };
    let mut es_inst = 0usize; let mut es_tall = 0usize; let mut es_low_geom = 0usize; let mut es_cell_h = 0.0f64; let mut es_geom_h = 0.0f64;
    let (gw, gh) = grid;
    let tan_h = (HFOV_DEG.to_radians() / 2.0).tan();
    let tan_v = tan_h / ASPECT;
    for spec in &specs {
        let eye = q2e([spec.qpos[0], spec.qpos[1], EYE_M / M]);
        let cell = world.locate_cell(eye);
        if cell >= world.cell_count() || world.cells[cell].is_solid || world.cells[cell].is_exterior {
            eprintln!("skip {} at {:?}: located solid/exterior", spec.label, spec.qpos);
            continue;
        }
        let look = q2e_dir(look_dir_quake(spec.yaw_deg, spec.pitch_deg)).normalize();
        let vp = view_proj(eye, look);
        let view = WalkView::camera(eye, vp);
        // R
        let rs = walk_from_camera(input, &view, cell, MAX_PORTAL_WALK_STEPS, &mut scratch);
        let mut rset = vec![false; world.cell_count()];
        for &c in scratch.reached() {
            rset[c as usize] = true;
        }
        // E
        let ex = oracle_from_camera(&world, &[], &view, cell);
        // S
        let fwd = look;
        let up_hint = if fwd.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
        let right = fwd.cross(up_hint).normalize();
        let up = right.cross(fwd);
        let mut sset = vec![false; world.cell_count()];
        let mut hit_tris: std::collections::HashSet<u32> = Default::default();
        for j in 0..gh {
            for i in 0..gw {
                let x = ((i as f32 + 0.5) / gw as f32) * 2.0 - 1.0;
                let y = 1.0 - ((j as f32 + 0.5) / gh as f32) * 2.0;
                let d = (fwd + right * (x * tan_h) + up * (y * tan_v)).normalize();
                // tmin: the near plane is perpendicular to the view axis at 0.1 m
                let tmin = 0.1 / d.dot(fwd);
                if let Some((_, ti)) = geo.bvh.hit(eye, d, tmin) {
                    sset[geo.bvh.tris[ti as usize].cell as usize] = true;
                    hit_tris.insert(ti);
                }
            }
        }
        let frustum: Frustum = extract_frustum_planes(vp);
        let weight = |set: &dyn Fn(usize) -> bool| -> u64 {
            let mut w = 0u64;
            for c in 0..world.cell_count() {
                if !world.cells[c].is_drawable || !set(c) {
                    continue;
                }
                for &l in &geo.cell_leaves[c] {
                    let lf = &geo.leaves[l as usize];
                    if !is_aabb_outside_frustum(lf.lo, lf.hi, &frustum) {
                        w += lf.tris as u64;
                    }
                }
            }
            w
        };
        let dr = |c: usize| world.cells[c].is_drawable;
        let mut row = Row { label: format!("{} yaw{:.0} p{:+.0}", spec.label, spec.yaw_deg, spec.pitch_deg), ..Default::default() };
        for c in 0..world.cell_count() {
            if !dr(c) {
                continue;
            }
            let (r, e, s) = (rset[c], ex.visible[c], sset[c]);
            row.r += r as usize;
            row.e += e as usize;
            row.s += s as usize;
            row.e_clean += ex.clean[c] as usize;
            row.r_minus_e += (r && !e) as usize;
            row.e_minus_r += (e && !r) as usize;
            row.e_minus_s += (e && !s) as usize;
            row.s_minus_e += (s && !e) as usize;
            row.s_minus_r += (s && !r) as usize;
            if e && !s {
                *es_freq.entry(c).or_default() += 1;
                es_band[band(c)] += 1;
                {
                    let cd = &world.cells[c];
                    let ch = (cd.bounds_max.y - cd.bounds_min.y) as f64;
                    let gt = geo.cell_leaves[c].iter().map(|&l| geo.leaves[l as usize].hi.y).fold(cd.bounds_min.y, f32::max);
                    let gh_ = (gt - cd.bounds_min.y) as f64;
                    es_inst += 1; es_cell_h += ch; es_geom_h += gh_;
                    if ch > 6.6 { es_tall += 1; }
                    if gh_ < 0.5 * ch { es_low_geom += 1; }
                }
                if is_pillar_cell(c) {
                    row.pillar_e_minus_s += 1;
                }
            }
            if r && !e {
                *re_freq.entry(c).or_default() += 1;
                re_band[band(c)] += 1;
                if is_pillar_cell(c) {
                    row.r_minus_e_pillar += 1;
                }
            }
        }
        row.wr = weight(&|c| rset[c]);
        row.we = weight(&|c| ex.visible[c]);
        row.ws = weight(&|c| sset[c]);
        row.tris_hit = hit_tris.len();
        row.oracle_trip = !ex.complete();
        row.rect_trip = rs.step_limit_hit;
        row.steps = ex.considered;
        println!(
            "{:<28} R={:4} E={:4} S={:4} | R-E={:3} E-R={:2} E-S={:4} S-E={} S-R={} | wR={:6} wE={:6} wS={:6} trisHit={:5} | pillar E-S={:3} R-E={:3} | oracleSteps={}{}{}",
            row.label, row.r, row.e, row.s, row.r_minus_e, row.e_minus_r, row.e_minus_s, row.s_minus_e, row.s_minus_r,
            row.wr, row.we, row.ws, row.tris_hit, row.pillar_e_minus_s, row.r_minus_e_pillar, row.steps,
            if row.oracle_trip { " ORACLE_TRIP" } else { "" }, if row.rect_trip { " RECT_TRIP" } else { "" }
        );
        rows.push(row);
    }
    // aggregates
    let n = rows.len() as f64;
    let sum = |f: &dyn Fn(&Row) -> f64| rows.iter().map(f).sum::<f64>();
    let med = |f: &dyn Fn(&Row) -> f64| {
        let mut v: Vec<f64> = rows.iter().map(f).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };
    println!("\n=== AGGREGATE over {} poses (drawable cells; mean / median / max)", rows.len());
    let line = |name: &str, f: &dyn Fn(&Row) -> f64| {
        let mx = rows.iter().map(f).fold(0.0, f64::max);
        println!("{name:<22} mean={:8.1} median={:8.1} max={:8.0}", sum(f) / n, med(f), mx);
    };
    line("|R|", &|r| r.r as f64);
    line("|E|", &|r| r.e as f64);
    line("|E clean|", &|r| r.e_clean as f64);
    line("|S|", &|r| r.s as f64);
    line("R-E (looseness)", &|r| r.r_minus_e as f64);
    line("E-R (rect misses)", &|r| r.e_minus_r as f64);
    line("E-S (granularity)", &|r| r.e_minus_s as f64);
    line("R-S (total overdraw)", &|r| (r.r - r.s + r.s_minus_r) as f64);
    line("S-E (oracle misses)", &|r| r.s_minus_e as f64);
    line("S-R (rect misses seen)", &|r| r.s_minus_r as f64);
    line("wR tris", &|r| r.wr as f64);
    line("wE tris", &|r| r.we as f64);
    line("wS tris", &|r| r.ws as f64);
    line("wR-wE", &|r| r.wr as f64 - r.we as f64);
    line("wE-wS", &|r| r.we as f64 - r.ws as f64);
    line("trisHit (distinct)", &|r| r.tris_hit as f64);
    line("pillar cells in E-S", &|r| r.pillar_e_minus_s as f64);
    line("pillar cells in R-E", &|r| r.r_minus_e_pillar as f64);
    println!("oracle trips: {}  rect trips: {}", rows.iter().filter(|r| r.oracle_trip).count(), rows.iter().filter(|r| r.rect_trip).count());
    // restricted subsets
    for (name, pred) in [
        ("small room poses", Box::new(|r: &Row| r.label.starts_with("small")) as Box<dyn Fn(&Row) -> bool>),
        ("corridor+room2+passage poses", Box::new(|r: &Row| r.label.starts_with("corr") || r.label.starts_with("room2") || r.label.starts_with("pass"))),
        ("hall-south poses", Box::new(|r: &Row| r.label.starts_with("hall-s"))),
    ] {
        let sel: Vec<&Row> = rows.iter().filter(|r| pred(r)).collect();
        if sel.is_empty() { continue; }
        let m = |f: &dyn Fn(&Row) -> f64| sel.iter().map(|r| f(r)).sum::<f64>() / sel.len() as f64;
        println!("[{name}: {} poses] mean R={:.1} E={:.1} S={:.1} R-E={:.1} E-S={:.1} | wR={:.0} wE={:.0} wS={:.0} | pillarE-S={:.1}",
            sel.len(), m(&|r| r.r as f64), m(&|r| r.e as f64), m(&|r| r.s as f64), m(&|r| r.r_minus_e as f64), m(&|r| r.e_minus_s as f64),
            m(&|r| r.wr as f64), m(&|r| r.we as f64), m(&|r| r.ws as f64), m(&|r| r.pillar_e_minus_s as f64));
    }
    println!("E-S instances by band [south-of-wall, wall-band, behind-wall gap, pillar-field, beyond-pillars, outside hall]: {:?}", es_band);
    println!("R-E instances by band (same order): {:?}", re_band);
    if es_inst > 0 {
        println!("E-S instances={es_inst}: cell taller than the 6.5 m wall: {es_tall}; geometry top < half cell height: {es_low_geom}; mean cell height {:.1} m vs mean geometry top {:.1} m above cell floor", es_cell_h / es_inst as f64, es_geom_h / es_inst as f64);
    }
    println!("max oracle steps: {}", rows.iter().map(|r| r.steps).max().unwrap_or(0));
    // most frequent E-S cells
    let mut es: Vec<(usize, usize)> = es_freq.into_iter().collect();
    es.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    println!("\nmost frequent E-S cells (cell: poses, pillar?, tris, quake bounds):");
    for &(c, k) in es.iter().take(25) {
        let cd = &world.cells[c];
        let (a, b) = (e2q(cd.bounds_min), e2q(cd.bounds_max));
        let tris: u32 = geo.cell_leaves[c].iter().map(|&l| geo.leaves[l as usize].tris).sum();
        println!("  cell {c}: {k} poses pillar={} tris={tris} Q[{:.0},{:.0},{:.0}]..[{:.0},{:.0},{:.0}]", is_pillar_cell(c),
            a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2]), a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2]));
    }
    let mut re: Vec<(usize, usize)> = re_freq.into_iter().collect();
    re.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    println!("\nmost frequent R-E cells:");
    for &(c, k) in re.iter().take(10) {
        let cd = &world.cells[c];
        let (a, b) = (e2q(cd.bounds_min), e2q(cd.bounds_max));
        println!("  cell {c}: {k} poses pillar={} Q[{:.0},{:.0},{:.0}]..[{:.0},{:.0},{:.0}]", is_pillar_cell(c),
            a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2]), a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2]));
    }
    if let Some(p) = out_csv {
        let mut s = String::from("label,R,E,S,RmE,EmR,EmS,SmE,SmR,wR,wE,wS,trisHit,pillarEmS,pillarRmE,oracleSteps,oracleTrip,rectTrip\n");
        for r in &rows {
            s += &format!("{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n", r.label.replace(',', ";"), r.r, r.e, r.s, r.r_minus_e, r.e_minus_r, r.e_minus_s, r.s_minus_e, r.s_minus_r, r.wr, r.we, r.ws, r.tris_hit, r.pillar_e_minus_s, r.r_minus_e_pillar, r.steps, r.oracle_trip, r.rect_trip);
        }
        std::fs::write(p, s).unwrap();
    }
}

pub fn run() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(|s| s.as_str()) {
        Some("stats") => {
            let n: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            for p in &a[3..] {
                stats(p, n);
            }
        }
        Some("bounds") => bounds(&a[2]),
        Some("decomp") => decomp(&a[2], a.get(3).map(|s| s.as_str()), (320, 180)),
        _ => eprintln!("usage: gran stats <sliver_n> <prl>... | bounds <prl> | decomp <prl> [csv]"),
    }
}
