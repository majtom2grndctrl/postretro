// Area-light shadow lattice: stratified points over [0,1)² that the solid-angle sampler maps onto
// the polygons, turned per texel by its seed, and the probe subset the centre sampler traces first.
// See: context/lib/build_pipeline.md §Compiler pipeline (Area lights)

use glam::DVec2;

/// Golden-ratio conjugate: the second axis's step on the rank-1 lattice.
const LATTICE_STEP: f64 = 0.618_033_988_749_894_9;

/// 2⁻³²: maps a `u32` onto [0, 1).
const U32_UNIT: f64 = 1.0 / 4_294_967_296.0;

/// Where the probe subset's points sit on the unshifted lattice: one per
/// quadrant, diagonals first, so any two probes lie apart on both axes.
const PROBE_TARGETS: [[f64; 2]; 4] = [[0.25, 0.25], [0.75, 0.75], [0.25, 0.75], [0.75, 0.25]];

/// Point `i` of a `count`-point lattice over [0,1)², shifted on the torus by
/// `seed`. The first axis holds one point per stratum `[i / count, (i + 1) /
/// count)`, which the sampler spends on picking a polygon and a fan triangle
/// by solid angle, so every polygon takes its share; the second steps by the
/// golden ratio. A toroidal shift keeps both properties, so adjacent texels
/// decorrelate without losing the stratification.
///
/// The shift reads only the seed's low half: the receiver footprint turns by
/// its high half (`texel_footprint::footprint_rotation`), so the two vary
/// independently, as the sphere lattice's do.
pub(crate) fn area_lattice_point(i: u32, count: u32, seed: u64) -> DVec2 {
    let (shift_x, shift_y) = lattice_shift(seed);
    let centre = f64::from(i) + 0.5;
    let x = centre / f64::from(count.max(1));
    let y = (centre * LATTICE_STEP).fract();
    DVec2::new((x + shift_x).fract(), (y + shift_y).fract())
}

/// The toroidal shift `seed` applies to the whole lattice.
fn lattice_shift(seed: u64) -> (f64, f64) {
    let mixed = (seed & 0xFFFF_FFFF).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    (
        f64::from((mixed >> 32) as u32) * U32_UNIT,
        f64::from(mixed as u32) * U32_UNIT,
    )
}

/// The four lattice indices the centre sampler traces before escalating: the
/// unshifted points nearest one target per quadrant, measured on the torus,
/// each distinct. A seed shifts every point alike, so their spread holds at
/// every texel. They depend on the count alone; at a count of four they are
/// the whole lattice. Area lights never read or write the point/spot probe
/// memo (`lightmap_bake::SoftProbes::new`).
pub(crate) fn area_probe_indices(count: u32) -> [u32; 4] {
    let mut out = [0, 1, 2, 3];
    if count <= 4 {
        return out;
    }
    let unshifted = |i: u32| {
        let centre = f64::from(i) + 0.5;
        [centre / f64::from(count), (centre * LATTICE_STEP).fract()]
    };
    let torus = |a: f64, b: f64| {
        let d = (a - b).abs();
        d.min(1.0 - d)
    };
    for (k, target) in PROBE_TARGETS.iter().enumerate() {
        let mut best = (f64::INFINITY, 0);
        for i in 0..count {
            if out[..k].contains(&i) {
                continue;
            }
            let [x, y] = unshifted(i);
            let dx = torus(x, target[0]);
            let dy = torus(y, target[1]);
            let distance = dx * dx + dy * dy;
            // Strict `<`: ties keep the lower index.
            if distance < best.0 {
                best = (distance, i);
            }
        }
        out[k] = best.1;
    }
    out
}
