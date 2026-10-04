// Browsable display choices and nominal labels; exact driver tuples stay intact.
// See: context/lib/player_options.md §7

use crate::options::DisplayMode;

const COMMON_REFRESH_HZ: &[u32] = &[
    24, 25, 30, 48, 50, 60, 72, 75, 85, 90, 100, 120, 144, 160, 165, 170, 175, 180, 200, 240, 250,
    280, 300, 360, 480, 500, 540, 600,
];

pub(super) fn nominal_refresh(rate: u32) -> Option<u32> {
    COMMON_REFRESH_HZ
        .iter()
        .copied()
        .filter(|hz| rate.abs_diff(hz * 1000) <= 1000)
        .min_by_key(|hz| (rate.abs_diff(hz * 1000), std::cmp::Reverse(*hz)))
}

pub(super) fn display_refresh(mode: &DisplayMode) -> f32 {
    nominal_refresh(mode.refresh_millihertz)
        .map_or(mode.refresh_millihertz as f32 / 1000.0, |hz| hz as f32)
}

fn same_aspect(mode: &DisplayMode, size: [u32; 2]) -> bool {
    if mode.width == 0 || mode.height == 0 || size.contains(&0) {
        return false;
    }
    let left = u128::from(mode.width) * u128::from(size[1]);
    let right = u128::from(mode.height) * u128::from(size[0]);
    // Pixel rounding (for example 1366x768) must not hide the 16:9 modes.
    left.abs_diff(right) * 1000 <= right
}

pub(super) fn same_choice(a: &DisplayMode, b: &DisplayMode) -> bool {
    a.width == b.width
        && a.height == b.height
        && a.bit_depth == b.bit_depth
        && a.monitor == b.monitor
        && match (
            nominal_refresh(a.refresh_millihertz),
            nominal_refresh(b.refresh_millihertz),
        ) {
            (Some(a), Some(b)) => a == b,
            _ => a.refresh_millihertz == b.refresh_millihertz,
        }
}

pub(super) fn choices(available: &[DisplayMode], size: [u32; 2]) -> Vec<DisplayMode> {
    let mut choices: Vec<_> = available
        .iter()
        .filter(|mode| same_aspect(mode, size))
        .filter_map(|mode| nominal_refresh(mode.refresh_millihertz).map(|hz| (hz, mode.clone())))
        .collect();
    choices.sort_by(|(a_hz, a), (b_hz, b)| {
        (a.width, a.height, a_hz, a.bit_depth, &a.monitor)
            .cmp(&(b.width, b.height, b_hz, b.bit_depth, &b.monitor))
            .then_with(|| {
                a.refresh_millihertz
                    .abs_diff(a_hz * 1000)
                    .cmp(&b.refresh_millihertz.abs_diff(b_hz * 1000))
            })
            .then_with(|| a.refresh_millihertz.cmp(&b.refresh_millihertz))
    });
    choices.dedup_by(|(_, a), (_, b)| same_choice(a, b));
    choices.into_iter().map(|(_, mode)| mode).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_filters_aspect_and_nominal_rates_without_rewriting_driver_modes() {
        let mode = |width, height, refresh_millihertz, bit_depth| DisplayMode {
            width,
            height,
            refresh_millihertz,
            bit_depth,
            monitor: "current".into(),
        };
        let available = vec![
            mode(1280, 720, 59_000, 32),
            mode(1280, 720, 59_940, 32),
            mode(1280, 720, 60_000, 32),
            mode(1280, 720, 60_000, 24),
            mode(1920, 1080, 119_000, 32),
            mode(1920, 1080, 119_000, 32),
            mode(1920, 1080, 144_000, 32),
            mode(1920, 1080, 65_000, 32),
            mode(1280, 1024, 60_000, 32),
        ];
        let picked = choices(&available, [1920, 1080]);
        assert_eq!(
            picked,
            vec![
                mode(1280, 720, 60_000, 24),
                mode(1280, 720, 60_000, 32),
                mode(1920, 1080, 119_000, 32),
                mode(1920, 1080, 144_000, 32),
            ]
        );
        assert!(same_choice(&available[0], &available[2]));
        assert_eq!(choices(&available, [1366, 768]), picked);
        assert!(choices(&available, [0, 1080]).is_empty());
        assert_eq!(
            choices(&available, [1280, 1024]),
            vec![mode(1280, 1024, 60_000, 32)]
        );
        let portrait = mode(720, 1280, 75_000, 32);
        assert_eq!(choices(&[portrait.clone()], [1080, 1920]), vec![portrait]);
        let wide = mode(1720, 720, 165_000, 32);
        assert_eq!(choices(&[wide.clone()], [3440, 1440]), vec![wide]);
        for (raw, nominal) in [
            (29_000, Some(30)),
            (59_000, Some(60)),
            (59_940, Some(60)),
            (119_000, Some(120)),
            (119_880, Some(120)),
            (143_855, Some(144)),
            (164_990, Some(165)),
            (239_760, Some(240)),
            (58_999, None),
            (118_999, None),
            (61_001, None),
            (0, None),
        ] {
            assert_eq!(nominal_refresh(raw), nominal, "driver rate {raw}");
        }
    }
}
