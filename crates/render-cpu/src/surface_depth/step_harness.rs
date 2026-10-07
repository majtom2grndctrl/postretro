//! Step-count harness: what the signed march costs, in DDA steps, against the
//! carve-only march it replaced.
//!
//! This Mac has no GPU timing, so cost is measured as texel boundaries the
//! view ray crosses — which the CPU reference reports deterministically and
//! the WGSL mirror walks one for one.
//!
//! The sweep is fixed and libm-free (integer azimuths normalized with `sqrt`,
//! which is correctly rounded everywhere), so the numbers are exact on every
//! platform: 64 start positions × 11 view slopes × 16 azimuths per map.
//!
//! BASELINE numbers were measured on the UNMODIFIED carve-only march, at the
//! commit before the signed rewrite, over today's encoding of the same maps:
//! stored `G = 255 − h` read as depth below the plane, quantized
//! `floor(g·L) / L`, with the pre-doubling step caps (Concrete 24, Default 16).
//! That march no longer exists, so its numbers are pinned here as constants.
//! A ray counted as starved when the same march with an unlimited budget
//! needed more crossings than the capped one could make.
//!
//! CURRENT numbers are pinned too, as ceilings: a change that raises any of
//! them fails this test. Lower them when a change improves them.

use super::*;
use postretro_render_data::material::Material;

/// Texel size, in meters. Depth is authored in texels, so the texel rate
/// cancels out of the march's horizontal travel; this only has to keep the
/// deepest scale under `SURFACE_DEPTH_MAX_METERS`.
const TEXEL_M: f32 = 0.01;
/// View-ray slopes: `tan θ` off the surface normal, up to ~83°.
const TAN_THETA: [f32; 11] = [0.0, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0];
const AZIMUTHS: [(i32, i32); 16] = [
    (1, 0),
    (3, 1),
    (1, 1),
    (1, 3),
    (0, 1),
    (-1, 3),
    (-1, 1),
    (-3, 1),
    (-1, 0),
    (-3, -1),
    (-1, -1),
    (-1, -3),
    (0, -1),
    (1, -3),
    (1, -1),
    (3, -1),
];
/// The eye's height above the plane: a standing eye, far above any band, so
/// the eye bound never engages and the sweep marches from the peak exactly as
/// it did before the bound existed.
const EYE_HEIGHT_M: f32 = 1.0;
const STARTS: usize = 64;
const RAYS: u64 = (STARTS * TAN_THETA.len() * AZIMUTHS.len()) as u64;

/// One measurement over the sweep. `sum` / [`RAYS`] is the mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Steps {
    sum: u64,
    p99: u32,
    starved: u64,
}

impl Steps {
    fn mean(self) -> f64 {
        self.sum as f64 / RAYS as f64
    }
    fn starve_rate(self) -> f64 {
        self.starved as f64 / RAYS as f64
    }
}

/// Which cost target the contract sets for a map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeanTarget {
    /// At or below baseline: carve-only and flat maps must not pay for raise.
    AtBaseline,
    /// Within 1.25× of baseline despite the doubled span: the real assets.
    WithinQuarter,
    /// No mean target: the synthetic ±full-range map shows the doubled span's
    /// raw cost.
    None,
}

struct Map {
    name: &'static str,
    width: i32,
    height: i32,
    /// Authored bytes under the signed meaning.
    authored: Vec<u8>,
    material: Material,
    baseline: Steps,
    /// Ceiling: the numbers this march measured when it landed.
    current: Steps,
    mean_target: MeanTarget,
    /// Set when the mean target is known to be out of reach of the cost
    /// levers, with the reason. Escalated to the owner, not loosened.
    mean_target_missed: Option<&'static str>,
}

/// Vent-001's relief really is taller now. Its authored range (51..153) used
/// to quantize at 3 levels onto two sink plateaus, 1 and 2 texels down, below
/// an empty top third the ray still had to descend through. Read signed, the
/// same pixels span four plateaus from +1 to −2 texels — the peak 0.195 rounds
/// up to 1/3 — so the walked span grows from 2/3 to the full depth (1.5×).
/// Starting at the peak recovers the empty third; no lever can shorten
/// a band the content actually fills.
const VENT_MISS: &str = "the signed reading makes the panel's relief 1.5x taller";

/// Identical geometry costs identical steps: at the old 24-step cap this map
/// measures the baseline exactly (33715 / p99 23 / 130 starved). The whole
/// excess is the doubled cap letting 90 formerly-starved grazing rays finish.
const CARVE_ONLY_MISS: &str = "the doubled step cap resolves rays that used to starve";

fn xorshift(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

fn starts(width: i32, height: i32) -> Vec<[f32; 2]> {
    let mut state = 0x9E37_79B9u32;
    (0..STARTS)
        .map(|_| {
            let a = xorshift(&mut state);
            let b = xorshift(&mut state);
            let fx = (a & 0xFFFF) as f32 / 65536.0;
            let fy = (b & 0xFFFF) as f32 / 65536.0;
            let tx = (a >> 16) as i32 % width;
            let ty = (b >> 16) as i32 % height;
            [
                (tx as f32 + fx) / width as f32,
                (ty as f32 + fy) / height as f32,
            ]
        })
        .collect()
}

/// The R channel of a `_h.png`, exactly as the bake reads it.
fn decode(relative: &str) -> (i32, i32, Vec<u8>) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/textures")
        .join(relative);
    let image = image::open(&path)
        .unwrap_or_else(|e| panic!("decode {}: {e}", path.display()))
        .to_rgba8();
    let (w, h) = image.dimensions();
    let red = image
        .into_raw()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| c[0])
        .collect();
    (w as i32, h as i32, red)
}

fn measure(map: &Map) -> Steps {
    let tuning = map.material.surface_depth();
    let stored_bytes: Vec<u8> = map.authored.iter().map(|&h| 255 - h).collect();
    let stored_g: Vec<f32> = stored_bytes.iter().map(|&g| f32::from(g) / 255.0).collect();
    let field = SurfaceDepthField {
        width: map.width,
        height: map.height,
        stored_g: &stored_g,
        quantize_levels: tuning.quantize_levels,
    };
    // Base level only: the bake's mip chain is not reproducible here. A
    // filter overshoot in a coarser mip would widen the runtime band.
    let rg: Vec<u8> = stored_bytes.iter().flat_map(|&g| [0u8, g]).collect();
    let relief = surface_relief_from_rg8_levels(&[(map.width as u32, map.height as u32, &rg)])
        .at(0)
        .quantized(tuning.quantize_levels as f32);
    let dims = [map.width as f32, map.height as f32];
    let scale = tuning.depth_meters * TEXEL_M;

    let mut steps = Vec::with_capacity(RAYS as usize);
    let mut starved = 0u64;
    for uv0 in starts(map.width, map.height) {
        for &tan in &TAN_THETA {
            for &(ax, ay) in &AZIMUTHS {
                let len = ((ax * ax + ay * ay) as f32).sqrt();
                let per_m = tan / TEXEL_M;
                let dir = [
                    ax as f32 / len * per_m / dims[0],
                    ay as f32 / len * per_m / dims[1],
                ];
                let hit = march_surface_depth(
                    &field,
                    uv0,
                    dir,
                    scale,
                    relief,
                    EYE_HEIGHT_M,
                    tuning.max_steps,
                );
                steps.push(hit.steps);
                starved += u64::from(hit.starved);
            }
        }
    }
    steps.sort_unstable();
    let index = ((steps.len() as f64) * 0.99).ceil() as usize - 1;
    Steps {
        sum: steps.iter().map(|&s| u64::from(s)).sum(),
        p99: steps[index],
        starved,
    }
}

fn steps(sum: u64, p99: u32, starved: u64) -> Steps {
    Steps { sum, p99, starved }
}

fn maps() -> Vec<Map> {
    let mut maps = Vec::new();
    // The three shipped assets keep their pixels and take the signed meaning.
    for (name, relative, material, baseline, current, mean_target_missed) in [
        (
            "concrete_stone_030",
            "50-free-textures/concrete_stone_030_h.png",
            Material::Concrete,
            steps(17424, 20, 68),
            steps(17955, 24, 8),
            None,
        ),
        (
            "Vent-001_Base-001",
            "Level Eleven Games Sci-Fi Texture Pack v1/Vent-001_Base-001_h.png",
            Material::Default,
            steps(38647, 14, 80),
            steps(54834, 22, 0),
            Some(VENT_MISS),
        ),
        (
            "CorrugatedMetalPanel-01V_64",
            "Level Eleven Games Sci-Fi Texture Pack v1/CorrugatedMetalPanel-01V_64_h.png",
            Material::Default,
            steps(53463, 15, 570),
            steps(21180, 10, 0),
            None,
        ),
    ] {
        let (width, height, authored) = decode(relative);
        maps.push(Map {
            name,
            width,
            height,
            authored,
            material,
            baseline,
            current,
            mean_target: MeanTarget::WithinQuarter,
            mean_target_missed,
        });
    }

    // Synthetic carve-only: 8×8 stones on full-depth mortar, stone tops on
    // plateaus 0..=2 of 6 below the plane. The baseline encoded the SAME
    // geometry in the old meaning (white = plane), so it compares like with
    // like rather than re-reading these bytes as a deeper carve.
    let n = 64i32;
    let plateau = |x: i32, y: i32| -> u32 {
        if x % 8 == 0 || y % 8 == 0 {
            6
        } else {
            let mut state = ((x / 8) * 131 + (y / 8) * 17 + 1) as u32;
            xorshift(&mut state) % 3
        }
    };
    let carve: Vec<u8> = (0..n * n)
        .map(|i| {
            let k = plateau(i % n, i / n) as f32;
            (128.0 - (128.0 * k / 6.0 + 0.5).floor()) as u8
        })
        .collect();
    maps.push(Map {
        name: "synthetic carve-only",
        width: n,
        height: n,
        authored: carve,
        material: Material::Concrete,
        baseline: steps(33715, 23, 130),
        current: steps(35255, 24, 40),
        mean_target: MeanTarget::AtBaseline,
        mean_target_missed: Some(CARVE_ONLY_MISS),
    });
    // Synthetic flat: all mid-gray now, all white (the old plane) at baseline.
    maps.push(Map {
        name: "synthetic all-mid-gray",
        width: n,
        height: n,
        authored: vec![128; (n * n) as usize],
        material: Material::Concrete,
        baseline: steps(0, 0, 0),
        current: steps(0, 0, 0),
        mean_target: MeanTarget::AtBaseline,
        mean_target_missed: None,
    });
    // Synthetic ±full-range: uniform noise over every byte, same pixels in
    // both eras. Its baseline is a carve of span D; now the span is 2D.
    let mut state = 0x1234_5678u32;
    let full: Vec<u8> = (0..n * n)
        .map(|_| (xorshift(&mut state) & 0xFF) as u8)
        .collect();
    maps.push(Map {
        name: "synthetic full-range",
        width: n,
        height: n,
        authored: full,
        material: Material::Concrete,
        baseline: steps(26320, 12, 0),
        current: steps(63060, 23, 0),
        mean_target: MeanTarget::None,
        mean_target_missed: None,
    });
    maps
}

impl MeanTarget {
    fn met(self, got: Steps, baseline: Steps) -> bool {
        match self {
            Self::AtBaseline => got.sum <= baseline.sum,
            Self::WithinQuarter => got.sum * 4 <= baseline.sum * 5,
            Self::None => true,
        }
    }
}

#[test]
fn step_counts_stay_within_their_pinned_ceilings() {
    for map in maps() {
        let got = measure(&map);
        println!(
            "{}: baseline mean {:.4} p99 {} starve {:.4} | now mean {:.4} p99 {} starve {:.4} \
             | mean ratio {:.3}",
            map.name,
            map.baseline.mean(),
            map.baseline.p99,
            map.baseline.starve_rate(),
            got.mean(),
            got.p99,
            got.starve_rate(),
            got.mean() / map.baseline.mean().max(f64::MIN_POSITIVE),
        );

        // Regression ceilings: any increase fails.
        assert!(
            got.sum <= map.current.sum,
            "{}: total steps {} rose past the pinned {}",
            map.name,
            got.sum,
            map.current.sum,
        );
        assert!(
            got.p99 <= map.current.p99,
            "{}: p99 {} rose past the pinned {}",
            map.name,
            got.p99,
            map.current.p99,
        );
        assert!(
            got.starved <= map.current.starved,
            "{}: {} starved rays, pinned {}",
            map.name,
            got.starved,
            map.current.starved,
        );

        // Targets. The starve rate may never exceed the baseline.
        assert!(
            got.starved <= map.baseline.starved,
            "{}: starve rate {:.4} is above the baseline {:.4}",
            map.name,
            got.starve_rate(),
            map.baseline.starve_rate(),
        );
        let met = map.mean_target.met(got, map.baseline);
        match map.mean_target_missed {
            None => assert!(
                met,
                "{}: mean {:.4} misses its {:?} target against baseline {:.4}",
                map.name,
                got.mean(),
                map.mean_target,
                map.baseline.mean(),
            ),
            Some(reason) if met => println!(
                "{}: the recorded miss ({reason}) is now met — clear it",
                map.name
            ),
            Some(_) => {}
        }
    }
}
