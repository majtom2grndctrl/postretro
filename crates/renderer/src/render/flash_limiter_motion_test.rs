// Ordinary motion through the flash limiter: a textured scene panned, turned,
// bobbed and shaken — nothing flashing — must present as authored. Each run
// is compared frame by frame against a rig with the frame limiter bypassed.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

use super::*;

/// A deterministic hash in [0, 1).
fn hash(mut x: u32) -> f32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// A wrapping panorama of linear-luminance values: a bright band of sky over
/// blocky walls of varied brightness, a few bright lights, and a dark floor.
struct Panorama {
    w: u32,
    h: u32,
    lum: Vec<f32>,
}

impl Panorama {
    fn new(seed: u32, w: u32, h: u32, contrast: f32) -> Self {
        let mut lum = vec![0.0; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let v = y as f32 / h as f32;
                let base = if v < 0.25 {
                    // Sky.
                    0.55 + 0.1 * hash(seed ^ (x / 23))
                } else if v < 0.7 {
                    // Walls: blocks of varied width and brightness, textured.
                    let block = x / (8 + (hash(seed ^ (x / 32) * 7) * 20.0) as u32);
                    let wall = 0.03 + 0.35 * hash(seed.wrapping_add(block * 131));
                    let texel = hash(seed ^ (x / 3) ^ ((y / 3) << 12));
                    wall * (0.7 + 0.6 * texel)
                } else {
                    // Floor.
                    0.02 + 0.03 * hash(seed ^ (x / 4) ^ ((y / 4) << 12))
                };
                lum[(y * w + x) as usize] = base;
            }
        }
        // Lights: small bright windows in the walls.
        for k in 0..w / 40 {
            let cx = (hash(seed ^ (k * 977)) * w as f32) as u32;
            let cy = (h as f32 * (0.35 + 0.25 * hash(seed ^ (k * 541)))) as u32;
            for y in cy.saturating_sub(4)..(cy + 4).min(h) {
                for x in cx.saturating_sub(5)..cx + 5 {
                    lum[(y * w + x % w) as usize] = 1.0;
                }
            }
        }
        // Blend toward mid-gray for lower-contrast variants.
        for l in &mut lum {
            *l = 0.15 + (*l - 0.15) * contrast;
        }
        Self { w, h, lum }
    }

    /// Bilinear sample at a fractional panorama position, wrapping in x and
    /// clamping in y.
    fn sample(&self, x: f32, y: f32) -> f32 {
        let fx = x.floor();
        let fy = y.floor();
        let tx = x - fx;
        let ty = y - fy;
        let at = |xi: i64, yi: i64| {
            let xi = xi.rem_euclid(self.w as i64) as u32;
            let yi = yi.clamp(0, self.h as i64 - 1) as u32;
            self.lum[(yi * self.w + xi) as usize]
        };
        let (x0, y0) = (fx as i64, fy as i64);
        let top = at(x0, y0) * (1.0 - tx) + at(x0 + 1, y0) * tx;
        let bottom = at(x0, y0 + 1) * (1.0 - tx) + at(x0 + 1, y0 + 1) * tx;
        top * (1.0 - ty) + bottom * ty
    }

    /// The 160×90 view whose top-left sits at panorama (`ox`, `oy`).
    fn view(&self, ox: f32, oy: f32) -> Vec<u8> {
        let one = f32_to_f16_bits(1.0);
        let mut data = Vec::with_capacity((WIDTH * HEIGHT * 8) as usize);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let l = f32_to_f16_bits(self.sample(ox + x as f32, oy + y as f32));
                for half in [l, l, l, one] {
                    data.extend_from_slice(&half.to_le_bytes());
                }
            }
        }
        data
    }
}

/// How the view moves: panorama offset (x, y) in pixels at time `t`.
type Motion = fn(f32) -> (f32, f32);

/// Per-cell luminance difference between two presented frames.
fn cell_diffs(a: &Presented, b: &Presented) -> Vec<f32> {
    let cell = WIDTH / 16;
    (0..144)
        .map(|c| {
            let r = cells(c % 16, c / 16, 1, 1);
            debug_assert_eq!(r.w, cell);
            (a.luminance(r) - b.luminance(r)).abs()
        })
        .collect()
}

struct Altered {
    frames: usize,
    cell_frames: usize,
    max_cells: usize,
    worst: f32,
    first: Option<usize>,
}

/// Run `motion` over `panorama` for `seconds` at `fps` through a limited and a
/// bypassed rig, and tally the cells the limiter altered visibly.
fn altered(panorama: &Panorama, motion: Motion, fps: f32, seconds: f32) -> Option<Altered> {
    let mut limited = Rig::new()?;
    let mut bypassed = Rig::new()?;
    bypassed.pass.bypass_frame_limiter = true;
    let dt = 1.0 / fps;
    let mut tally = Altered {
        frames: 0,
        cell_frames: 0,
        max_cells: 0,
        worst: 0.0,
        first: None,
    };
    for n in 0..(seconds * fps).round() as usize {
        let (ox, oy) = motion(n as f32 * dt);
        let pixels = panorama.view(ox, oy);
        let a = limited.present(&Frame::scene(pixels.clone(), dt));
        let b = bypassed.present(&Frame::scene(pixels, dt));
        let diffs = cell_diffs(&a, &b);
        let visible = diffs.iter().filter(|d| **d > 0.004).count();
        tally.worst = diffs.iter().fold(tally.worst, |m, d| m.max(*d));
        if visible > 0 {
            tally.frames += 1;
            tally.first.get_or_insert(n);
        }
        tally.cell_frames += visible;
        tally.max_cells = tally.max_cells.max(visible);
    }
    Some(tally)
}

const MOTIONS: &[(&str, Motion)] = &[
    ("still", |_| (0.0, 20.0)),
    // A slow pan: a quarter screen a second.
    ("slow pan", |t| (40.0 * t, 20.0)),
    // A steady turn: one screen width a second.
    ("turn 1 screen/s", |t| (160.0 * t, 20.0)),
    // A fast turn: three screens a second.
    ("turn 3 screens/s", |t| (480.0 * t, 20.0)),
    // Looking left and right: ±half a screen at 1.5 Hz.
    ("look flick 1.5 Hz", |t| {
        (80.0 * (t * 1.5 * std::f32::consts::TAU).sin(), 20.0)
    }),
    // Walking: forward view bob ±2% of the height at 2 Hz, slow drift.
    ("walk bob", |t| {
        (
            10.0 * t,
            20.0 + 1.8 * (t * 2.0 * std::f32::consts::TAU).sin(),
        )
    }),
    // Screen shake: ±3 px jitter every frame.
    ("shake", |t| {
        let n = (t * 60.0).round() as u32;
        (
            6.0 * hash(n * 2) - 3.0,
            20.0 + 6.0 * hash(n * 2 + 1) - 3.0,
        )
    }),
    // Strafe-and-turn: a combat mix of both.
    ("combat mix", |t| {
        (
            200.0 * t + 60.0 * (t * 1.1 * std::f32::consts::TAU).sin(),
            20.0 + 1.8 * (t * 2.4 * std::f32::consts::TAU).sin(),
        )
    }),
];

/// Exploration: print how much each ordinary motion is altered.
///
/// Run: `cargo test -p postretro-renderer --lib ordinary_motion_report --
/// --ignored --nocapture`
#[test]
#[ignore = "report, not a gate; run with --ignored --nocapture"]
fn ordinary_motion_report() {
    let fps = 60.0;
    for contrast in [1.0, 0.6] {
        let panorama = Panorama::new(7, 1280, 140, contrast);
        for (name, motion) in MOTIONS {
            let Some(a) = altered(&panorama, *motion, fps, 4.0) else {
                eprintln!("no adapter, skipping (not a pass)");
                return;
            };
            println!(
                "contrast {contrast:.1} {name:>18}: {:>3}/240 frames altered, {:>4} cell-frames, \
                 max {:>3} cells in a frame, worst Δlum {:.3}, first at frame {:?}",
                a.frames, a.cell_frames, a.max_cells, a.worst, a.first
            );
        }
    }
}
