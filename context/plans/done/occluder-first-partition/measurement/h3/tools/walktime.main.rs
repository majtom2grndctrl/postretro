// Task 9d (H3) scratch: CPU cost of the portal walk per world pose, timed in-process.
// Calls the engine's own `determine_visible_cells` (the `visibility` CPU stage) with the
// harness camera of 9g's uncap9g (render_camera: 100 deg HFOV at 16:9, near 0.1,
// far 4096, yaw and pitch) and TimingGate::ON, as the engine runs under
// POSTRETRO_CPU_TIMING=1. Built twice: against crates/visibility (the 5,000-step cap,
// "capped") and against 9g's copy whose only edit is MAX_PORTAL_WALK_STEPS =
// 10,000,000 ("uncapped"). Per pose: 3 warm calls, then repeated calls until 40 ms
// and at least 15 calls; reports the median wall time of the whole call and the
// median `PortalWalk` stage time.
// usage: walktime <prl> <poses file> [pose-label-filter-file]
use glam::{Mat4, Vec3};
use postretro_level_loader::load_prl;
use postretro_visibility::{PortalWalkScratch, TimingGate, VisibilityStage, determine_visible_cells};
use std::time::Instant;

fn camera(eye: Vec3, yaw: f32, pitch: f32) -> Mat4 {
    let (up, level) = pitch.sin_cos();
    let look = Vec3::new(-yaw.sin() * level, up, -yaw.cos() * level);
    let view = glam::camera::rh::view::look_at_mat4(eye, eye + look, Vec3::Y);
    let aspect = 16.0f32 / 9.0;
    let vfov = 2.0 * ((100f32.to_radians() / 2.0).tan() / aspect).atan();
    let proj = glam::camera::rh::proj::directx::perspective(vfov, aspect, 0.1, 4096.0);
    proj * view
}

fn median(v: &mut Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { 0.5 * (v[n / 2 - 1] + v[n / 2]) }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let world = load_prl(&a[1]).expect("load");
    let text = std::fs::read_to_string(&a[2]).unwrap();
    let only: Option<Vec<String>> = a.get(3).map(|f| {
        std::fs::read_to_string(f).unwrap().lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
    });
    let mut scratch = PortalWalkScratch::new();
    println!("# label considered accepted step_limit path visibility_us_med walk_us_med calls");
    for line in text.lines().filter(|l| l.starts_with("pose ")) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if let Some(o) = &only { if !o.iter().any(|x| x == f[1]) { continue; } }
        let p = |i: usize| f[i].parse::<f32>().unwrap();
        let eye = Vec3::new(p(2), p(3), p(4));
        let vp = camera(eye, p(5).to_radians(), p(6).to_radians());
        let mut call = || determine_visible_cells(eye, vp, &world, &[], false, &mut scratch, TimingGate::ON).0;
        for _ in 0..3 { std::hint::black_box(call()); }
        let (mut tv, mut tw) = (Vec::new(), Vec::new());
        let start = Instant::now();
        let mut last = None;
        while tv.len() < 15 || start.elapsed().as_secs_f64() < 0.040 {
            let t0 = Instant::now();
            let vis = call();
            tv.push(t0.elapsed().as_secs_f64() * 1e6);
            tw.push(vis.stats.cpu.value(VisibilityStage::PortalWalk).map_or(f64::NAN, |ns| ns as f64 / 1e3));
            last = Some(vis);
        }
        let vis = last.unwrap();
        let st = &vis.stats.cpu;
        let considered = st.value(VisibilityStage::Considered).unwrap_or(0);
        let accepted = st.value(VisibilityStage::Accepted).unwrap_or(0);
        let trip = st.value(VisibilityStage::StepLimit).unwrap_or(0);
        let n = tv.len();
        println!("{} {} {} {} {:?} {:.2} {:.2} {}", f[1], considered, accepted, trip, vis.stats.path, median(&mut tv), median(&mut tw), n);
    }
}
