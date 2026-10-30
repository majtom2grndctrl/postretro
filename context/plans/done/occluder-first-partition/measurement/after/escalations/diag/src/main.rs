// Scratch diagnostic for Task 9: why a ray-grid hit lands in an unreached cell.
// usage: diag miss <prl> <eye x,y,z> <yaw_deg> <point x,y,z>
//        diag pose <prl> <eye x,y,z> <yaw_deg>      (walk stats + portal-test drivers)
use glam::{Mat4, Vec3};
use postretro_level_loader::{LevelWorld, load_prl};
use postretro_visibility::{
    PortalWalkScratch, TimingGate, VisibilityStage, VisibleCells, determine_visible_cells,
};

fn v3(s: &str) -> Vec3 {
    let p: Vec<f32> = s.split(',').map(|x| x.parse().unwrap()).collect();
    Vec3::new(p[0], p[1], p[2])
}

fn camera(eye: Vec3, yaw_deg: f32) -> Mat4 {
    let yaw = yaw_deg.to_radians();
    let look = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    let view = Mat4::look_at_rh(eye, eye + look, Vec3::Y);
    let aspect = 16.0f32 / 9.0;
    let vfov = 2.0 * ((100f32.to_radians() / 2.0).tan() / aspect).atan();
    let proj = glam::camera::rh::proj::directx::perspective(vfov, aspect, 0.1, 4096.0);
    proj * view
}

fn walk(world: &LevelWorld, eye: Vec3, yaw: f32) -> (Vec<u32>, Vec<u32>, u32, String) {
    let mut scratch = PortalWalkScratch::new();
    let (vis, _) = determine_visible_cells(eye, camera(eye, yaw), world, &[], false, &mut scratch, TimingGate::ON);
    let drawn = match &vis.visible_cells {
        VisibleCells::Culled(c) => c.clone(),
        VisibleCells::DrawAll => vec![],
    };
    let considered = vis.stats.cpu.value(VisibilityStage::Considered).unwrap_or(0) as u32;
    let mut reach = vis.fog_reachable.clone();
    reach.sort_unstable();
    (drawn, reach, considered, format!("{:?}", vis.stats.path))
}

fn bounds(world: &LevelWorld, c: usize) -> String {
    let cd = &world.cells[c];
    let e = cd.bounds_max - cd.bounds_min;
    format!(
        "cell {c} min {:.2?} max {:.2?} ext {:.2}x{:.2}x{:.2} solid={} drawable={} faces={} portals={}",
        cd.bounds_min.to_array(),
        cd.bounds_max.to_array(),
        e.x,
        e.y,
        e.z,
        cd.is_solid,
        cd.is_drawable,
        cd.face_count,
        world.cell_portal_count(c)
    )
}

fn poly_area(p: &[Vec3]) -> f32 {
    let mut a = Vec3::ZERO;
    for i in 1..p.len().saturating_sub(1) {
        a += (p[i] - p[0]).cross(p[i + 1] - p[0]);
    }
    a.length() * 0.5
}

fn inside_poly(p: &[Vec3], n: Vec3, x: Vec3) -> bool {
    let k = p.len();
    for i in 0..k {
        let e = p[(i + 1) % k] - p[i];
        if e.cross(x - p[i]).dot(n) < -1e-4 {
            return false;
        }
    }
    true
}

fn portal_normal(p: &[Vec3]) -> Vec3 {
    let mut a = Vec3::ZERO;
    for i in 1..p.len().saturating_sub(1) {
        a += (p[i] - p[0]).cross(p[i + 1] - p[0]);
    }
    a.normalize_or_zero()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args[1] == "locate" {
        let before = load_prl(&args[2]).expect("load before");
        let after = load_prl(&args[3]).expect("load after");
        let class = |w: &LevelWorld, p: Vec3| { let c = &w.cells[w.locate_cell(p)]; if c.is_solid { 's' } else if c.is_exterior { 'x' } else { 'i' } };
        let text = std::fs::read_to_string(&args[4]).unwrap();
        let offsets: [(&str, f32); 6] = [("feet", -1.5), ("feet+1mm", -1.499), ("core_bottom", -1.3), ("centre", -0.5), ("eye", 0.0), ("core_top", 0.3)];
        let mut counts: std::collections::BTreeMap<(String, char, char), u32> = Default::default();
        let mut shown = 0;
        let mut seen = std::collections::HashSet::new();
        for line in text.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            let (label, eye) = if line.starts_with("pose ") {
                (f[1].to_string(), Vec3::new(f[2].parse().unwrap(), f[3].parse().unwrap(), f[4].parse().unwrap()))
            } else if line.starts_with("point ") {
                // point <label> x y z  (feet point; eye = +1.5)
                (f[1].to_string(), Vec3::new(f[2].parse().unwrap(), f[3].parse::<f32>().unwrap() + 1.5, f[4].parse().unwrap()))
            } else { continue };
            if !seen.insert((eye.x.to_bits(), eye.y.to_bits(), eye.z.to_bits())) { continue; }
            for (name, dy) in offsets {
                let p = eye + Vec3::new(0.0, dy, 0.0);
                let (b, a) = (class(&before, p), class(&after, p));
                *counts.entry((name.to_string(), b, a)).or_default() += 1;
                if b == 'i' && a != 'i' && shown < 40 { shown += 1; println!("  {label} {name} {:?} before i after {a} (after cell {})", p.to_array(), after.locate_cell(p)); }
            }
        }
        for ((n, b, a), c) in counts { println!("{n}: before {b} after {a}: {c}"); }
        return;
    }
    let world = load_prl(&args[2]).expect("load");
    let eye = v3(&args[3]);
    let yaw: f32 = args[4].parse().unwrap();
    let (drawn, reach, considered, path) = walk(&world, eye, yaw);
    let cam = world.locate_cell(eye);
    println!("path {path} considered {considered} drawn {} reach {}", drawn.len(), reach.len());
    println!("camera {}", bounds(&world, cam));
    match args[1].as_str() {
        "miss" => {
            let target = v3(&args[5]);
            let dir = (target - eye).normalize();
            let len = (target - eye).length();
            let step = 0.005f32;
            let mut prev = cam;
            let mut t = 0.0f32;
            println!("ray len {len:.2}");
            while t <= len {
                let p = eye + dir * t;
                let c = world.locate_cell(p);
                if c != prev {
                    let r = reach.binary_search(&(c as u32)).is_ok();
                    println!("t={t:.3} p={:.3?} -> {} reached={r}", p.to_array(), bounds(&world, c));
                    // portals joining prev and c
                    let mut any = false;
                    for (i, po) in world.portals.iter().enumerate() {
                        if (po.front_cell == prev && po.back_cell == c) || (po.front_cell == c && po.back_cell == prev) {
                            any = true;
                            let n = portal_normal(&po.polygon);
                            let d = n.dot(po.polygon[0]);
                            let denom = n.dot(dir);
                            let hit = if denom.abs() > 1e-6 {
                                let s = (d - n.dot(eye)) / denom;
                                let x = eye + dir * s;
                                format!("plane hit t={s:.3} inside={}", inside_poly(&po.polygon, n, x))
                            } else {
                                "parallel".into()
                            };
                            println!(
                                "    portal {i} {}->{} area {:.3} n {:.3?} verts {} {hit}  poly {:.2?}",
                                po.front_cell,
                                po.back_cell,
                                poly_area(&po.polygon),
                                n.to_array(),
                                po.polygon.len(),
                                po.polygon.iter().map(|v| v.to_array()).collect::<Vec<_>>()
                            );
                        }
                    }
                    if !any {
                        println!("    no portal between {prev} and {c}");
                    }
                    prev = c;
                }
                t += step;
            }
        }
        "pose" => {
            // degree distribution of drawn cells
            let mut deg: Vec<(usize, u32)> = drawn.iter().map(|&c| (world.cell_portal_count(c as usize), c)).collect();
            deg.sort_unstable_by(|a, b| b.cmp(a));
            let total: usize = deg.iter().map(|d| d.0).sum();
            println!("sum portal degree over drawn cells {total}");
            for (d, c) in deg.iter().take(15) {
                println!("  deg {d} {}", bounds(&world, *c as usize));
            }
        }
        "face" => {
            // sample a rectangle: args[5]=corner a, args[6]=corner b (axis-aligned face), args[7]=normal offset axis (x|y|z)
            let a = v3(&args[5]);
            let b = v3(&args[6]);
            let ax = match args[7].as_str() { "x" => 0, "y" => 1, _ => 2 };
            let n = 12;
            for side in [-0.005f32, 0.005] {
                println!("side offset {side}");
                let mut seen: std::collections::BTreeMap<(usize, bool), (usize, Vec3, Vec3)> = Default::default();
                for i in 0..=n {
                    for j in 0..=n {
                        let mut p = a;
                        let mut k = 0;
                        for d in 0..3 {
                            if d == ax { continue; }
                            let f = if k == 0 { i as f32 / n as f32 } else { j as f32 / n as f32 };
                            p[d] = a[d] + (b[d] - a[d]) * f;
                            k += 1;
                        }
                        p[ax] += side;
                        let (leaf, cell) = leaf_of(&world, p);
                        let e = seen.entry(leaf).or_insert((cell, p, p));
                        e.1 = e.1.min(p);
                        e.2 = e.2.max(p);
                    }
                }
                for (leaf, (cell, lo, hi)) in seen {
                    println!("  leaf node {} front {} cell {} solid {} samples {:.3?}..{:.3?}", leaf.0, leaf.1, cell, world.cells[cell].is_solid, lo.to_array(), hi.to_array());
                }
            }
        }
        _ => {}
    }
}

fn leaf_of(world: &LevelWorld, p: Vec3) -> ((usize, bool), usize) {
    use postretro_level_loader::CellLocatorChild;
    let mut cur = world.cell_locator_root;
    let mut last = (usize::MAX, true);
    loop {
        match cur {
            CellLocatorChild::Cell(c) => return (last, c),
            CellLocatorChild::Node(ni) => {
                let node = &world.cell_locator_nodes[ni];
                let front = node.plane_normal.dot(p) - node.plane_distance >= 0.0;
                last = (ni, front);
                cur = if front { node.front } else { node.back };
            }
        }
    }
}
