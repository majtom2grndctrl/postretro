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

#[test]
fn cell_offsets_are_whole_texels_and_constant_within_a_cell() {
    let dims = Vec2::splat(64.0);
    for cell in grid(IVec2::new(-20, -20), 40) {
        for time in [0.0, 1.3, 7.9, 42.0] {
            let corner = cell.as_vec2();
            let mut seen = None;
            for fraction in [Vec2::splat(0.01), Vec2::new(0.5, 0.25), Vec2::splat(0.99)] {
                let texel_uv = corner + fraction;
                let offset = warped_texel_uv(texel_uv, Vec2::ZERO, dims, time, 0.1) - texel_uv;
                let whole = offset.round();
                assert!(
                    (offset - whole).abs().max_element() < 1.0e-4,
                    "offset {offset} is not whole texels"
                );
                assert_eq!(
                    *seen.get_or_insert(whole),
                    whole,
                    "offset varies inside {cell}"
                );
            }
        }
    }
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

        let warped = warped_texel_uv(Vec2::new(10.25, 3.5), rate, dims, time, 0.1);
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
            let warped = warped_texel_uv(texel_uv, rate, dims, time, 0.1);
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
