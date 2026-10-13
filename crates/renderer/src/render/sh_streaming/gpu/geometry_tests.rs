//! Geometry and deterministic long-generation fragmentation proofs.
use super::*;
use crate::render::sh_streaming::allocator::FirstFitRanges;

#[test]
fn whole_level_geometry_preserves_floor_share_and_covers_tiny_zero_and_full_layers() {
    let limits = wgpu::Limits::default();
    let old_share = AtlasShape::for_slots(40_000, 40_000, &limits).unwrap();
    assert_eq!(
        (old_share.tiles_per_row, old_share.tiles_per_layer),
        (200, 40_000)
    );
    let minimum = AtlasShape::for_slots(1, 100_000, &limits).unwrap();
    assert!(
        u64::from(minimum.tiles_per_layer) * u64::from(limits.max_texture_array_layers) >= 225_000
    );
    assert_eq!(minimum.layers, 1);
    assert_eq!(AtlasShape::for_slots(1, 0, &limits).unwrap().slots, 1);
    let full = (limits.max_texture_dimension_2d / 8).pow(2);
    let shape = AtlasShape::for_slots(full + 1, full + 1, &limits).unwrap();
    assert_eq!(shape.tiles_per_layer, full);
    assert_eq!(shape.slots, full * 2);
}

struct ChurnRng(u64);
impl ChurnRng {
    fn index(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound as u64) as usize
    }
    fn shuffle(&mut self, order: &mut [usize]) {
        for i in (1..order.len()).rev() {
            order.swap(i, self.index(i + 1));
        }
    }
}

// Frozen nonzero id-50 stored-tile writer closures from the named PRLs on
// 2026-10-13. Treating closures as independent variable sized allocations
// conservatively retains their halo tiles. These fixtures need no large bake.
const CAMPAIGN: &[u32] = &[
    14479, 4742, 978, 342, 1526, 324, 5081, 68, 5601, 330, 1002, 2455, 1873, 153, 143, 2825, 5524,
    8756, 574, 260, 260,
];
const WARREN_HALLWAY: &[u32] = &[
    7450, 12923, 6027, 7318, 2896, 6952, 481, 7475, 7514, 8805, 6710, 7261, 4067, 2884, 6942, 8296,
    7535, 11019, 11467, 9143, 2411, 6629, 3676, 2793, 3641, 1491, 180, 3419, 5976, 3148, 305, 5996,
    3021, 7177, 556, 391, 6857, 7312, 7194, 600, 7012, 7850, 8712, 7315, 20523, 3808, 10809, 353,
    8374, 8083, 4765, 812, 5466, 5080, 6605, 7003, 7545, 4913, 3737, 6978, 2535, 3070, 786, 5813,
    6386, 7090, 7989, 5452, 7671, 8648, 5632, 772, 8770, 7044, 8618, 616, 6983, 12324, 4596, 5650,
    5397, 3468, 8866, 8655, 3562, 6570, 2006, 626, 702, 615, 2851, 372, 2670, 388, 9793, 392, 697,
    407, 392, 407, 4054, 6640, 9744, 7107, 7546, 8377, 8091, 8813, 568, 7153, 2706, 4554, 3101,
    6912, 8930, 8735, 684, 7476, 6961, 3167, 4514, 3222, 4693, 12685, 7540, 8883, 253, 315, 1640,
    3248, 2014, 2553, 264, 7830, 342, 2382, 8856, 5546, 6763, 5720, 7132, 8146, 8912, 9140, 4554,
    7493, 6415, 7830, 3868, 10846, 5883, 5738, 7397, 14614, 8060, 3264, 5930, 8553, 3421, 2694,
    357, 2006, 323, 4063, 4855, 4740, 6796, 6969, 7107, 8578, 10558, 8980, 9006, 733, 8862, 9574,
    6613, 469, 13251, 10433, 7649, 8890, 3546, 336, 352, 336, 352, 4878, 336, 2354, 3118, 4068,
    3122,
];

fn churn_peak(sizes: &[u32], seed: u64) -> u32 {
    let mut rng = ChurnRng(seed);
    let mut pool = FirstFitRanges::default();
    let mut live = vec![None; sizes.len()];
    let mut order = (0..sizes.len()).collect::<Vec<_>>();
    let whole: u32 = sizes.iter().sum();
    let mut full_demand_steps = 0;
    for step in 0..=20_000 {
        if step % 257 == 0 || step == 20_000 {
            rng.shuffle(&mut order);
            for &i in &order {
                if live[i].is_none() {
                    live[i] = Some(pool.allocate(sizes[i]).unwrap());
                }
            }
            assert_eq!(live.iter().flatten().map(|r| r.len).sum::<u32>(), whole);
            full_demand_steps += 1;
        }
        if step != 20_000 {
            let i = rng.index(sizes.len());
            if let Some(range) = live[i].take() {
                pool.release(range).unwrap();
            } else {
                live[i] = Some(pool.allocate(sizes[i]).unwrap());
            }
        }
    }
    assert!(full_demand_steps >= 79);
    pool.capacity
}

#[test]
fn fixture_churn_fits_fragmentation_headroom() {
    let limits = wgpu::Limits::default();
    let seeds = [1, 7, 19, 43, 101, 313, 911, 2027];
    let factors = [(1, 1), (5, 4), (3, 2), (7, 4), (2, 1)];
    let mut refused = [false; 5];
    for (name, sizes) in [
        ("campaign-test", CAMPAIGN),
        ("stress-warren-hallway-inspection", WARREN_HALLWAY),
    ] {
        let whole = sizes.iter().sum();
        let chosen = AtlasShape::for_slots(1, whole, &limits).unwrap();
        let chosen_max = chosen.tiles_per_layer * limits.max_texture_array_layers;
        for seed in seeds {
            let peak = churn_peak(sizes, seed);
            assert!(
                peak <= chosen_max,
                "{name}, seed {seed}, peak {peak}, cap {chosen_max}"
            );
            for (i, (numerator, denominator)) in factors.into_iter().enumerate() {
                let shape =
                    AtlasShape::for_slots_with_headroom(1, whole, &limits, numerator, denominator)
                        .unwrap();
                refused[i] |= peak > shape.tiles_per_layer * limits.max_texture_array_layers;
            }
        }
    }
    assert_eq!(refused, [true, true, false, false, false]);
    assert_eq!(
        (
            DENSE_FRAGMENTATION_HEADROOM_NUMERATOR,
            DENSE_FRAGMENTATION_HEADROOM_DENOMINATOR
        ),
        (9, 4)
    );
    eprintln!(
        "[ShGrowthProof] Churn: 2 fixtures × 8 seeds × 20,000 steps; largest refused 1.25; smallest passing 1.5; chosen 2.25 (1.5 margin)"
    );
}
