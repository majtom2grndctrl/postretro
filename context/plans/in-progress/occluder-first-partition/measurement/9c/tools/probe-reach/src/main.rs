// Scratch (Task 9c): how far SH clusters' baked probe ranges reach into other
// clusters. Per cluster, its id-34 dense ranges (the probes it carries); a
// valid probe whose position locates to an interior cell of another cluster
// is a reach into that cluster. Also the runtime's patch-owner edges (a
// cluster depends on the lowest-id cluster carrying each of its probes) and
// sparse halo owner edges. Node (brick) owners are not counted.
// usage: shreach <prl>
use std::collections::BTreeSet;
use glam::Vec3;
use postretro_level_format::cluster_directory::{ClusterRangeRole, ClusterResourceDomain};
use postretro_level_loader::load_prl;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let w = load_prl(&path).expect("load");
    let m = w.sh_stream_manifest().expect("SH streamed");
    let dir = m.cluster_directory();
    let base = m.base();
    let [gx, gy, gz] = base.grid_dimensions.map(|v| v as usize);
    let o = Vec3::from(base.grid_origin);
    let cs = Vec3::from(base.cell_size);
    let nclu = dir.clusters.len();
    let mut cell_cluster = vec![u32::MAX; w.cells.len()];
    for (c, rec) in dir.clusters.iter().enumerate() {
        for &cell in &dir.members[rec.member_start as usize..(rec.member_start + rec.member_count) as usize] {
            cell_cluster[cell as usize] = c as u32;
        }
    }
    let base_res = dir.resources.iter().position(|r| r.section_id == 34).expect("id-34 resource");
    let ranges = |c: usize| { let r = &dir.clusters[c]; &dir.ranges[r.range_start as usize..(r.range_start + r.range_count) as usize] };
    let mut writer = vec![u32::MAX; base.probes.len()];
    let mut probe_cluster = vec![u32::MAX; base.probes.len()]; // cluster of the cell the probe locates to
    for (i, p) in base.probes.iter().enumerate() {
        if p.validity == 0 { continue; }
        let (x, y, z) = (i % gx, i / gx % gy, i / (gx * gy));
        let pos = o + cs * Vec3::new(x as f32, y as f32, z as f32);
        let cell = w.locate_cell(pos);
        let d = &w.cells[cell];
        if !d.is_solid && !d.is_exterior { probe_cluster[i] = cell_cluster[cell]; }
    }
    let _ = gz;
    for c in 0..nclu {
        for r in ranges(c) {
            if r.resource_index as usize != base_res { continue; }
            for i in r.start as usize..(r.start + r.count) as usize {
                if base.probes[i].validity != 0 { writer[i] = writer[i].min(c as u32); }
            }
        }
    }
    let (mut reach_clusters, mut reach_cells, mut reach_refs, mut reach_pairs) = (0usize, 0usize, 0usize, 0usize);
    let (mut patch_clusters, mut patch_edges, mut halo_clusters, mut halo_edges) = (0usize, 0usize, 0usize, 0usize);
    let mut max_reach = 0usize;
    let (mut deep_refs, mut deep_clusters, mut deep_cells, mut deep_pairs) = (0usize, 0usize, 0usize, 0usize);
    for c in 0..nclu {
        let (mut into, mut patch, mut halo, mut refs) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new(), 0usize);
        let rec = &dir.clusters[c];
        let own: Vec<(Vec3, Vec3)> = dir.members[rec.member_start as usize..(rec.member_start + rec.member_count) as usize]
            .iter().map(|&k| &w.cells[k as usize]).filter(|d| !d.is_solid && !d.is_exterior)
            .map(|d| (d.bounds_min, d.bounds_max)).collect();
        let mut deep_into = BTreeSet::new();
        for r in ranges(c) {
            let res = &dir.resources[r.resource_index as usize];
            if r.resource_index as usize == base_res {
                for i in r.start as usize..(r.start + r.count) as usize {
                    if base.probes[i].validity == 0 { continue; }
                    let pc = probe_cluster[i];
                    if pc != u32::MAX && pc != c as u32 {
                        into.insert(pc); refs += 1;
                        let (x, y, z) = (i % gx, i / gx % gy, i / (gx * gy));
                        let pos = o + cs * Vec3::new(x as f32, y as f32, z as f32);
                        let dist = own.iter().map(|(lo, hi)| (pos.clamp(*lo, *hi) - pos).abs().max_element()).fold(f32::INFINITY, f32::min);
                        if dist > cs.max_element() + 1e-3 { deep_into.insert(pc); deep_refs += 1; }
                    }
                    if writer[i] != c as u32 { patch.insert(writer[i]); }
                }
            } else if res.domain == ClusterResourceDomain::AffinityCell && r.role == ClusterRangeRole::Halo && r.owner_cluster_id != c as u32 {
                halo.insert(r.owner_cluster_id);
            }
        }
        if !into.is_empty() { reach_clusters += 1; reach_cells += own.len(); reach_refs += refs; reach_pairs += into.len(); max_reach = max_reach.max(into.len()); }
        if !deep_into.is_empty() { deep_clusters += 1; deep_cells += own.len(); deep_pairs += deep_into.len(); }
        if !patch.is_empty() { patch_clusters += 1; patch_edges += patch.len(); }
        if !halo.is_empty() { halo_clusters += 1; halo_edges += halo.len(); }
    }
    let interior = w.cells.iter().filter(|d| !d.is_solid && !d.is_exterior).count();
    println!("{path}\n  SH grid {gx}x{gy}x{gz} cell {:?}; {nclu} clusters, {interior} interior cells", base.cell_size);
    println!("  probe reach into another cluster's cells: {reach_clusters} clusters ({reach_cells} interior cells), {reach_pairs} cluster pairs, {reach_refs} probe refs, max {max_reach} clusters reached by one");
    println!("  of which more than one probe spacing beyond the cluster's own interior cells: {deep_clusters} clusters ({deep_cells} interior cells), {deep_pairs} cluster pairs, {deep_refs} probe refs");
    println!("  owner edges: patch {patch_edges} over {patch_clusters} clusters; sparse halo {halo_edges} over {halo_clusters} clusters");
}
