// Scratch (Task 9c): classify thin interior cells (smallest extent in [lo, hi)).
// usage: thin <prl> <cap_m> [lo hi] [-v]
use postretro_level_loader::load_prl;

fn on_line(v: f32, cap: f32) -> bool {
    let k = (v / cap).round();
    k != 0.0 && (v - k * cap).abs() < 2e-3
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let w = load_prl(&a[1]).expect("load");
    let cap: f32 = a[2].parse().unwrap();
    let lo: f32 = a.get(3).map_or(0.25, |s| s.parse().unwrap());
    let hi: f32 = a.get(4).map_or(0.5, |s| s.parse().unwrap());
    let verbose = a.iter().any(|s| s == "-v");
    let ext = |c: usize| w.cells[c].bounds_max - w.cells[c].bounds_min;
    let thin: Vec<usize> = (0..w.cells.len())
        .filter(|&c| { let d = &w.cells[c]; !d.is_solid && !d.is_exterior })
        .filter(|&c| { let e = ext(c); let m = e.x.min(e.y).min(e.z); m >= lo && m < hi })
        .collect();
    let mut by_axis = [0usize; 3];
    let (mut long_on_cap, mut thin_on_cap, mut split_nb, mut faceless) = (0, 0, 0, 0);
    let mut parent: Vec<usize> = (0..w.cells.len()).collect();
    fn find(p: &mut Vec<usize>, x: usize) -> usize { let mut x = x; while p[x] != x { p[x] = p[p[x]]; x = p[x]; } x }
    for &c in &thin {
        let d = &w.cells[c];
        let e = ext(c);
        let ax = if e.x <= e.y && e.x <= e.z { 0 } else if e.y <= e.z { 1 } else { 2 };
        by_axis[ax] += 1;
        let horiz = [0usize, 2];
        let tb = |v: glam::Vec3| v[ax];
        let thin_cap = horiz.contains(&ax) && (on_line(tb(d.bounds_min), cap) || on_line(tb(d.bounds_max), cap));
        let long_cap = horiz.iter().filter(|&&h| h != ax).any(|&h| on_line(d.bounds_min[h], cap) || on_line(d.bounds_max[h], cap));
        if thin_cap { thin_on_cap += 1; }
        if long_cap { long_on_cap += 1; }
        if d.face_count == 0 { faceless += 1; }
        // Slab neighbours: same thin-axis bounds, joined by a portal.
        let mut nbs = vec![];
        for k in 0..w.cell_portal_count(c) {
            let Some(p) = w.cell_portal_index(c, k) else { continue };
            let pd = &w.portals[p];
            let o = if pd.front_cell == c { pd.back_cell } else { pd.front_cell };
            let od = &w.cells[o];
            if (od.bounds_min[ax] - d.bounds_min[ax]).abs() < 2e-3 && (od.bounds_max[ax] - d.bounds_max[ax]).abs() < 2e-3 {
                let on = pd.polygon.iter().all(|v| horiz.iter().filter(|&&h| h != ax).any(|&h| on_line(v[h], cap)));
                nbs.push((o, on));
                if on { let (x, y) = (find(&mut parent, c), find(&mut parent, o)); parent[x] = y; }
            }
        }
        if nbs.iter().any(|n| n.1) { split_nb += 1; }
        if verbose {
            println!("cell {c} axis {} min {:.3?} max {:.3?} ext {:.3}x{:.3}x{:.3} faces {} portals {} thin_on_cap {} long_on_cap {} slab_nbs {:?}",
                ["x","y","z"][ax], d.bounds_min.to_array(), d.bounds_max.to_array(), e.x, e.y, e.z, d.face_count, w.cell_portal_count(c), thin_cap, long_cap, nbs);
        }
    }
    let mut roots: Vec<usize> = thin.iter().map(|&c| find(&mut parent, c)).collect();
    roots.sort_unstable(); roots.dedup();
    println!("{}: interior cells with smallest extent [{lo}, {hi}) m: {} (thin axis x {}, y {}, z {}); faceless {}; long-side bound on a {cap} m cap line {}; thin-side bound on a cap line {}; joined to a same-slab neighbour across a cap-line portal {}; distinct slabs after re-joining cap splits {}",
        a[1], thin.len(), by_axis[0], by_axis[1], by_axis[2], faceless, long_on_cap, thin_on_cap, split_nb, roots.len());
}
