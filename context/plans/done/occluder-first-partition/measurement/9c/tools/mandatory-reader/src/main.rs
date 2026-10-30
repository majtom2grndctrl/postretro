// Scratch: per camera cell, lightmap texel area of id-51 entries within lead L.
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_format::read_container;
use postretro_level_format::lightmap::LightmapBlockIndex;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let lead_m: f64 = std::env::args().nth(2).map(|s| s.parse().unwrap()).unwrap_or(16.0);
    let mut f = BufReader::new(File::open(&path).unwrap());
    let meta = read_container(&mut f).unwrap();
    let read = |f: &mut BufReader<File>, id: u32, cap: Option<u64>| -> (Vec<u8>, u64) {
        let e = meta.find_section(id).unwrap();
        let n = cap.map_or(e.size, |c| c.min(e.size));
        f.seek(SeekFrom::Start(e.offset)).unwrap();
        let mut b = vec![0u8; n as usize];
        f.read_exact(&mut b).unwrap();
        (b, e.size)
    };
    let (lm, lm_len) = read(&mut f, 22, Some(64 << 20));
    let index = LightmapBlockIndex::from_prefix(&lm, lm_len).unwrap();
    let (rs, _) = read(&mut f, 51, None);
    let cell_count = u32::from_le_bytes(rs[4..8].try_into().unwrap()) as usize;
    let set = CellResidencySetSection::from_bytes(&rs, cell_count).unwrap();
    let mut cell_texels = vec![0u64; cell_count];
    for r in &index.records {
        cell_texels[r.cell_id as usize] += u64::from(r.width) * u64::from(r.height);
    }
    let lead_max = (lead_m * 1024.0) as u32;
    let mut sums: Vec<(u64, usize)> = (0..set.camera_cell_count())
        .map(|c| (set.entries_for(c).iter().filter(|e| e.lead <= lead_max).map(|e| cell_texels[e.cell_id as usize]).sum(), c))
        .collect();
    sums.sort_unstable_by(|a, b| b.cmp(a));
    let n = sums.len();
    let mut v: Vec<u64> = sums.iter().map(|s| s.0).collect();
    v.sort_unstable();
    println!("{path}: L {lead_m} m, {n} camera cells; mandatory texels p50 {:.1} M p95 {:.1} M max {:.1} M (pool cap 15 layers ~62.9 M)",
        v[n / 2] as f64 / 1e6, v[n * 95 / 100] as f64 / 1e6, v[n - 1] as f64 / 1e6);
    for (t, c) in sums.iter().take(5) {
        println!("  cell {c}: {:.1} M, {} entries", *t as f64 / 1e6, set.entries_for(*c).len());
    }
    if let Some(c) = std::env::args().nth(3) {
        let c: usize = c.parse().unwrap();
        let t: u64 = set.entries_for(c).iter().filter(|e| e.lead <= lead_max).map(|e| cell_texels[e.cell_id as usize]).sum();
        println!("  cell {c}: {:.1} M", t as f64 / 1e6);
    }
    let over = v.iter().filter(|&&t| t as f64 > 62.9e6).count();
    println!("  cells over 62.9 M: {over}");
}
