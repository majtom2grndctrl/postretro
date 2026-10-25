// Driver: exact walk vs bounded region-propagation alternatives. Single-threaded.
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::Instant;

use glam::Vec3;
use postretro_level_loader::LevelWorld;

use crate::alt_vis::*;
use crate::portal_vis;

enum Kind {
    R2(Scratch<2>),
    R4(Scratch<4>),
    Hyb(HybScratch, u32),
}

struct Var {
    name: &'static str,
    kind: Kind,
}

#[derive(Default, Clone, Copy)]
struct VRec {
    steps: u32,
    ns: u64,
    draw: u32,
    faces: u32,
    reach: u32,
    miss_draw: u32,
    miss_reach: u32,
    reprop_tot: u32,
    reprop_max: u32,
    hd_reexp_max: u32,
    camplane: u32,
    eyefb: u32,
    permdiff: u32,
    trip: bool,
    exact_steps: u32,
}

fn shuffle_refs(w: &mut LevelWorld, seed: u64) {
    let mut s = seed;
    let mut rnd = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    for c in 0..w.cells.len() {
        let st = w.cells[c].portal_ref_start as usize;
        let n = w.cells[c].portal_ref_count as usize;
        let sl = &mut w.cell_portal_refs[st..st + n];
        for i in (1..n).rev() {
            let j = (rnd() % (i as u64 + 1)) as usize;
            sl.swap(i, j);
        }
    }
}

fn run_var(v: &mut Var, world: &LevelWorld, geo: &Geo, cam: &Cam, fr: &postretro_visibility::Frustum, cc: usize, vb: &mut [bool]) -> (AltStats, u64) {
    let t = Instant::now();
    let st = match &mut v.kind {
        Kind::R2(s) => region_traverse(world, geo, cam, cc, s),
        Kind::R4(s) => region_traverse(world, geo, cam, cc, s),
        Kind::Hyb(h, b) => hybrid_traverse(world, geo, cam, fr, cc, *b, None, h),
    };
    let ns = t.elapsed().as_nanos() as u64;
    vb.iter_mut().for_each(|x| *x = false);
    match &v.kind {
        Kind::R2(s) => s.touched.iter().for_each(|&c| vb[c] = true),
        Kind::R4(s) => s.touched.iter().for_each(|&c| vb[c] = true),
        Kind::Hyb(h, _) => {
            for (i, x) in h.visible.iter().enumerate() {
                if *x {
                    vb[i] = true;
                }
            }
            h.rs.touched.iter().for_each(|&c| vb[c] = true);
        }
    }
    (st, ns)
}

fn hd_reexp(v: &Var, hideg: &[bool], hd_cells: &[usize]) -> u32 {
    let _ = hideg;
    let mut m = 0;
    for &c in hd_cells {
        let vis = match &v.kind {
            Kind::R2(s) => s.visits(c),
            Kind::R4(s) => s.visits(c),
            Kind::Hyb(h, _) => h.rs.visits(c),
        };
        m = m.max(vis.saturating_sub(1));
    }
    m
}

pub fn run(args: &[String]) {
    let path = &args[0];
    let out_dir = &args[1];
    let stride: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
    let world = crate::load(path);
    let mut wperm = crate::load(path);
    shuffle_refs(&mut wperm, 0x9E3779B97F4A7C15);
    let geo = build_geo(&world);
    println!("portal orientation: locate fallbacks={} ambiguous={}", geo.orient_fallbacks, geo.orient_bad);
    let n = world.cell_count();
    let cells = &world.cells;
    let hideg: Vec<bool> = cells.iter().map(|c| !c.is_solid && c.portal_ref_count >= 25).collect();
    let hd_cells: Vec<usize> = (0..n).filter(|&c| hideg[c]).collect();
    println!("high-degree (>=25) cells: {:?}", hd_cells.iter().map(|&c| (c, cells[c].portal_ref_count)).collect::<Vec<_>>());

    // samples: identical to main sweep
    let mut samples = Vec::new();
    for (i, c) in cells.iter().enumerate() {
        if c.is_solid || c.is_exterior {
            continue;
        }
        let ctr = (c.bounds_min + c.bounds_max) * 0.5;
        let h = c.bounds_max.y - c.bounds_min.y;
        let mut cands = vec![(0u8, ctr)];
        if h > crate::EYE + 0.1 {
            cands.push((1u8, Vec3::new(ctr.x, c.bounds_min.y + crate::EYE, ctr.z)));
        }
        for (k, p) in cands {
            let cc = world.locate_cell(p);
            if cc >= cells.len() || cells[cc].is_solid || cells[cc].is_exterior {
                continue;
            }
            samples.push((i, k, p, cc));
        }
    }
    let mut views = Vec::new();
    for pitch in [0.0f32, 45.0, -45.0] {
        for k in 0..8 {
            let off: f32 = std::env::var("YAW_OFFSET_DEG").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
            views.push(((k as f32 * 45.0 + off).to_radians(), pitch.to_radians()));
        }
    }
    let nwalk = samples.len() * views.len();
    println!("samples={} walks={} stride={}", samples.len(), nwalk, stride);

    let base = |q: f32| Cfg { q, facing: true, ordered: true, eye_parent: false, delta: 0.01 };
    let mut vars = vec![
        Var { name: "rect256", kind: Kind::R2(Scratch::new(n, base(256.0))) },
        Var { name: "rect256fifo", kind: Kind::R2(Scratch::new(n, Cfg { ordered: false, ..base(256.0) })) },
        Var { name: "rect256nofacing", kind: Kind::R2(Scratch::new(n, Cfg { facing: false, ..base(256.0) })) },
        Var { name: "oct64", kind: Kind::R4(Scratch::new(n, Cfg { eye_parent: true, ..base(64.0) })) },
        Var { name: "oct256", kind: Kind::R4(Scratch::new(n, Cfg { eye_parent: true, ..base(256.0) })) },
        Var { name: "oct256clip", kind: Kind::R4(Scratch::new(n, base(256.0))) },
        Var { name: "hyb2k", kind: Kind::Hyb(HybScratch::new(n, base(256.0)), 2000) },
        Var { name: "hyb500", kind: Kind::Hyb(HybScratch::new(n, base(256.0)), 500) },
    ];
    if let Ok(f) = std::env::var("ALT_VARIANTS") {
        let keep: Vec<&str> = f.split(',').collect();
        vars.retain(|v| keep.contains(&v.name));
    }
    let mut href = HybScratch::new(n, base(256.0));

    let mut csv = BufWriter::new(File::create(format!("{}/alt_sweep.csv", out_dir)).unwrap());
    let mut miss_f = BufWriter::new(File::create(format!("{}/alt_missing.csv", out_dir)).unwrap());
    writeln!(miss_f, "walk,variant,x,y,z,yaw,pitch,cam_cell,missing_cells").unwrap();
    write!(csv, "walk,src_cell,kind,cam_cell,x,y,z,yaw,pitch,fa_draw,fa_faces,ex_steps,ex_ns,excap_ns,ex_trip,ex_draw,ex_faces,ex_reach,ex_hd_entries_max,ex_permdiff,hybinf_mismatch").unwrap();
    for v in &vars {
        let p = v.name;
        write!(csv, ",{p}_steps,{p}_ns,{p}_draw,{p}_faces,{p}_reach,{p}_miss_draw,{p}_miss_reach,{p}_reprop_tot,{p}_reprop_max,{p}_hd_reexp_max,{p}_camplane,{p}_eyefb,{p}_permdiff,{p}_trip,{p}_exact_steps").unwrap();
    }
    writeln!(csv).unwrap();

    let mut ex_vis = vec![false; n];
    let mut vb = vec![false; n];
    let mut vp_buf = vec![false; n];
    let t_all = Instant::now();
    let mut done = 0usize;
    for w in (0..nwalk).step_by(stride) {
        let (src, kind, pos, cc) = samples[w / views.len()];
        let (yaw, pitch) = views[w % views.len()];
        let vp = crate::view_proj(pos, yaw, pitch);
        let fr = postretro_visibility::extract_frustum_planes(vp);
        let cam = Cam::new(pos, vp, &fr);

        // frustum-all upper bound
        let (mut fa_draw, mut fa_faces) = (0u32, 0u32);
        for c in cells.iter() {
            if c.is_drawable && !crate::aabb_out(c.bounds_min, c.bounds_max, &fr) {
                fa_draw += 1;
                fa_faces += c.face_count;
            }
        }

        // exact uncapped (reference), min of 2
        let mut ex_ns = u64::MAX;
        let mut ex = None;
        for _ in 0..2 {
            let t = Instant::now();
            let r = portal_vis::portal_traverse_with_step_limit(pos, cc, &fr, &world, &[], false, 50_000_000);
            ex_ns = ex_ns.min(t.elapsed().as_nanos() as u64);
            ex = Some(r);
        }
        let ex = ex.unwrap();
        ex_vis.copy_from_slice(&ex.visible);
        // exact capped at 20k + frustum-all fallback on trip (the shipping behaviour)
        let mut excap_ns = u64::MAX;
        let mut ex_trip = false;
        for _ in 0..2 {
            let t = Instant::now();
            let r = portal_vis::portal_traverse_with_step_limit(pos, cc, &fr, &world, &[], false, 20_000);
            if r.stats.step_limit_hit {
                ex_trip = true;
                let v: Vec<usize> = cells
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.is_drawable && !crate::aabb_out(c.bounds_min, c.bounds_max, &fr))
                    .map(|(i, _)| i)
                    .collect();
                std::hint::black_box(v);
            }
            std::hint::black_box(&r);
            excap_ns = excap_ns.min(t.elapsed().as_nanos() as u64);
        }
        let (mut ex_draw, mut ex_faces, mut ex_reach) = (0u32, 0u32, 0u32);
        for c in 0..n {
            if ex_vis[c] {
                ex_reach += 1;
                if cells[c].is_drawable {
                    ex_draw += 1;
                    ex_faces += cells[c].face_count;
                }
            }
        }
        // exact on permuted portal order
        let rp = portal_vis::portal_traverse_with_step_limit(pos, cc, &fr, &wperm, &[], false, 50_000_000);
        let ex_permdiff = (0..n).filter(|&c| rp.visible[c] != ex_vis[c]).count() as u32;
        // hybrid with infinite budget == exact copy (sanity) + high-degree entry counts
        let _ = hybrid_traverse(&world, &geo, &cam, &fr, cc, u32::MAX, Some(&hideg), &mut href);
        let hybinf_mismatch = (0..n).filter(|&c| href.visible[c] != ex_vis[c]).count() as u32;
        let ex_hd = href.ent_touched.iter().map(|&c| href.entries[c]).max().unwrap_or(0);

        write!(
            csv,
            "{},{},{},{},{:.2},{:.2},{:.2},{:.0},{:.0},{},{},{},{},{},{},{},{},{},{},{},{}",
            w, src, kind, cc, pos.x, pos.y, pos.z, yaw.to_degrees(), pitch.to_degrees(), fa_draw, fa_faces, ex.stats.considered, ex_ns, excap_ns,
            ex_trip as u8, ex_draw, ex_faces, ex_reach, ex_hd, ex_permdiff, hybinf_mismatch
        )
        .unwrap();

        for v in vars.iter_mut() {
            let mut best = u64::MAX;
            let mut st = AltStats::default();
            for _ in 0..2 {
                let (s, ns) = run_var(v, &world, &geo, &cam, &fr, cc, &mut vb);
                best = best.min(ns);
                st = s;
            }
            let mut r = VRec { steps: st.steps, ns: best, reprop_tot: st.reprop_total, reprop_max: st.reprop_max, camplane: st.camplane, eyefb: st.wmin_fallback, trip: st.tripped, exact_steps: st.exact_steps, ..Default::default() };
            let mut missing = Vec::new();
            for c in 0..n {
                if vb[c] {
                    r.reach += 1;
                    if cells[c].is_drawable {
                        r.draw += 1;
                        r.faces += cells[c].face_count;
                    }
                }
                if ex_vis[c] && !vb[c] {
                    r.miss_reach += 1;
                    if cells[c].is_drawable {
                        r.miss_draw += 1;
                    }
                    missing.push(c);
                }
            }
            if !missing.is_empty() {
                writeln!(miss_f, "{},{},{:.2},{:.2},{:.2},{:.0},{:.0},{},{:?}", w, v.name, pos.x, pos.y, pos.z, yaw.to_degrees(), pitch.to_degrees(), cc, &missing[..missing.len().min(12)]).unwrap();
            }
            r.hd_reexp_max = hd_reexp(v, &hideg, &hd_cells);
            // permuted portal order
            let _ = run_var(v, &wperm, &geo, &cam, &fr, cc, &mut vp_buf);
            r.permdiff = (0..n).filter(|&c| vp_buf[c] != vb[c]).count() as u32;
            write!(
                csv,
                ",{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                r.steps, r.ns, r.draw, r.faces, r.reach, r.miss_draw, r.miss_reach, r.reprop_tot, r.reprop_max, r.hd_reexp_max, r.camplane, r.eyefb, r.permdiff, r.trip as u8, r.exact_steps
            )
            .unwrap();
        }
        writeln!(csv).unwrap();
        done += 1;
        if done % 10000 == 0 {
            eprintln!("{} walks in {:?}", done, t_all.elapsed());
        }
    }
    eprintln!("done {} walks in {:?}", done, t_all.elapsed());
}

// Debug: find an exact chain reaching `target`, replay it against rect propagation.
pub fn debug(args: &[String]) {
    let world = crate::load(&args[0]);
    let walk: usize = args[1].parse().unwrap();
    let off: f32 = args[2].parse().unwrap();
    let target: usize = args[3].parse().unwrap();
    let mut samples = Vec::new();
    for c in world.cells.iter() {
        if c.is_solid || c.is_exterior { continue; }
        let ctr = (c.bounds_min + c.bounds_max) * 0.5;
        let h = c.bounds_max.y - c.bounds_min.y;
        let mut cands = vec![ctr];
        if h > crate::EYE + 0.1 { cands.push(Vec3::new(ctr.x, c.bounds_min.y + crate::EYE, ctr.z)); }
        for p in cands {
            let cc = world.locate_cell(p);
            if cc >= world.cells.len() || world.cells[cc].is_solid || world.cells[cc].is_exterior { continue; }
            samples.push(p);
        }
    }
    let pos = samples[walk / 24];
    let k = walk % 24;
    let pitch = [0.0f32, 45.0, -45.0][k / 8];
    let yaw = (k % 8) as f32 * 45.0 + off;
    println!("pos {:?} yaw {} pitch {}", pos, yaw, pitch);
    let cc = world.locate_cell(pos);
    let vp = crate::view_proj(pos, yaw.to_radians(), pitch.to_radians());
    let fr = postretro_visibility::extract_frustum_planes(vp);
    let cam = Cam::new(pos, vp, &fr);
    let geo = build_geo(&world);
    let n = world.cell_count();
    let q = 256.0;
    let mut s: Scratch<2> = Scratch::new(n, Cfg { q, facing: true, ordered: true, eye_parent: false, delta: 0.01 });
    let st = region_traverse(&world, &geo, &cam, cc, &mut s);
    println!("cam cell {} rect steps {} reached target {}", cc, st.steps, s.reached(target));
    // exact DFS to target
    let mut vf = fr.clone();
    vf.planes[4] = cam.near;
    let mut path: Vec<(usize, usize, Vec<Vec3>)> = Vec::new();
    let mut found = None;
    fn dfs(world: &LevelWorld, cam: &Cam, cell: usize, f: &postretro_visibility::Frustum, target: usize, path: &mut Vec<(usize, usize, Vec<Vec3>)>, found: &mut Option<Vec<(usize, usize, Vec<Vec3>)>>, budget: &mut u32) {
        if found.is_some() || *budget == 0 { return; }
        if cell == target { *found = Some(path.clone()); return; }
        for i in 0..world.cell_portal_count(cell) {
            *budget = budget.saturating_sub(1);
            let pi = world.cell_portal_index(cell, i).unwrap();
            let p = &world.portals[pi];
            let nb = if p.front_cell == cell { p.back_cell } else { p.front_cell };
            if path.iter().any(|x| x.1 == pi) || world.cells[nb].is_solid { continue; }
            let (mut a, mut b) = (Vec::new(), Vec::new());
            let (nf, poly) = if portal_vis::camera_on_polygon_plane(cam.pos, &p.polygon) {
                (portal_vis::narrow_frustum(cam.pos, &p.polygon, f), p.polygon.clone())
            } else {
                let c = portal_vis::clip_polygon_to_frustum(&p.polygon, f, &mut a, &mut b).to_vec();
                if c.len() < 3 { continue; }
                (portal_vis::narrow_frustum(cam.pos, &c, f), c)
            };
            let Some(nf) = nf else { continue };
            path.push((cell, pi, poly));
            dfs(world, cam, nb, &nf, target, path, found, budget);
            path.pop();
            if found.is_some() { return; }
        }
    }
    let mut budget = 50_000_000;
    dfs(&world, &cam, cc, &vf, target, &mut path, &mut found, &mut budget);
    let Some(chain) = found else { println!("no exact chain"); return };
    for (k, (c, pi, poly)) in chain.iter().enumerate() {
        let p = &world.portals[*pi];
        let nb = if p.front_cell == *c { p.back_cell } else { p.front_cell };
        let g = &geo.portals[*pi];
        let raw = g.normal.dot(cam.pos - g.centroid);
        let s_front = -raw * g.front_neg;
        let s_src = if p.front_cell == *c { s_front } else { -s_front };
        let ndc: Vec<(f32, f32, f32)> = poly.iter().map(|q| { let h = q.extend(1.0); let w = cam.rows[3].dot(h); (cam.rows[0].dot(h) / w, cam.rows[1].dot(h) / w, w) }).collect();
        let r = s.region(*c);
        println!("hop {} cell {} (rect reached {} region x[{},{}] y[{},{}]) -> portal {} -> {} (reached {}) s_src={:.5} n={:?} nverts={} area_poly={:.4}",
            k, c, s.reached(*c), r.lo[0], r.hi[0], r.lo[1], r.hi[1], pi, nb, s.reached(nb), s_src, g.normal, p.polygon.len(), 0.0);
        println!("    exact clipped ndc*256: {:?}", ndc.iter().map(|(x, y, w)| (x * 256.0, y * 256.0, *w)).collect::<Vec<_>>());
        let mut planes = vec![cam.near];
        r.push_planes(&cam.rows, q, &mut planes);
        planes.push(cam.far);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let rc = clip_poly(&p.polygon, &planes, &mut a, &mut b);
        println!("    rect clip of full portal vs region: {} verts", rc.len());
    }
}
