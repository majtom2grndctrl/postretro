// Repeatable dev-tools probe-overlay launch settings for profiling.
// See: context/lib/rendering_pipeline.md §12

use super::{MarkerMode, ShDiagnosticsState};

impl ShDiagnosticsState {
    pub(in crate::render) fn from_environment() -> Self {
        let mut state = Self::default();
        if let Ok(value) = std::env::var("POSTRETRO_SH_PROBE_OVERLAY") {
            match parse_overlay(&value) {
                Some((mode, radius)) => {
                    state.show_markers = true;
                    state.marker_mode = mode;
                    state.cell_radius = radius;
                    log::info!("[SH diagnostics] launch overlay mode={mode:?} radius={radius}");
                }
                None => log::warn!(
                    "[SH diagnostics] invalid POSTRETRO_SH_PROBE_OVERLAY; expected mode,radius (0 < radius <= 64)"
                ),
            }
        }
        state
    }
}

fn parse_overlay(value: &str) -> Option<(MarkerMode, f32)> {
    let (mode, radius) = value.split_once(',')?;
    let mode = match mode {
        "irradiance" => MarkerMode::Irradiance,
        "residency" => MarkerMode::Residency,
        "validity" => MarkerMode::Validity,
        "density" => MarkerMode::DensityLevel,
        "uniform" => MarkerMode::Uniform,
        _ => return None,
    };
    let radius: f32 = radius.parse().ok()?;
    (radius.is_finite() && radius > 0.0 && radius <= 64.0).then_some((mode, radius))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_overlay_accepts_panel_modes_and_rejects_invalid_radius() {
        for (name, mode) in [
            ("irradiance", MarkerMode::Irradiance),
            ("residency", MarkerMode::Residency),
            ("validity", MarkerMode::Validity),
            ("density", MarkerMode::DensityLevel),
            ("uniform", MarkerMode::Uniform),
        ] {
            let (actual, radius) = parse_overlay(&format!("{name},30")).unwrap();
            assert_eq!(actual, mode);
            assert!((radius - 30.0).abs() < 1.0e-6);
        }
        for invalid in [
            "",
            "irradiance",
            "unknown,30",
            "irradiance,0",
            "irradiance,-1",
            "irradiance,65",
            "irradiance,NaN",
            "irradiance,inf",
        ] {
            assert!(parse_overlay(invalid).is_none(), "{invalid}");
        }
    }
}
