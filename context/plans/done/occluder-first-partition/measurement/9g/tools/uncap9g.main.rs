// Task 9g scratch: uncapped portal-walk cost per world pose.
// Runs the unchanged walk from a copy of crates/visibility/src whose only edit
// is MAX_PORTAL_WALK_STEPS = 10,000,000, with the harness camera
// (render_camera: 100 deg HFOV at 16:9, near 0.1, far 4096, yaw and pitch).
// usage: uncap9g <prl> <poses file>    -> one row per pose, then a summary
use glam::{Mat4, Vec3};
use postretro_level_loader::load_prl;
use postretro_visibility::{PortalWalkScratch, TimingGate, VisibilityStage, determine_visible_cells};

fn camera(eye: Vec3, yaw: f32, pitch: f32) -> Mat4 {
    let (up, level) = pitch.sin_cos();
    let look = Vec3::new(-yaw.sin() * level, up, -yaw.cos() * level);
    let view = glam::camera::rh::view::look_at_mat4(eye, eye + look, Vec3::Y);
    let aspect = 16.0f32 / 9.0;
    let vfov = 2.0 * ((100f32.to_radians() / 2.0).tan() / aspect).atan();
    let proj = glam::camera::rh::proj::directx::perspective(vfov, aspect, 0.1, 4096.0);
    proj * view
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let world = load_prl(&a[1]).expect("load");
    let text = std::fs::read_to_string(&a[2]).unwrap();
    let mut scratch = PortalWalkScratch::new();
    let mut rows: Vec<(u64, String)> = Vec::new();
    for line in text.lines().filter(|l| l.starts_with("pose ")) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let p = |i: usize| f[i].parse::<f32>().unwrap();
        let eye = Vec3::new(p(2), p(3), p(4));
        let (yaw, pitch) = (p(5).to_radians(), p(6).to_radians());
        let (vis, _) = determine_visible_cells(eye, camera(eye, yaw, pitch), &world, &[], false, &mut scratch, TimingGate::ON);
        let considered = vis.stats.cpu.value(VisibilityStage::Considered).unwrap_or(0) as u64;
        println!("pose {} considered={} reach={} path={:?}", f[1], considered, vis.fog_reachable.len(), vis.stats.path);
        rows.push((considered, f[1].to_string()));
    }
    let mut v: Vec<u64> = rows.iter().map(|r| r.0).collect();
    v.sort_unstable();
    let n = v.len();
    // nearest-rank percentiles
    let q = |x: f64| v[((n as f64 * x).ceil() as usize).clamp(1, n) - 1];
    let mean = v.iter().sum::<u64>() as f64 / n as f64;
    let worst = rows.iter().max_by_key(|r| r.0).unwrap();
    let over = v.iter().filter(|&&c| c > 5000).count();
    println!("summary poses {n} mean {mean:.1} p50 {} p95 {} p99 {} max {} worst {} over5000 {over} total {}",
        q(0.5), q(0.95), q(0.99), v[n - 1], worst.1, v.iter().sum::<u64>());
}
