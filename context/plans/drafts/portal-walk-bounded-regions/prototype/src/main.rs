// Offline portal-walk sweep harness. Throwaway; not part of the workspace.
#![allow(dead_code)]
mod portal_vis;
mod alt_vis;
mod alt_main;

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use glam::{Mat4, Vec3};
use postretro_level_format::{
    SectionId, cell_locator::CellLocatorSection, cells::CellsSection, portals::PortalsSection,
    read_container, read_section_data,
};
use postretro_level_loader::{CellData, CellLocatorChild, CellLocatorNodeData, LevelWorld, PortalData};

const HFOV: f32 = 100.0 * std::f32::consts::PI / 180.0;
const NEAR: f32 = 0.1;
const FAR: f32 = 4096.0;
const ASPECT: f32 = 16.0 / 9.0;
const EYE: f32 = 1.6;

fn load(path: &str) -> LevelWorld {
    let mut f = File::open(path).expect("open");
    let meta = read_container(&mut f).expect("container");
    let cells_raw = read_section_data(&mut f, &meta, SectionId::Cells as u32).unwrap().expect("cells");
    let portals_raw = read_section_data(&mut f, &meta, SectionId::Portals as u32).unwrap().expect("portals");
    let loc_raw = read_section_data(&mut f, &meta, SectionId::CellLocator as u32).unwrap().expect("locator");
    let cs = CellsSection::from_bytes(&cells_raw).expect("cells parse");
    let cell_count = cs.cells.len() as u32;
    let ps = PortalsSection::from_bytes(&portals_raw).expect("portals parse");
    let ls = CellLocatorSection::from_bytes(&loc_raw, cell_count).expect("loc parse");

    let cells: Vec<CellData> = cs
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
    let conv = |c: postretro_level_format::cell_locator::CellLocatorChild| match c {
        postretro_level_format::cell_locator::CellLocatorChild::Cell(i) => CellLocatorChild::Cell(i as usize),
        postretro_level_format::cell_locator::CellLocatorChild::Node(i) => CellLocatorChild::Node(i as usize),
    };
    let nodes: Vec<CellLocatorNodeData> = ls
        .nodes
        .iter()
        .map(|n| CellLocatorNodeData {
            plane_normal: Vec3::from(n.plane_normal),
            plane_distance: n.plane_distance,
            front: conv(n.front),
            back: conv(n.back),
        })
        .collect();
    let portals: Vec<PortalData> = ps
        .portals
        .iter()
        .map(|p| PortalData {
            polygon: ps.vertices[p.vertex_start as usize..(p.vertex_start + p.vertex_count) as usize]
                .iter()
                .map(|v| Vec3::from(*v))
                .collect(),
            front_cell: p.front_leaf as usize,
            back_cell: p.back_leaf as usize,
        })
        .collect();
    let has = !portals.is_empty();
    LevelWorld::new_visibility_only(cells, cs.portal_refs, conv(ls.root), nodes, portals, has).expect("world")
}

fn view_proj(pos: Vec3, yaw: f32, pitch: f32) -> Mat4 {
    let look = Vec3::new(-yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos());
    let view = Mat4::look_at_rh(pos, pos + look, Vec3::Y);
    let vfov = 2.0 * ((HFOV / 2.0).tan() / ASPECT).atan();
    Mat4::perspective_rh(vfov, ASPECT, NEAR, FAR) * view
}

fn poly_area_normal(poly: &[Vec3]) -> (f32, Vec3) {
    let mut s = Vec3::ZERO;
    for i in 0..poly.len() {
        s += poly[i].cross(poly[(i + 1) % poly.len()]);
    }
    (0.5 * s.length(), s.normalize_or_zero())
}

#[derive(Clone, Copy)]
struct Sample {
    src_cell: usize,
    kind: u8, // 0 center, 1 eye
    pos: Vec3,
    cam_cell: usize,
}

#[derive(Clone, Copy, Default)]
struct Rec {
    considered: u32,
    accepted: u32,
    visible: u32,
    reach: u32,
    trip: bool,
    us: u32,
    cycle: u32,
    clip: u32,
    narrow: u32,
    aabb_fallback: u32,
}

fn pct(v: &mut Vec<u32>, p: f64) -> u32 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    let i = ((v.len() - 1) as f64 * p).round() as usize;
    v[i]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s == "geo").unwrap_or(false) {
        for p in &args[2..] {
            let w = load(p);
            let c5 = (5.0f32).to_radians().cos();
            let axis = |v: Vec3| v.abs().max_element() >= c5;
            let (mut np, mut nonax, mut ne, mut nonax_e, mut int_p, mut int_nonax) = (0, 0, 0, 0, 0, 0);
            for pt in &w.portals {
                let (_, n) = poly_area_normal(&pt.polygon);
                np += 1;
                let na = !axis(n);
                if na { nonax += 1; }
                let interior = !w.cells[pt.front_cell].is_exterior && !w.cells[pt.back_cell].is_exterior;
                if interior { int_p += 1; if na { int_nonax += 1; } }
                for i in 0..pt.polygon.len() {
                    let e = pt.polygon[(i + 1) % pt.polygon.len()] - pt.polygon[i];
                    if e.length() < 1e-4 { continue; }
                    ne += 1;
                    if !axis(e.normalize()) { nonax_e += 1; }
                }
            }
            let empty = w.cells.iter().filter(|c| !c.is_solid && !c.is_exterior).count();
            println!("{}	cells={} empty_int={} portals={} nonaxis_normal={:.1}% interior_portals={} interior_nonaxis={:.1}% edges={} nonaxis_edges={:.1}%",
                p.rsplit('/').next().unwrap(), w.cells.len(), empty, np, 100.0 * nonax as f64 / np.max(1) as f64, int_p, 100.0 * int_nonax as f64 / int_p.max(1) as f64, ne, 100.0 * nonax_e as f64 / ne.max(1) as f64);
        }
        return;
    }
    if args.get(1).map(|s| s == "dbg").unwrap_or(false) {
        alt_main::debug(&args[2..]);
        return;
    }
    if args.get(1).map(|s| s == "alt").unwrap_or(false) {
        alt_main::run(&args[2..]);
        return;
    }
    let path = args.get(1).expect("prl path");
    let out_dir = args.get(2).cloned().unwrap_or_else(|| ".".into());
    let t0 = Instant::now();
    let world = load(path);
    eprintln!("loaded in {:?}", t0.elapsed());

    let cells = &world.cells;
    let n_solid = cells.iter().filter(|c| c.is_solid).count();
    let n_ext = cells.iter().filter(|c| c.is_exterior).count();
    let n_draw = cells.iter().filter(|c| c.is_drawable).count();
    println!(
        "cells={} solid={} exterior={} drawable={} portals={} portal_refs={} locator_nodes={}",
        cells.len(), n_solid, n_ext, n_draw, world.portals.len(), world.cell_portal_refs.len(), world.cell_locator_nodes.len()
    );

    // ---- portal geometry stats
    let mut areas: Vec<f32> = Vec::new();
    let (mut horiz, mut vert, mut oblique) = (0, 0, 0);
    let mut tiny = 0;
    for p in &world.portals {
        let (a, n) = poly_area_normal(&p.polygon);
        areas.push(a);
        if n.y.abs() > 0.99 {
            horiz += 1
        } else if n.y.abs() < 0.01 {
            vert += 1
        } else {
            oblique += 1
        }
        if a < 0.01 {
            tiny += 1;
        }
    }
    areas.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |p: f64| areas[((areas.len() - 1) as f64 * p) as usize];
    println!(
        "portal area m^2: p5={:.4} p50={:.3} p95={:.2} max={:.1}; <0.01m^2={} ; horizontal={} vertical={} oblique={}",
        q(0.05), q(0.5), q(0.95), q(1.0), tiny, horiz, vert, oblique
    );
    let mut heights: Vec<f32> = cells.iter().filter(|c| !c.is_solid && !c.is_exterior).map(|c| c.bounds_max.y - c.bounds_min.y).collect();
    heights.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let hq = |p: f64| heights[((heights.len() - 1) as f64 * p) as usize];
    println!(
        "empty interior cell heights: n={} p5={:.3} p50={:.2} p95={:.2}; thin(<0.5m)={}",
        heights.len(), hq(0.05), hq(0.5), hq(0.95), heights.iter().filter(|&&h| h < 0.5).count()
    );
    let mut deg: Vec<u32> = cells.iter().filter(|c| !c.is_solid && !c.is_exterior).map(|c| c.portal_ref_count).collect();
    let dmax = *deg.iter().max().unwrap_or(&0);
    println!("interior cell portal degree p50={} p95={} max={}", pct(&mut deg.clone(), 0.5), pct(&mut deg, 0.95), dmax);
    let mut wmin = Vec3::splat(f32::MAX);
    let mut wmax = Vec3::splat(f32::MIN);
    for c in cells.iter().filter(|c| !c.is_solid && !c.is_exterior) {
        wmin = wmin.min(c.bounds_min);
        wmax = wmax.max(c.bounds_max);
    }
    println!("interior extent {:?} .. {:?}", wmin, wmax);

    // ---- samples
    let mut samples = Vec::new();
    let mut skipped = 0;
    for (i, c) in cells.iter().enumerate() {
        if c.is_solid || c.is_exterior {
            continue;
        }
        let ctr = (c.bounds_min + c.bounds_max) * 0.5;
        let h = c.bounds_max.y - c.bounds_min.y;
        let mut cands = vec![(0u8, ctr)];
        if h > EYE + 0.1 {
            cands.push((1u8, Vec3::new(ctr.x, c.bounds_min.y + EYE, ctr.z)));
        }
        for (k, p) in cands {
            let cc = world.locate_cell(p);
            if cc >= cells.len() || cells[cc].is_solid || cells[cc].is_exterior {
                skipped += 1;
                continue;
            }
            samples.push(Sample { src_cell: i, kind: k, pos: p, cam_cell: cc });
        }
    }
    let mut views = Vec::new();
    for pitch in [0.0f32, 45.0, -45.0] {
        for k in 0..8 {
            views.push(((k as f32 * 45.0).to_radians(), pitch.to_radians()));
        }
    }
    let nwalk = samples.len() * views.len();
    println!("samples={} (skipped {} located solid/exterior) views={} walks={}", samples.len(), skipped, views.len(), nwalk);

    let recs: Vec<std::sync::Mutex<Rec>> = (0..nwalk).map(|_| std::sync::Mutex::new(Rec::default())).collect();
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let t1 = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let w = next.fetch_add(1, Ordering::Relaxed);
                if w >= nwalk {
                    break;
                }
                let smp = samples[w / views.len()];
                let (yaw, pitch) = views[w % views.len()];
                let vp = view_proj(smp.pos, yaw, pitch);
                let fr = postretro_visibility::extract_frustum_planes(vp);
                let ts = Instant::now();
                let cap: u32 = std::env::var("STEP_CAP").ok().and_then(|v| v.parse().ok()).unwrap_or(20_000);
                let r = portal_vis::portal_traverse_with_step_limit(smp.pos, smp.cam_cell, &fr, &world, &[], false, cap);
                let us = ts.elapsed().as_micros() as u32;
                let visible = r.visible.iter().enumerate().filter(|(i, v)| **v && world.cells[*i].is_drawable).count() as u32;
                let reach = r.visible.iter().filter(|v| **v).count() as u32;
                let aabb = if r.stats.step_limit_hit {
                    world.cells.iter().filter(|c| c.is_drawable && !aabb_out(c.bounds_min, c.bounds_max, &fr)).count() as u32
                } else {
                    0
                };
                *recs[w].lock().unwrap() = Rec {
                    considered: r.stats.considered,
                    accepted: r.stats.accepted,
                    visible,
                    reach,
                    trip: r.stats.step_limit_hit,
                    us,
                    cycle: r.stats.rejected_path_cycle,
                    clip: r.stats.rejected_clipped,
                    narrow: r.stats.rejected_narrow,
                    aabb_fallback: aabb,
                };
            });
        }
    });
    eprintln!("sweep in {:?} on {} threads", t1.elapsed(), threads);
    let recs: Vec<Rec> = recs.into_iter().map(|m| m.into_inner().unwrap()).collect();

    // ---- CSV
    let mut csv = BufWriter::new(File::create(format!("{}/sweep.csv", out_dir)).unwrap());
    writeln!(csv, "src_cell,kind,cam_cell,x,y,z,yaw_deg,pitch_deg,considered,accepted,visible_drawable,reach,trip,us,cycle_rej,clip_rej,narrow_rej,aabb_fallback_drawable").unwrap();
    for (w, r) in recs.iter().enumerate() {
        let smp = samples[w / views.len()];
        let (yaw, pitch) = views[w % views.len()];
        writeln!(
            csv,
            "{},{},{},{:.2},{:.2},{:.2},{:.0},{:.0},{},{},{},{},{},{},{},{},{},{}",
            smp.src_cell, smp.kind, smp.cam_cell, smp.pos.x, smp.pos.y, smp.pos.z, yaw.to_degrees(), pitch.to_degrees(),
            r.considered, r.accepted, r.visible, r.reach, r.trip as u8, r.us, r.cycle, r.clip, r.narrow, r.aabb_fallback
        )
        .unwrap();
    }
    drop(csv);

    // ---- summary
    let mut steps: Vec<u32> = recs.iter().map(|r| r.considered).collect();
    let mut us: Vec<u32> = recs.iter().map(|r| r.us).collect();
    let mut vis: Vec<u32> = recs.iter().filter(|r| !r.trip).map(|r| r.visible).collect();
    let trips = recs.iter().filter(|r| r.trip).count();
    println!(
        "steps(considered): p50={} p90={} p95={} p99={} p99.9={} max={}",
        pct(&mut steps, 0.5), pct(&mut steps, 0.9), pct(&mut steps, 0.95), pct(&mut steps, 0.99), pct(&mut steps, 0.999), pct(&mut steps, 1.0)
    );
    println!(
        "walk time us: p50={} p95={} p99={} max={}",
        pct(&mut us, 0.5), pct(&mut us, 0.95), pct(&mut us, 0.99), pct(&mut us, 1.0)
    );
    println!("visible drawable (non-trip): p50={} p95={} max={}", pct(&mut vis, 0.5), pct(&mut vis, 0.95), pct(&mut vis, 1.0));
    for pitch in [0.0f32, 45.0, -45.0] {
        let sel: Vec<&Rec> = recs.iter().enumerate().filter(|(w, _)| (views[w % views.len()].1.to_degrees() - pitch).abs() < 1.0).map(|(_, r)| r).collect();
        let t = sel.iter().filter(|r| r.trip).count();
        let mut st: Vec<u32> = sel.iter().map(|r| r.considered).collect();
        println!("  pitch {:>4}: walks={} trips={} p50={} p99={}", pitch, sel.len(), t, pct(&mut st, 0.5), pct(&mut st, 0.99));
    }
    let mut mult: Vec<u32> = recs.iter().filter(|r| !r.trip && r.reach > 0).map(|r| (r.accepted * 100) / r.reach.max(1)).collect();
    println!("path multiplicity (accepted/reach x100, non-trip): p50={} p95={} p99={} max={}", pct(&mut mult, 0.5), pct(&mut mult, 0.95), pct(&mut mult, 0.99), pct(&mut mult, 1.0));
    println!("step-limit trips: {} of {} walks ({:.3}%)", trips, nwalk, 100.0 * trips as f64 / nwalk as f64);

    // trips per camera cell
    let mut by_cell: HashMap<usize, (u32, Vec3, u32)> = HashMap::new();
    for (w, r) in recs.iter().enumerate() {
        if r.trip {
            let smp = samples[w / views.len()];
            let e = by_cell.entry(smp.cam_cell).or_insert((0, smp.pos, 0));
            e.0 += 1;
            e.2 = e.2.max(r.aabb_fallback);
        }
    }
    let mut v: Vec<_> = by_cell.into_iter().collect();
    v.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    println!("distinct camera cells with >=1 trip: {}", v.len());
    let mut tf = BufWriter::new(File::create(format!("{}/trip_cells.csv", out_dir)).unwrap());
    writeln!(tf, "cam_cell,trips,x,y,z,bmin_x,bmin_y,bmin_z,bmax_x,bmax_y,bmax_z,degree,aabb_fallback").unwrap();
    for (c, (n, p, a)) in &v {
        let cd = &cells[*c];
        writeln!(tf, "{},{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{},{}", c, n, p.x, p.y, p.z, cd.bounds_min.x, cd.bounds_min.y, cd.bounds_min.z, cd.bounds_max.x, cd.bounds_max.y, cd.bounds_max.z, cd.portal_ref_count, a).unwrap();
    }
    for (c, (n, p, a)) in v.iter().take(25) {
        let cd = &cells[*c];
        println!(
            "  cell {} trips={} sample=({:.1},{:.1},{:.1}) bounds=({:.1},{:.1},{:.1})..({:.1},{:.1},{:.1}) deg={} aabb_fallback_drawable={}",
            c, n, p.x, p.y, p.z, cd.bounds_min.x, cd.bounds_min.y, cd.bounds_min.z, cd.bounds_max.x, cd.bounds_max.y, cd.bounds_max.z, cd.portal_ref_count, a
        );
    }

    // ---- cross-check harness copy vs real crate on a stride of walks
    let mut mism = 0;
    let mut checked = 0;
    let mut scratch = Vec::new();
    for w in (0..nwalk).step_by((nwalk / 2000).max(1)) {
        let smp = samples[w / views.len()];
        let (yaw, pitch) = views[w % views.len()];
        let vp = view_proj(smp.pos, yaw, pitch);
        let (res, _) = postretro_visibility::determine_visible_cells(smp.pos, vp, &world, &[], false, &mut scratch);
        let real_trip = matches!(res.stats.path, postretro_visibility::VisibilityPath::PortalStepLimitFallback { .. });
        let real_n = match &res.visible_cells {
            postretro_visibility::VisibleCells::Culled(v) => v.len() as u32,
            _ => u32::MAX,
        };
        let r = &recs[w];
        let exp_n = if r.trip { r.aabb_fallback } else { r.visible };
        if real_trip != r.trip || real_n != exp_n {
            mism += 1;
        }
        if let postretro_visibility::VisibleCells::Culled(v) = res.visible_cells {
            scratch = v;
        }
        checked += 1;
    }
    println!("cross-check vs postretro_visibility::determine_visible_cells: {} walks, {} mismatches", checked, mism);

    // ---- deep reruns on worst tripped walks with a large cap
    let mut tripped: Vec<usize> = (0..nwalk).filter(|&w| recs[w].trip).collect();
    // one per camera cell, spread
    let mut seen = std::collections::HashSet::new();
    tripped.retain(|&w| seen.insert(samples[w / views.len()].cam_cell));
    let deep_n: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(40);
    let cap: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(5_000_000);
    println!("deep reruns (cap {}) on {} of {} tripped camera cells:", cap, deep_n.min(tripped.len()), tripped.len());
    let stride = (tripped.len() / deep_n.max(1)).max(1);
    for &w in tripped.iter().step_by(stride).take(deep_n) {
        let smp = samples[w / views.len()];
        let (yaw, pitch) = views[w % views.len()];
        let fr = postretro_visibility::extract_frustum_planes(view_proj(smp.pos, yaw, pitch));
        let ts = Instant::now();
        let r = portal_vis::portal_traverse_with_step_limit(smp.pos, smp.cam_cell, &fr, &world, &[], false, cap);
        let el = ts.elapsed();
        let visible = r.visible.iter().enumerate().filter(|(i, v)| **v && world.cells[*i].is_drawable).count();
        println!(
            "  cell {} pos=({:.1},{:.1},{:.1}) yaw={:.0} pitch={:.0}: considered={} accepted={} tripped_again={} true_visible_drawable={} aabb_fallback={} time={:?} cyc={} clip={} narrow={}",
            smp.cam_cell, smp.pos.x, smp.pos.y, smp.pos.z, yaw.to_degrees(), pitch.to_degrees(), r.stats.considered, r.stats.accepted,
            r.stats.step_limit_hit, visible, recs[w].aabb_fallback, el, r.stats.rejected_path_cycle, r.stats.rejected_clipped, r.stats.rejected_narrow
        );
    }

    // ---- hot-cell histogram for the single worst tripped walk (by trace)
    if let Some(&w) = tripped.first() {
        let smp = samples[w / views.len()];
        let (yaw, pitch) = views[w % views.len()];
        let fr = postretro_visibility::extract_frustum_planes(view_proj(smp.pos, yaw, pitch));
        let (_r, trace) = portal_vis::portal_traverse_inner(smp.pos, smp.cam_cell, &fr, &world, &[], true, 20_000);
        let mut entries: HashMap<usize, u32> = HashMap::new();
        for line in trace.unwrap_or_default().lines() {
            let l = line.trim();
            if let Some(rest) = l.strip_prefix("acc ") {
                if let Some((_, b)) = rest.split(' ').next().unwrap().split_once("->") {
                    *entries.entry(b.parse().unwrap()).or_default() += 1;
                }
            }
        }
        let mut ev: Vec<_> = entries.into_iter().collect();
        ev.sort_by(|a, b| b.1.cmp(&a.1));
        println!("hot cells (entries per cell) for tripped walk cam_cell={} pos={:?}:", smp.cam_cell, smp.pos);
        for (c, n) in ev.iter().take(15) {
            let cd = &cells[*c];
            println!("   cell {} entered {}x  bounds=({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2}) deg={} drawable={}", c, n, cd.bounds_min.x, cd.bounds_min.y, cd.bounds_min.z, cd.bounds_max.x, cd.bounds_max.y, cd.bounds_max.z, cd.portal_ref_count, cd.is_drawable);
        }
    }
}

fn aabb_out(mins: Vec3, maxs: Vec3, f: &postretro_visibility::Frustum) -> bool {
    for p in &f.planes {
        let pv = Vec3::new(
            if p.normal.x >= 0.0 { maxs.x } else { mins.x },
            if p.normal.y >= 0.0 { maxs.y } else { mins.y },
            if p.normal.z >= 0.0 { maxs.z } else { mins.z },
        );
        if p.normal.dot(pv) + p.dist < 0.0 {
            return true;
        }
    }
    false
}
