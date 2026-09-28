// Player volume taper: stored linear [0, 1] → decibels at the audio seam.
// See: context/lib/audio.md §1 (Mixer bus tree)

use kira::Decibels;

/// Map a stored linear volume to decibels on the perceptual taper
/// 40·log₁₀ v (gain v²): 1.0 is unity, 0.5 is about −12 dB, and 0 is kira
/// silence. Out-of-range or non-finite values clamp into `[0, 1]`.
pub fn linear_volume_to_decibels(volume: f32) -> f32 {
    let volume = if volume.is_finite() {
        volume.clamp(0.0, 1.0)
    } else {
        1.0
    };
    if volume <= 0.0 {
        return Decibels::SILENCE.0;
    }
    (40.0 * volume.log10()).max(Decibels::SILENCE.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_taper_maps_unity_half_and_zero() {
        assert_eq!(linear_volume_to_decibels(1.0), 0.0);
        assert!((linear_volume_to_decibels(0.5) - (-12.0)).abs() <= 0.1);
        assert_eq!(linear_volume_to_decibels(0.0), Decibels::SILENCE.0);
        assert_eq!(Decibels(linear_volume_to_decibels(0.0)).as_amplitude(), 0.0);
        assert_eq!(linear_volume_to_decibels(f32::NAN), 0.0);
    }
}
