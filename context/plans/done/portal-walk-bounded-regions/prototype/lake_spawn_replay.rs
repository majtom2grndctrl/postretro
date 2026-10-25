// TEMPORARY single-pose replay: reflection-lake-test spawn, rect walk vs oracle vs old chain walk.

use glam::{Mat4, Vec3};

use super::geometry::distance_to_polygon;
use super::oracle::oracle_from_camera;
use super::real_map_sweep::load_world;
use super::test_fixtures::view_proj_with;
use super::tests::unclassified_misses;
use super::{
    MAX_PORTAL_WALK_STEPS, PORTAL_BYPASS_DISTANCE, PortalWalkScratch, WalkTrace, WalkView,
    WalkWorld, crosses_from_source_side, walk_from_camera, walk_from_camera_traced,
};
use crate::portal_vis::{clip_polygon_to_frustum, narrow_frustum};
use crate::visibility::Frustum;
use postretro_level_loader::LevelWorld;

const PRL: &str = "/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/4cd91634-5d3f-4db5-a168-136f9867d0b1/scratchpad/rect3/maps/reflection-lake-test.prl";

fn view_proj(eye: Vec3) -> Mat4 {
    view_proj_with(eye, 90f32.to_radians(), 0.0, 100.0, 1280.0 / 720.0)
}

fn ndc_of(p: Vec3, vp: &Mat4) -> (f32, f32, f32) {
    let c = *vp * p.extend(1.0);
    (c.x / c.w, c.y / c.w, c.w)
}

/// Old pre-change bypass: eye within 1e-3 of the polygon's infinite plane.
fn old_on_plane(apex: Vec3, polygon: &[Vec3]) -> bool {
    let n = polygon.len();
    if n < 3 {
        return false;
    }
    let centroid = polygon.iter().copied().sum::<Vec3>() / n as f32;
    let mut normal = Vec3::ZERO;
    for i in 0..n {
        let cur = polygon[i];
        let nxt = polygon[(i + 1) % n];
        normal.x += (cur.y - nxt.y) * (cur.z + nxt.z);
        normal.y += (cur.z - nxt.z) * (cur.x + nxt.x);
        normal.z += (cur.x - nxt.x) * (cur.y + nxt.y);
    }
    if normal.length_squared() < 1e-12 {
        return false;
    }
    normal.normalize().dot(apex - centroid).abs() < 1e-3
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Oracle,
    Old,
}

#[derive(Clone, Debug)]
struct Hop {
    portal: usize,
    from: usize,
    to: usize,
    source_side: f32,
    bypass: bool,
    backward: bool,
    on_screen: bool,
    clipped: usize,
}

struct Flood<'a> {
    world: &'a LevelWorld,
    view: &'a WalkView,
    mode: Mode,
    visible: Vec<bool>,
    clean: Vec<bool>,
    targets: Vec<usize>,
    chains: Vec<(usize, Vec<Hop>)>,
    path: Vec<Hop>,
    steps: u64,
}

fn flood(s: &mut Flood, cell: usize, frustum: &Frustum, forward: bool, on_screen: bool) {
    s.visible[cell] = true;
    s.clean[cell] |= forward && on_screen;
    if s.targets.contains(&cell) && s.chains.iter().filter(|(c, _)| *c == cell).count() < 4 {
        s.chains.push((cell, s.path.clone()));
    }
    if s.path.len() >= 256 || s.steps > 20_000_000 {
        return;
    }
    let world = s.world;
    let view = s.view;
    let planes = world.portal_planes();
    let eye = view.eye;
    for i in 0..world.cell_portal_count(cell) {
        s.steps += 1;
        let Some(pi) = world.cell_portal_index(cell, i) else {
            continue;
        };
        let portal = &world.portals[pi];
        let plane = planes[pi];
        let neighbor = if portal.front_cell == cell {
            portal.back_cell
        } else {
            portal.front_cell
        };
        if neighbor >= world.cell_count()
            || s.path.iter().any(|h| h.portal == pi)
            || world.cell_is_solid(neighbor)
        {
            continue;
        }
        let d = plane.signed_distance(eye);
        let source_side = if portal.front_cell == cell { d } else { -d };
        let (bypass, backward) = match s.mode {
            Mode::Oracle => {
                let b = distance_to_polygon(eye, &portal.polygon, &plane) <= PORTAL_BYPASS_DISTANCE;
                (b, !b && plane.oriented && !crosses_from_source_side(source_side))
            }
            Mode::Old => (old_on_plane(eye, &portal.polygon), false),
        };
        let (child, hop_on_screen, clipped_len) = if bypass {
            let Some(mut narrowed) = narrow_frustum(eye, &portal.polygon, frustum) else {
                continue;
            };
            if s.mode == Mode::Oracle {
                narrowed.planes.extend_from_slice(&frustum.planes);
            }
            (narrowed, !wholly_outside(&portal.polygon, view.frustum()), portal.polygon.len())
        } else {
            let mut a = Vec::new();
            let mut b = Vec::new();
            let clipped = clip_polygon_to_frustum(&portal.polygon, frustum, &mut a, &mut b);
            if clipped.len() < 3 {
                continue;
            }
            let os = !wholly_outside(clipped, view.frustum());
            let len = clipped.len();
            let Some(narrowed) = narrow_frustum(eye, clipped, frustum) else {
                continue;
            };
            (narrowed, os, len)
        };
        s.path.push(Hop {
            portal: pi,
            from: cell,
            to: neighbor,
            source_side,
            bypass,
            backward,
            on_screen: hop_on_screen,
            clipped: clipped_len,
        });
        flood(s, neighbor, &child, forward && !backward, hop_on_screen);
        s.path.pop();
    }
}

fn wholly_outside(polygon: &[Vec3], frustum: &Frustum) -> bool {
    frustum
        .planes
        .iter()
        .any(|p| polygon.iter().all(|v| p.normal.dot(*v) + p.dist < 0.0))
}

fn hits_along(world: &LevelWorld, origin: Vec3, dir: Vec3) -> Vec<(f32, usize, usize, f32, Vec3)> {
    // (t, leaf, cell, min barycentric, hit point)
    let mut out = Vec::new();
    for (li, leaf) in world.bvh.leaves.iter().enumerate() {
        let start = leaf.index_offset as usize;
        let end = start + leaf.index_count as usize;
        for tri in world.indices[start..end].chunks(3) {
            let p = |k: usize| Vec3::from(world.vertices[tri[k] as usize].position);
            let (a, b, c) = (p(0), p(1), p(2));
            let e1 = b - a;
            let e2 = c - a;
            let pv = dir.cross(e2);
            let det = e1.dot(pv);
            if det.abs() < 1e-12 {
                continue;
            }
            let inv = 1.0 / det;
            let tv = origin - a;
            let u = tv.dot(pv) * inv;
            let qv = tv.cross(e1);
            let v = dir.dot(qv) * inv;
            let t = e2.dot(qv) * inv;
            let w = 1.0 - u - v;
            let m = u.min(v).min(w);
            if t > 0.0 && m > -0.02 {
                out.push((t, li, leaf.cell_id as usize, m, origin + dir * t));
            }
        }
    }
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    out
}

#[test]
#[ignore = "temporary lake spawn replay"]
fn tmp_lake_spawn_replay() {
    let (world, full) = load_world(PRL);
    eprintln!("full_load={full} cells={} portals={}", world.cell_count(), world.portals.len());
    let input = WalkWorld {
        world: &world,
        blocked_portals: &[],
    };
    let mut scratch = PortalWalkScratch::new();
    let mut zs: Vec<f32> = vec![-48.77, -48.771, -48.78, -48.76, -48.67, -48.87];
    let mut z = -48.800f32;
    while z <= -48.740 {
        zs.push(z);
        z += 0.001;
    }
    for (k, &z) in zs.iter().enumerate() {
        let detail = k < 6;
        let eye = Vec3::new(-16.87, 1.6, z);
        let vp = view_proj(eye);
        let view = WalkView::camera(eye, vp);
        let cell = world.locate_cell(eye);
        let stats = walk_from_camera(input, &view, cell, MAX_PORTAL_WALK_STEPS, &mut scratch);
        let exact = oracle_from_camera(&world, &[], &view, cell);
        let mut old = Flood {
            world: &world,
            view: &view,
            mode: Mode::Old,
            visible: vec![false; world.cell_count()],
            clean: vec![false; world.cell_count()],
            targets: vec![],
            chains: vec![],
            path: vec![],
            steps: 0,
        };
        let fr = view.frustum().clone();
        flood(&mut old, cell, &fr, true, true);
        let oracle_only: Vec<usize> = (0..world.cell_count())
            .filter(|&c| exact.visible[c] && !scratch.is_reached(c))
            .collect();
        let old_only: Vec<usize> = (0..world.cell_count())
            .filter(|&c| old.visible[c] && !scratch.is_reached(c))
            .collect();
        let unclassified = unclassified_misses(&exact, &scratch);
        eprintln!(
            "z={z:.4} cell={cell} walk_tests={} reached={} oracle_visible={} oracle_only={oracle_only:?} (clean? {:?}) unclassified={unclassified:?} old_only={old_only:?}",
            stats.considered,
            scratch.reached().len(),
            exact.visible.iter().filter(|&&v| v).count(),
            oracle_only.iter().map(|&c| exact.clean[c]).collect::<Vec<_>>(),
        );
        if !detail {
            continue;
        }
        let mut targets = oracle_only.clone();
        targets.extend(old_only.iter().copied());
        for mode in [Mode::Oracle, Mode::Old] {
            let mut s = Flood {
                world: &world,
                view: &view,
                mode,
                visible: vec![false; world.cell_count()],
                clean: vec![false; world.cell_count()],
                targets: targets.clone(),
                chains: vec![],
                path: vec![],
                steps: 0,
            };
            flood(&mut s, cell, &fr, true, true);
            for (c, chain) in &s.chains {
                eprintln!("  {} chain to {c}:", if mode == Mode::Oracle { "oracle" } else { "old" });
                for h in chain {
                    let p = &world.portals[h.portal];
                    let pl = world.portal_planes()[h.portal];
                    eprintln!(
                        "    portal {} {}->{} src_side={:+.6} bypass={} backward={} on_screen={} clipped={} dist_poly={:.5} plane n=({:.3},{:.3},{:.3}) d={:.4} oriented={} poly={:?}",
                        h.portal, h.from, h.to, h.source_side, h.bypass, h.backward, h.on_screen, h.clipped,
                        distance_to_polygon(eye, &p.polygon, &pl),
                        pl.normal.x, pl.normal.y, pl.normal.z, pl.dist, pl.oriented,
                        p.polygon.iter().map(|v| (v.x, v.y, v.z)).collect::<Vec<_>>()
                    );
                }
            }
        }
        // Ray through pixel centre (640.5, 368.5).
        let inv = vp.inverse();
        let ndc_x = 640.5 / 640.0 - 1.0;
        let ndc_y = 1.0 - 368.5 / 360.0;
        let near = inv.project_point3(Vec3::new(ndc_x, ndc_y, 0.0));
        let far = inv.project_point3(Vec3::new(ndc_x, ndc_y, 1.0));
        let dir = (far - near).normalize();
        eprintln!("  ray from {near:?} dir {dir:?}");
        for (t, li, c, m, hit) in hits_along(&world, eye, dir).iter().take(12) {
            eprintln!(
                "   hit t={t:.4} leaf={li} cell={c} min_bary={m:+.5} at ({:.4},{:.4},{:.4}) walk={} oracle={} old={} ndc={:?}",
                hit.x, hit.y, hit.z,
                scratch.is_reached(*c), exact.visible[*c], old.visible[*c], ndc_of(*hit, &vp)
            );
        }
        if k == 0 {
            let mut trace = WalkTrace::begin(eye, cell, &world);
            let st = walk_from_camera_traced(input, &view, cell, MAX_PORTAL_WALK_STEPS, &mut scratch, Some(&mut trace));
            eprintln!("{}", trace.finish(&st, scratch.reached().len()));
        }
    }
}

#[test]
#[ignore = "temporary lake spawn geometry dump"]
fn tmp_lake_spawn_dump() {
    let (world, _) = load_world(PRL);
    let planes = world.portal_planes();
    for c in [2usize, 3, 4, 147, 161, 165, 166, 167, 52] {
        let cd = &world.cells[c];
        eprintln!("cell {c}: bounds {:?}..{:?} solid={} ext={} drawable={} faces={}+{}", cd.bounds_min, cd.bounds_max, cd.is_solid, cd.is_exterior, cd.is_drawable, cd.face_start, cd.face_count);
        for i in 0..world.cell_portal_count(c) {
            let pi = world.cell_portal_index(c, i).unwrap();
            let p = &world.portals[pi];
            let pl = planes[pi];
            eprintln!("   portal {pi} {}<->{} n=({:.3},{:.3},{:.3}) d={:.4} poly={:?}", p.front_cell, p.back_cell, pl.normal.x, pl.normal.y, pl.normal.z, pl.dist, p.polygon.iter().map(|v| (v.x, v.y, v.z)).collect::<Vec<_>>());
        }
        for (li, leaf) in world.bvh.leaves.iter().enumerate() {
            if leaf.cell_id as usize != c { continue; }
            let start = leaf.index_offset as usize;
            let end = start + leaf.index_count as usize;
            let mut verts: Vec<(f32,f32,f32)> = world.indices[start..end].iter().map(|&i| { let v = world.vertices[i as usize].position; (v[0], v[1], v[2]) }).collect();
            verts.sort_by(|a, b| a.partial_cmp(b).unwrap());
            verts.dedup();
            eprintln!("   leaf {li} aabb {:?}..{:?} verts {:?}", leaf.aabb_min, leaf.aabb_max, verts);
        }
    }
    // Rays at neighbouring columns, z = -48.77.
    let eye = Vec3::new(-16.87, 1.6, -48.77);
    let vp = view_proj(eye);
    let inv = vp.inverse();
    for px in [638.5f32, 639.5, 640.0, 640.5, 641.5, 642.5] {
        let ndc_x = px / 640.0 - 1.0;
        let ndc_y = 1.0 - 368.5 / 360.0;
        let near = inv.project_point3(Vec3::new(ndc_x, ndc_y, 0.0));
        let far = inv.project_point3(Vec3::new(ndc_x, ndc_y, 1.0));
        let dir = (far - near).normalize();
        let h = hits_along(&world, eye, dir);
        eprintln!("px {px}: {:?}", h.iter().take(4).map(|(t, li, c, m, p)| (*t, *li, *c, *m, (p.x, p.y, p.z))).collect::<Vec<_>>());
    }
}

#[test]
#[ignore = "temporary lake spawn portal gap check"]
fn tmp_lake_spawn_gap() {
    let pre = "/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/4cd91634-5d3f-4db5-a168-136f9867d0b1/scratchpad/prechange/maps/reflection-lake-test.prl";
    for path in [PRL, pre] {
        let (world, _) = load_world(path);
        eprintln!("{path}: cells={} portals={}", world.cell_count(), world.portals.len());
        for p in [Vec3::new(-76.39, 0.5, -48.82), Vec3::new(-76.42, 0.5, -48.82), Vec3::new(-76.39, 0.9, -48.82), Vec3::new(-76.42, 0.5, -52.07), Vec3::new(-76.42, 0.5, -45.57)] {
            eprintln!("  locate {p:?} -> {}", world.locate_cell(p));
        }
        // Portals on x = -76.4032 overlapping each 10 cm z strip.
        for (lo, hi) in [(-48.8696f32, -48.768f32), (-52.1208, -52.0192), (-45.6184, -45.5168)] {
            let found: Vec<usize> = (0..world.portals.len()).filter(|&i| {
                let poly = &world.portals[i].polygon;
                poly.iter().all(|v| (v.x + 76.4032).abs() < 1e-3)
                    && poly.iter().map(|v| v.z).fold(f32::MAX, f32::min) < hi - 1e-3
                    && poly.iter().map(|v| v.z).fold(f32::MIN, f32::max) > lo + 1e-3
            }).collect();
            eprintln!("  portals on x=-76.4032 overlapping z ({lo},{hi}): {found:?}");
        }
        for i in [25usize, 64] { let p = &world.portals[i]; eprintln!("  portal {i} {}<->{} {:?}", p.front_cell, p.back_cell, p.polygon); }
        let mut v: Vec<String> = world.portals.iter().map(|p| format!("{}-{}:{:?}", p.front_cell, p.back_cell, p.polygon)).collect();
        v.sort();
        let h = v.iter().fold(0u64, |h, s| s.bytes().fold(h, |h, b| h.wrapping_mul(1099511628211).wrapping_add(b as u64)));
        eprintln!("  portal-set hash {h:x}");
    }
    // Band edges at 0.1 mm.
    let (world, _) = load_world(PRL);
    let input = WalkWorld { world: &world, blocked_portals: &[] };
    let mut scratch = PortalWalkScratch::new();
    for z in [-48.7660f32, -48.7670, -48.7675, -48.7678, -48.7679, -48.7680, -48.7681, -48.7682, -48.7685, -48.8690, -48.8694, -48.8696, -48.8697, -48.8698, -48.8700, -48.8710] {
        let eye = Vec3::new(-16.87, 1.6, z);
        let view = WalkView::camera(eye, view_proj(eye));
        let cell = world.locate_cell(eye);
        walk_from_camera(input, &view, cell, MAX_PORTAL_WALK_STEPS, &mut scratch);
        let exact = oracle_from_camera(&world, &[], &view, cell);
        let pl = world.portal_planes();
        eprintln!("z={z:.4} cell={cell} walk 166={} 167={} | oracle 166={}/{} 167={}/{} | sd(425)={:+.6} sd(409)={:+.6} unclassified={:?}",
            scratch.is_reached(166), scratch.is_reached(167), exact.visible[166], exact.clean[166], exact.visible[167], exact.clean[167],
            pl[425].signed_distance(eye), pl[409].signed_distance(eye), unclassified_misses(&exact, &scratch));
    }
}
