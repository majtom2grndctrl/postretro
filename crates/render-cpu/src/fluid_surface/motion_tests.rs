// Whole-texel fluid motion against its invariants: the 3 Hz flash bound, the
// shimmer and ring amplitude and fade, drift scroll rate and wrap, and the
// declared wave's coherence, travel and wrap.
// See: context/lib/rendering_pipeline.md §7.3a

use super::*;

const FRAME: f32 = 1.0 / 60.0;
/// The general flash threshold every per-cell change stays under.
const FLASH_HZ: f32 = 3.0;

fn grid(origin: IVec2, size: i32) -> Vec<IVec2> {
    (0..size)
        .flat_map(|y| (0..size).map(move |x| origin + IVec2::new(x, y)))
        .collect()
}

/// Largest share of `cells` whose displayed offset changes between two
/// consecutive frames, over `frames` frames.
fn worst_frame_change_share(
    cells: &[IVec2],
    frames: usize,
    time_at: impl Fn(usize) -> f32,
    texels_per_pixel_at: impl Fn(usize) -> f32,
) -> f32 {
    let mut previous: Vec<IVec2> = cells
        .iter()
        .map(|&cell| displayed_offset(cell, time_at(0), texels_per_pixel_at(0)))
        .collect();
    let mut worst = 0usize;
    for frame in 1..frames {
        let mut changed = 0;
        for (cell, before) in cells.iter().zip(previous.iter_mut()) {
            let now = displayed_offset(*cell, time_at(frame), texels_per_pixel_at(frame));
            if now != *before {
                changed += 1;
                *before = now;
            }
        }
        worst = worst.max(changed);
    }
    worst as f32 / cells.len() as f32
}

// The wave counts texels, never texture periods, so a 64 px and a 128 px
// texture at one scale take the same offsets.
#[test]
fn cell_offsets_are_whole_texels_constant_within_a_cell_and_independent_of_texture_size() {
    let dev_wave = wave(63.0, 6.0, [48.0, 80.0]);
    let mut any_wave = false;
    for cell in grid(IVec2::new(-20, -20), 40) {
        for time in [0.0, 1.3, 7.9, 42.0, 86_000.3] {
            any_wave |= wave_offset(cell, time, dev_wave) != IVec2::ZERO;
            let corner = cell.as_vec2();
            let mut seen = None;
            for fraction in [Vec2::splat(0.01), Vec2::new(0.5, 0.25), Vec2::splat(0.99)] {
                let texel_uv = corner + fraction;
                for dims in [Vec2::splat(64.0), Vec2::splat(128.0)] {
                    let offset =
                        warped_texel_uv(texel_uv, Vec2::ZERO, dims, dev_wave, time, 0.1) - texel_uv;
                    let whole = offset.round();
                    assert!(
                        (offset - whole).abs().max_element() < 1.0e-4,
                        "offset {offset} is not whole texels"
                    );
                    assert_eq!(
                        *seen.get_or_insert(whole),
                        whole,
                        "offset varies inside {cell} or with texture size"
                    );
                }
            }
        }
    }
    assert!(any_wave, "the wave never moved a cell");
}

// At a fixed texels-per-pixel (the 3 Hz bound governs the animation clock, not
// camera-driven fade crossings). A ring centred on one cell spans neighbours
// stepping on other phases.
#[test]
fn displayed_offset_at_a_fixed_view_changes_only_on_the_cells_own_step_boundaries_under_3_hz() {
    const SAMPLE_HZ: f32 = 600.0;
    let cells = grid(IVec2::new(-40, -40), 80);
    for &cell in &cells {
        let mut previous_offset = displayed_offset(cell, 0.0, 0.1);
        let mut previous_step = cell_step(cell, 0.0);
        let mut last_change: Option<f32> = None;
        for sample in 1..(20.0 * SAMPLE_HZ) as usize {
            let time = sample as f32 / SAMPLE_HZ;
            let offset = displayed_offset(cell, time, 0.1);
            let step = cell_step(cell, time);
            if offset != previous_offset {
                assert_ne!(step, previous_step, "{cell} changed off its step boundary");
                if let Some(last) = last_change {
                    assert!(
                        time - last >= 1.0 / FLASH_HZ,
                        "{cell} changed {}s after its last change",
                        time - last
                    );
                }
                last_change = Some(time);
            }
            previous_offset = offset;
            previous_step = step;
        }
    }
}

// Cells at negative texel coordinates hash to their own phases.
#[test]
fn no_frame_changes_more_than_a_tenth_of_a_grid_including_negative_cells() {
    let phase = |cell: IVec2| hash_unit(cell_seed(cell));
    assert_ne!(phase(IVec2::new(-1, 0)), phase(IVec2::ZERO));
    assert_ne!(phase(IVec2::new(0, -1)), phase(IVec2::ZERO));

    for origin in [IVec2::ZERO, IVec2::splat(-32), IVec2::splat(-64)] {
        let cells = grid(origin, 64);
        let share = worst_frame_change_share(&cells, 600, |f| f as f32 * FRAME, |_| 0.1);
        assert!(
            share <= 0.10,
            "grid at {origin}: a frame changed {share} of cells"
        );
    }

    // A block of negative cells never steps on one frame.
    let negative = grid(IVec2::splat(-8), 8);
    for frame in 0..600 {
        let time = frame as f32 * FRAME;
        let stepping = negative
            .iter()
            .filter(|&&cell| cell_step(cell, time + FRAME) != cell_step(cell, time))
            .count();
        assert!(
            stepping < negative.len() / 2,
            "frame {frame}: {stepping} cells stepped"
        );
    }
}

#[test]
fn shimmer_averages_zero_and_stays_within_its_amplitude() {
    for cell in grid(IVec2::new(-16, -16), 32) {
        let mut sum = Vec2::ZERO;
        let frames = 600;
        for frame in 0..frames {
            let time = 3.7 + frame as f32 * FRAME;
            let offset = shimmer_offset(cell, cell_step(cell, time));
            assert!(offset.abs().max_element() <= FLUID_SHIMMER_AMPLITUDE);
            sum += offset.as_vec2();
        }
        let mean = sum / frames as f32;
        assert!(
            mean.abs().max_element() <= 0.05,
            "{cell} shimmer mean {mean}"
        );
    }
}

#[test]
fn displayed_offset_stays_within_the_combined_amplitude() {
    let combined = FLUID_SHIMMER_AMPLITUDE + FLUID_RING_AMPLITUDE;
    let mut any_ring = false;
    for cell in grid(IVec2::new(-48, -48), 96) {
        for step in 0..24 {
            let ring = ring_offset(cell, step);
            any_ring |= ring != IVec2::ZERO;
            let offset = shimmer_offset(cell, step) + ring;
            assert!(offset.abs().max_element() <= combined);
        }
    }
    assert!(any_ring, "the grid saw no ring at all");
}

#[test]
fn offset_reaches_zero_past_the_fade_threshold() {
    let past = FLUID_FADE_END_TEXELS_PER_PIXEL + 1.0e-3;
    for cell in grid(IVec2::new(-32, -32), 64) {
        for time in [0.0, 0.9, 5.1, 33.3] {
            assert_eq!(displayed_offset(cell, time, past), IVec2::ZERO);
        }
    }
    let before = FLUID_FADE_START_TEXELS_PER_PIXEL - 1.0e-3;
    assert!(
        grid(IVec2::ZERO, 64)
            .iter()
            .any(|&cell| displayed_offset(cell, 1.0, before) != IVec2::ZERO),
        "nothing moves before the fade band"
    );
}

#[test]
fn sweeping_the_fade_band_never_flattens_a_tenth_of_cells_on_one_frame() {
    let cells = grid(IVec2::new(-32, -32), 64);
    let start = FLUID_FADE_START_TEXELS_PER_PIXEL - 0.01;
    let end = FLUID_FADE_END_TEXELS_PER_PIXEL + 0.01;
    for direction in [1.0f32, -1.0] {
        let share = worst_frame_change_share(
            &cells,
            61,
            |f| 2.0 + f as f32 * FRAME,
            |f| {
                let t = f as f32 / 60.0;
                if direction > 0.0 {
                    start + (end - start) * t
                } else {
                    end - (end - start) * t
                }
            },
        );
        assert!(
            share <= 0.10,
            "fade sweep changed {share} of cells on one frame"
        );
    }
}

#[test]
fn rings_are_a_pure_function_that_moves_and_does_not_repeat_per_tile() {
    let cells = grid(IVec2::ZERO, 64);
    for step in 0..12 {
        for &cell in &cells {
            assert_eq!(ring_offset(cell, step), ring_offset(cell, step));
        }
        assert!(
            cells
                .iter()
                .any(|&cell| ring_offset(cell, step) != ring_offset(cell, step + 1)),
            "rings did not change between steps {step} and {}",
            step + 1
        );
    }
    let period = IVec2::new(64, 0);
    assert!(
        (0..12).any(|step| cells
            .iter()
            .any(|&cell| ring_offset(cell, step) != ring_offset(cell + period, step))),
        "rings repeat on every texture tile"
    );
}

#[test]
fn drift_scroll_moves_at_its_rate_in_whole_texels_and_reverses() {
    let dims = Vec2::new(64.0, 32.0);
    let rate = Vec2::new(0.75, -0.2);
    let travel = |rate: Vec2| {
        let mut total = Vec2::ZERO;
        let mut previous = drift_texels(rate, 0.0, dims);
        for frame in 1..=600 {
            let now = drift_texels(rate, frame as f32 * FRAME, dims);
            assert_eq!(now, now.floor());
            // Unwrap by the nearest whole period.
            let step = now - previous;
            total += step - (step / dims).round() * dims;
            previous = now;
        }
        total
    };
    let forward = travel(rate);
    let expected = rate * dims * 10.0;
    assert!(
        (forward - expected).abs().max_element() <= 1.0,
        "{forward} vs {expected}"
    );
    let backward = travel(-rate);
    assert!(
        (backward + expected).abs().max_element() <= 1.0,
        "{backward} vs -{expected}"
    );
}

// After 24 h of script time in f32 the scroll stays whole-texel, on speed
// and within one period of the unwarped UV.
#[test]
fn drift_scroll_after_a_day_of_script_time_keeps_whole_texels_and_true_speed() {
    let dims = Vec2::splat(64.0);
    // A scale-0.25 face: 1 / (0.0254 m × 0.25) texels per meter, at 3 m/s.
    let texels_per_second = 3.0 / (0.0254 * 0.25);
    let rate = Vec2::new(texels_per_second / dims.x, 0.0);
    let day = 86_400.0f64;
    let time_at = |frame: usize| (day + frame as f64 / 60.0) as f32;

    let mut total = 0.0f32;
    let mut previous = drift_texels(rate, time_at(0), dims);
    for frame in 1..=600 {
        let time = time_at(frame);
        let now = drift_texels(rate, time, dims);
        assert_eq!(now, now.floor());
        let step = now.x - previous.x;
        total += step - (step / dims.x).round() * dims.x;
        previous = now;

        let warped = warped_texel_uv(
            Vec2::new(10.25, 3.5),
            rate,
            dims,
            FluidWaveParams::NONE,
            time,
            0.1,
        );
        assert!((warped - Vec2::new(10.25, 3.5)).abs().max_element() < dims.x);
    }
    let elapsed = time_at(600) - time_at(0);
    let mean = total / elapsed;
    assert!(
        (mean - texels_per_second).abs() <= 1.0,
        "mean {mean} texels/s vs {texels_per_second}"
    );
}

// Shimmer and rings key on the fixed grid, so a scroll wrap leaves them.
#[test]
fn wrapping_the_drift_scroll_changes_no_cell_state() {
    let dims = Vec2::splat(64.0);
    let rate = Vec2::new(0.9, 0.0);
    let mut wrapped = false;
    let mut previous = drift_scroll_periods(rate, 0.0);
    for frame in 1..600 {
        let time = frame as f32 * FRAME;
        let scroll = drift_scroll_periods(rate, time);
        let wrap_frame = (scroll.x - previous.x).abs() > 0.5;
        wrapped |= wrap_frame;
        previous = scroll;
        for cell in grid(IVec2::new(-4, -4), 8) {
            let texel_uv = cell.as_vec2() + 0.5;
            let warped = warped_texel_uv(texel_uv, rate, dims, FluidWaveParams::NONE, time, 0.1);
            let cell_part = warped - texel_uv - drift_texels(rate, time, dims);
            assert_eq!(cell_part, displayed_offset(cell, time, 0.1).as_vec2());
        }
    }
    assert!(wrapped, "the scroll never wrapped");
}

#[test]
fn exact_fract_product_matches_f64_at_large_times() {
    for &(a, b) in &[
        (86_400.0f32, 7.38f32),
        (12_345.678, -3.21),
        (0.5, 0.25),
        (1.0e6, 0.001),
    ] {
        let exact = (f64::from(a) * f64::from(b)).rem_euclid(1.0) as f32;
        let got = exact_fract_product(a, b);
        let error = (got - exact).abs().min(1.0 - (got - exact).abs());
        assert!(error < 1.0e-5, "fract({a} × {b}) = {got}, expected {exact}");
    }
}

// A cell evaluates only its own block's drop, so a ring must never reach a
// block's edge cells: past them it would be cut off at the block border.
#[test]
fn rings_never_reach_their_blocks_edge_cells() {
    let size = 1 << FLUID_RING_BLOCK_SHIFT;
    let mut any_ring = false;
    for step in 0..(4 * FLUID_RING_LIFETIME_STEPS) {
        for cell in grid(IVec2::new(-64, -64), 128) {
            if ring_offset(cell, step) == IVec2::ZERO {
                continue;
            }
            any_ring = true;
            let local = cell - ((cell >> FLUID_RING_BLOCK_SHIFT) << FLUID_RING_BLOCK_SHIFT);
            assert!(
                local.min_element() > 0 && local.max_element() < size - 1,
                "{cell} rings on its block's edge at step {step}"
            );
        }
    }
    assert!(any_ring);
}

// --- Declared wave -------------------------------------------------------

/// The four coherence settings: each range minimum is exactly 8 × amplitude.
const COHERENCE_SETTINGS: [(f32, [f32; 2]); 4] = [
    (16.0, [128.0, 128.0]),
    (8.0, [64.0, 80.0]),
    (6.0, [48.0, 80.0]),
    (1.0, [8.0, 128.0]),
];
const DAY: f64 = 86_400.0;
const WAVE_SALTS: [u32; 2] = [FLUID_SALT_WAVE_U, FLUID_SALT_WAVE_V];

fn wave(period: f32, amplitude: f32, length: [f32; 2]) -> FluidWaveParams {
    FluidWaveParams {
        inv_period: 1.0 / period,
        amplitude,
        length_min: length[0],
        length_max: length[1],
    }
}

/// The wave's phase at `time`, split as `wave_offset` splits it once per cell.
fn phase_at(time: f32, wave: FluidWaveParams) -> (i32, f32) {
    exact_split_product(time, wave.inv_period)
}

fn mean_length(wave: FluidWaveParams) -> f32 {
    0.5 * (wave.length_min + wave.length_max)
}

/// Rounds as the twin does (`floor(x + 0.5)`, which WGSL and Rust agree on).
fn whole(x: f32) -> i32 {
    (x + 0.5).floor() as i32
}

/// A script time on the 1/128 s grid, where f32 holds every value to beyond a
/// day, so `time + period` is exact.
fn grid_time(seconds: f64) -> f32 {
    ((seconds * 128.0).round() / 128.0) as f32
}

/// The twin's half-arc from its own constants, in f64, on `x` in `[0, 1]`.
fn half_arc(x: f64) -> f64 {
    let g = x * (1.0 - x);
    g * (f64::from(FLUID_WAVE_ARC_LINEAR) + f64::from(FLUID_WAVE_ARC_QUADRATIC) * g)
}

/// Peak `|dh/dx|` of a unit half-arc, by central differences.
fn peak_slope(arc: impl Fn(f64) -> f64) -> f64 {
    const STEPS: usize = 200_000;
    let dx = 1.0 / STEPS as f64;
    (0..=STEPS)
        .map(|i| {
            let x = i as f64 * dx;
            let (lo, hi) = ((x - 0.5 * dx).max(0.0), (x + 0.5 * dx).min(1.0));
            ((arc(hi) - arc(lo)) / (hi - lo)).abs()
        })
        .fold(0.0, f64::max)
}

/// The arc's peak slope at the range minimum L = 8A, in texels per cell: a
/// half-arc spans L/2 cells and peaks at A, so `A · peak · 2 / L = peak / 4`.
fn slope_at_range_minimum() -> f32 {
    (peak_slope(half_arc) / 4.0) as f32
}

#[test]
fn the_wave_arc_peaks_under_0_8_texel_per_cell_at_8_times_amplitude() {
    assert_eq!(half_arc(0.0), 0.0);
    assert_eq!(half_arc(1.0), 0.0);
    assert!((half_arc(0.5) - 1.0).abs() < 1.0e-12);
    // Closed form: h' = (5 − 2y²)·y with y = 1 − 2x peaks at y² = 5/6.
    let closed = (5.0 - 2.0 * 5.0 / 6.0) * (5.0f64 / 6.0).sqrt();
    assert!((peak_slope(half_arc) - closed).abs() < 1.0e-3);
    let slope = slope_at_range_minimum();
    assert!(slope <= 0.8, "peak slope {slope} texel per cell at L = 8A");
    assert_eq!(slope.floor() as i32 + 1, 1, "the coherence bound");
    // A two-parabola arc peaks at exactly 1 texel per cell: the fold point.
    let parabola = peak_slope(|x| 4.0 * x * (1.0 - x)) / 4.0;
    assert!((parabola - 1.0).abs() < 1.0e-4, "parabola {parabola}");

    // The twin's arc is that shape: positive then negative, zero at each join.
    for length in [8.0f32, 48.0, 80.0, 128.0] {
        for i in 0..=64 {
            let x = i as f32 / 64.0;
            let first = wave_arc(Vec2::new(0.5 * length * x, length));
            assert!((f64::from(first) - half_arc(f64::from(x))).abs() < 1.0e-5);
            if x < 1.0 {
                let second = wave_arc(Vec2::new(0.5 * length * (1.0 + x), length));
                assert!((f64::from(second) + half_arc(f64::from(x))).abs() < 1.0e-5);
            }
        }
        for join in [0.0, 0.5 * length, length] {
            assert_eq!(wave_arc(Vec2::new(join, length)), 0.0);
        }
    }
}

// Coherence (brief Acceptance): the bound derives from the arc's peak slope at
// the range minimum, plus one texel for rounding. Cross-coupled, a horizontal
// neighbour shares the `u` shift and a vertical one the `v` shift, so each
// axis's line carries the grid's steps; the lines run much longer than the grid.
#[test]
fn coherence_keeps_adjacent_cells_wave_offsets_within_the_arc_slope_bound() {
    let slope = slope_at_range_minimum();
    let bound = slope.floor() as i32 + 1;
    let times: Vec<f32> = [
        0.0, 0.25, 3.7, 17.3, 59.9, 600.5, 3_600.125, 43_210.7, 86_399.9,
    ]
    .into_iter()
    .chain((0..24).map(|i| (i as f32 * 3_607.37) % 86_400.0))
    .collect();
    for (amplitude, length) in COHERENCE_SETTINGS {
        assert_eq!(length[0], 8.0 * amplitude);
        for period in [6.0, 63.0] {
            let w = wave(period, amplitude, length);
            let mut widest = 0;
            for &time in &times {
                for salt in WAVE_SALTS {
                    let line: Vec<f32> = (-1024..1024)
                        .map(|i| wave_value(i as f32 + 0.5, phase_at(time, w), w, salt))
                        .collect();
                    for pair in line.windows(2) {
                        assert!(
                            (pair[1] - pair[0]).abs() <= slope + 1.0e-4,
                            "A={amplitude} {length:?}: continuous step {} at t={time}",
                            pair[1] - pair[0]
                        );
                        let step = (whole(pair[1]) - whole(pair[0])).abs();
                        assert!(step <= bound, "A={amplitude} {length:?}: step {step}");
                        widest = widest.max(step);
                    }
                }
                for cell in grid(IVec2::new(-16, -16), 32) {
                    let here = wave_offset(cell, time, w);
                    for neighbour in [IVec2::X, IVec2::Y] {
                        let step = (wave_offset(cell + neighbour, time, w) - here)
                            .abs()
                            .max_element();
                        assert!(step <= bound, "{cell} at t={time}: step {step}");
                    }
                }
            }
            assert_eq!(widest, bound, "A={amplitude} {length:?} never stepped");
        }
    }
}

#[test]
fn the_wave_is_exactly_zero_without_amplitude_or_period() {
    let flat = [
        wave(63.0, 0.0, [64.0, 80.0]),
        FluidWaveParams {
            inv_period: 0.0,
            ..wave(63.0, 8.0, [64.0, 80.0])
        },
        FluidWaveParams::NONE,
    ];
    for w in flat {
        for time in [0.0, 5.5, 1_000.25, 86_399.0] {
            for cell in grid(IVec2::new(-40, -40), 80) {
                assert_eq!(wave_offset(cell, time, w), IVec2::ZERO);
                for salt in WAVE_SALTS {
                    assert_eq!(
                        wave_value(cell.x as f32 + 0.5, phase_at(time, w), w, salt),
                        0.0
                    );
                }
            }
        }
    }
}

// The phase splits exactly, so a whole period later the pattern is the same
// pattern one mean length further on, to a day of script time. t = 49 146.5
// at period 6 crosses the hash-repeat span's wrap (8192 mean lengths).
#[test]
fn one_period_later_the_wave_is_the_same_pattern_one_mean_length_on() {
    let settings = COHERENCE_SETTINGS.into_iter().chain([(4.0, [37.5, 90.25])]);
    for (amplitude, length) in settings {
        for period in [6.0f32, 20.0, 63.0] {
            let w = wave(period, amplitude, length);
            let mean = mean_length(w);
            for seconds in [
                0.0,
                1.5,
                63.0,
                1_000.25,
                49_146.5,
                43_200.7,
                DAY - f64::from(period) - 0.3,
            ] {
                let time = grid_time(seconds);
                let later = time + period;
                assert_eq!(f64::from(later), f64::from(time) + f64::from(period));
                for salt in WAVE_SALTS {
                    for i in 0..3_000 {
                        let position = -600.0 + i as f32 * 0.37;
                        let now = wave_value(position - mean, phase_at(time, w), w, salt);
                        let then = wave_value(position, phase_at(later, w), w, salt);
                        assert!(
                            (now - then).abs() <= 1.0e-3,
                            "A={amplitude} {length:?} period {period} t={time} at {position}: \
                             {then} vs {now}"
                        );
                    }
                }
            }
        }
    }
}

// Tracks crests by their travel over long runs, including the last minutes of
// a day: each keeps its length, sits where the travel puts it, ends where its
// neighbour starts, and is zero at each arc join.
#[test]
fn every_crest_travels_at_mean_length_per_period_and_keeps_its_length() {
    for (amplitude, length) in COHERENCE_SETTINGS {
        for period in [6.0f32, 63.0] {
            let w = wave(period, amplitude, length);
            let speed = f64::from(mean_length(w)) * f64::from(w.inv_period);
            for origin in [0.0, 43_200.25, 86_000.5] {
                let t0 = grid_time(origin);
                for salt in WAVE_SALTS {
                    for i in 0..17 {
                        let probe = -300.0 + i as f32 * 37.0;
                        let crest = wave_crest(probe, phase_at(t0, w), w, salt);
                        let crest_length = crest.y;
                        assert!(
                            (length[0]..=length[1]).contains(&crest_length),
                            "crest length {crest_length} outside {length:?}"
                        );
                        let start0 = f64::from(probe - crest.x);
                        for step in 1..=800 {
                            let time = grid_time(f64::from(t0) + step as f64 * 0.5);
                            let start = start0 + speed * (f64::from(time) - f64::from(t0));
                            let middle = (start + 0.5 * f64::from(crest_length)) as f32;
                            let tracked = wave_crest(middle, phase_at(time, w), w, salt);
                            assert!(
                                (tracked.y - crest_length).abs() <= 1.0e-3,
                                "crest {crest_length} became {} at t={time}",
                                tracked.y
                            );
                            assert!(
                                (tracked.x - 0.5 * crest_length).abs() <= 1.0e-2,
                                "crest at t={time} is {} into itself, expected {}",
                                tracked.x,
                                0.5 * crest_length
                            );
                            let end = (start + f64::from(crest_length)) as f32;
                            let next = wave_crest(end + 0.05, phase_at(time, w), w, salt);
                            assert!((next.x - 0.05).abs() <= 1.0e-2, "spacing changed");
                            assert!((length[0]..=length[1]).contains(&next.y));
                            for join in [start as f32, middle, end] {
                                assert!(
                                    wave_value(join, phase_at(time, w), w, salt).abs() <= 1.0e-2
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

// P10: every whole-period boundary to 24 h, frame by frame. At period 6 the run
// also crosses the hash-repeat span's wrap (t = 49 152 s).
#[test]
fn wave_wrap_across_every_period_boundary_to_a_day_never_jumps_or_rehashes_a_crest() {
    let cells = [
        IVec2::new(-1_001, 37),
        IVec2::new(-37, -5),
        IVec2::ZERO,
        IVec2::ONE,
        IVec2::new(17, -300),
        IVec2::new(63, 64),
        IVec2::new(200, 999),
        IVec2::new(513, -2),
    ];
    let span_count = |time: f32, w: FluidWaveParams| {
        exact_split_product(time, w.inv_period).0 & (2 * FLUID_WAVE_PAIRS - 1)
    };
    for (amplitude, length) in [(6.0, [48.0, 80.0]), (1.0, [8.0, 128.0])] {
        for period in [6.0f32, 63.0] {
            let w = wave(period, amplitude, length);
            let speed = f64::from(mean_length(w)) * f64::from(w.inv_period);
            let boundaries = (DAY / f64::from(period)).floor() as i64;
            let mut crossed_span_wrap = false;
            for k in 1..=boundaries {
                let boundary = k as f64 * f64::from(period);
                let frames: [f32; 5] =
                    std::array::from_fn(|j| (boundary + (j as f64 - 2.0) / 60.0) as f32);
                for cell in cells {
                    let mut previous = wave_offset(cell, frames[0], w);
                    for &time in &frames[1..] {
                        let now = wave_offset(cell, time, w);
                        assert!(
                            (now - previous).abs().max_element() <= 1,
                            "{cell} jumped {previous} -> {now} at t={time} (period {period})"
                        );
                        previous = now;
                    }
                }
                let (before, after) = (frames[1], frames[3]);
                crossed_span_wrap |= span_count(after, w) < span_count(before, w);
                let probe = ((k * 7_919) % 997) as f32 - 498.5;
                for salt in WAVE_SALTS {
                    let crest = wave_crest(probe, phase_at(before, w), w, salt);
                    let start =
                        f64::from(probe - crest.x) + speed * (f64::from(after) - f64::from(before));
                    let middle = (start + 0.5 * f64::from(crest.y)) as f32;
                    let tracked = wave_crest(middle, phase_at(after, w), w, salt);
                    assert!(
                        (tracked.y - crest.y).abs() <= 1.0e-3
                            && (tracked.x - 0.5 * crest.y).abs() <= 1.0e-2,
                        "crest {} became {tracked} across t={boundary} (period {period})",
                        crest.y
                    );
                }
            }
            assert_eq!(
                crossed_span_wrap,
                f64::from(period) * f64::from(2 * FLUID_WAVE_PAIRS) <= DAY,
                "period {period}: hash-span wrap crossing"
            );
        }
    }
}

#[test]
fn exact_split_product_count_and_fraction_agree_with_f64() {
    for &(a, b) in &[
        (86_400.0f32, 1.0f32 / 6.0),
        (86_400.0, 1.0 / 63.0),
        (49_152.0, 1.0 / 6.0),
        (12_345.678, -3.21),
        (0.5, 0.25),
        (1.0e6, 0.001),
        (-7.25, 0.4),
    ] {
        let exact = f64::from(a) * f64::from(b);
        let (count, fraction) = exact_split_product(a, b);
        assert!((0.0..1.0).contains(&fraction), "fraction {fraction}");
        let got = f64::from(count) + f64::from(fraction);
        assert!(
            (got - exact).abs() < 1.0e-5,
            "{a} × {b} = {got}, expected {exact}"
        );
    }
}

// The wave adds beside drift in the warped sample; the per-cell displayed
// offset the flash guards measure never sees it.
#[test]
fn the_wave_lands_in_the_warped_sample_but_not_in_the_displayed_offset() {
    let dims = Vec2::splat(64.0);
    let drift = Vec2::new(0.3, -0.1);
    let w = wave(6.0, 6.0, [48.0, 80.0]);
    let mut any_wave = false;
    for cell in grid(IVec2::new(-24, -24), 48) {
        for time in [0.5, 7.1, 300.25] {
            let texel_uv = cell.as_vec2() + 0.5;
            let still = warped_texel_uv(texel_uv, drift, dims, FluidWaveParams::NONE, time, 0.1);
            let waving = warped_texel_uv(texel_uv, drift, dims, w, time, 0.1);
            assert_eq!(waving - still, wave_offset(cell, time, w).as_vec2());
            assert_eq!(
                still - texel_uv - drift_texels(drift, time, dims),
                displayed_offset(cell, time, 0.1).as_vec2()
            );
            any_wave |= waving != still;
        }
    }
    assert!(any_wave, "the wave never moved a cell");
}

// P6: the wave keeps no state, so the first frame after a reload that changes
// the period is a fresh evaluation at the new period; nothing eases across.
#[test]
fn a_wave_period_change_takes_effect_as_a_fresh_evaluation_on_the_next_frame() {
    let old = wave(63.0, 8.0, [64.0, 80.0]);
    let new = wave(20.0, 8.0, [64.0, 80.0]);
    let reload_frame = 90_000usize;
    let time_at = |frame: usize| (frame as f64 / 60.0) as f32;
    let cells = grid(IVec2::new(-32, -32), 64);
    let shown = |frame: usize| {
        let params = if frame < reload_frame { old } else { new };
        cells
            .iter()
            .map(|&cell| wave_offset(cell, time_at(frame), params))
            .collect::<Vec<_>>()
    };
    let before = shown(reload_frame - 1);
    let first = shown(reload_frame);
    let fresh: Vec<IVec2> = cells
        .iter()
        .map(|&cell| wave_offset(cell, time_at(reload_frame), new))
        .collect();
    assert_eq!(first, fresh);
    assert_ne!(before, first, "the reload should jump the pattern once");
}
