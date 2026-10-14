// Mod-manifest fluid declarations and the declaration-ordered fluid registry.
// See: context/lib/scripting.md §2 (Mod-init context lifecycle)

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::data_descriptors::{DescriptorError, validate_sound_key};

/// One `defineFluid` declaration: the substance a fluid volume names. The
/// volume owns the place; this owns what it is like to be in it. `viscosity`
/// is movement data (drag while swimming, 1/s); the rest is presentation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FluidDescriptor {
    /// Name a map's `fluid_volume` refers to through its `fluid` key.
    pub name: String,
    /// Drag while swimming, 1/s. Finite and ≥ 0.
    pub viscosity: f32,
    /// Full-screen tint while the eye is inside: rgb, then strength. Each in
    /// `[0, 1]`.
    pub view_tint: [f32; 4],
    /// World-sound low-pass while the eye is inside: 0 none, 1 maximum.
    pub muffle: f32,
    /// Alpha of the fluid's surface faces, `[0, 1]`.
    pub surface_opacity: f32,
    /// Sound key played when a swim starts in this fluid.
    pub enter_sound: Option<String>,
    /// Sound key played when a swim in this fluid ends.
    pub exit_sound: Option<String>,
    /// Metres of view path into the fluid, seen from outside, that hide 95% of
    /// what lies beyond. `None`: clear. Finite and > 0.
    pub fog_distance: Option<f32>,
    /// Murk rgb in `view_tint`'s space, each in `[0, 1]`. `None`: the murk is
    /// `view_tint`'s rgb; see [`Self::resolved_fog_color`].
    pub fog_color: Option<[f32; 3]>,
    /// Seconds per cycle of an average-length wave crest. `None`: no wave.
    /// Finite and ≥ [`Self::MIN_WAVE_PERIOD`].
    pub wave_period: Option<f32>,
    /// Peak sideways shift of the surface wave, in texture texels; 0 is no
    /// wave. Resolved: [`Self::DEFAULT_WAVE_AMPLITUDE`] when omitted.
    pub wave_amplitude: f32,
    /// Crest-to-crest length range `[min, max]`, in texture texels. Resolved:
    /// [`Self::default_wave_length`] of the amplitude when omitted.
    pub wave_length: [f32; 2],
}

/// The first out-of-range field on a fluid declaration, by its authored key.
#[derive(Debug, Clone, PartialEq)]
pub struct FluidFieldError {
    /// Authored (camelCase) field name.
    pub field: &'static str,
    /// What the field must be.
    pub requirement: &'static str,
    /// The rejected value, formatted for the diagnostic.
    pub got: String,
}

impl fmt::Display for FluidFieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}, got {}", self.field, self.requirement, self.got)
    }
}

impl FluidDescriptor {
    /// `waveAmplitude` when omitted, in texels.
    pub const DEFAULT_WAVE_AMPLITUDE: f32 = 8.0;
    /// Shortest `wavePeriod`, in seconds: about Quake's turb speed.
    pub const MIN_WAVE_PERIOD: f32 = 6.0;
    /// Largest `waveAmplitude`, in texels.
    pub const MAX_WAVE_AMPLITUDE: f32 = 16.0;
    /// Longest `waveLength`, in texels. With the period floor it caps crest
    /// speed.
    pub const MAX_WAVE_LENGTH: f32 = 128.0;
    /// A crest is at least this many times the amplitude long, so the
    /// cross-coupled warp never folds.
    pub const WAVE_LENGTH_PER_AMPLITUDE: f32 = 8.0;

    /// The `waveLength` an omitted length resolves to for `amplitude`:
    /// `[max(48, 8A), max(80, 8A)]`. Always valid for an in-range amplitude.
    pub fn default_wave_length(amplitude: f32) -> [f32; 2] {
        let floor = Self::WAVE_LENGTH_PER_AMPLITUDE * amplitude;
        [48.0_f32.max(floor), 80.0_f32.max(floor)]
    }

    /// The murk colour, in `view_tint`'s rgb space: `fog_color` when declared,
    /// else `view_tint`'s rgb.
    pub fn resolved_fog_color(&self) -> [f32; 3] {
        self.fog_color
            .unwrap_or([self.view_tint[0], self.view_tint[1], self.view_tint[2]])
    }

    /// Check every field's range. A declaration that fails is rejected whole;
    /// the rest of the manifest stands. The wave amplitude is checked before
    /// the length, so an out-of-range amplitude with a defaulted length is
    /// reported as the amplitude; the 8× length rule binds a defaulted
    /// amplitude as it does a written one.
    pub fn validate(&self) -> Result<(), FluidFieldError> {
        fn unit(value: f32) -> bool {
            value.is_finite() && (0.0..=1.0).contains(&value)
        }
        if self.name.is_empty() {
            return Err(FluidFieldError {
                field: "name",
                requirement: "must be a non-empty string",
                got: "\"\"".to_string(),
            });
        }
        if !self.viscosity.is_finite() || self.viscosity < 0.0 {
            return Err(FluidFieldError {
                field: "viscosity",
                requirement: "must be a finite value >= 0",
                got: self.viscosity.to_string(),
            });
        }
        if !self.view_tint.iter().all(|&component| unit(component)) {
            return Err(FluidFieldError {
                field: "viewTint",
                requirement: "must have every component finite and in [0, 1]",
                got: format!("{:?}", self.view_tint),
            });
        }
        if !unit(self.muffle) {
            return Err(FluidFieldError {
                field: "muffle",
                requirement: "must be a finite value in [0, 1]",
                got: self.muffle.to_string(),
            });
        }
        if !unit(self.surface_opacity) {
            return Err(FluidFieldError {
                field: "surfaceOpacity",
                requirement: "must be a finite value in [0, 1]",
                got: self.surface_opacity.to_string(),
            });
        }
        for (field, key) in [
            ("enterSound", self.enter_sound.as_deref()),
            ("exitSound", self.exit_sound.as_deref()),
        ] {
            if let Some(key) = key
                && validate_sound_key(field, key).is_err()
            {
                return Err(FluidFieldError {
                    field,
                    requirement: "must be a sound key: non-empty, forward slashes, no parent traversal",
                    got: format!("`{key}`"),
                });
            }
        }
        self.validate_fog_and_wave()
    }

    fn validate_fog_and_wave(&self) -> Result<(), FluidFieldError> {
        fn unit(value: f32) -> bool {
            value.is_finite() && (0.0..=1.0).contains(&value)
        }
        if let Some(distance) = self.fog_distance
            && !(distance.is_finite() && distance > 0.0)
        {
            return Err(FluidFieldError {
                field: "fogDistance",
                requirement: "must be a finite value > 0",
                got: distance.to_string(),
            });
        }
        if let Some(color) = self.fog_color
            && !color.iter().all(|&component| unit(component))
        {
            return Err(FluidFieldError {
                field: "fogColor",
                requirement: "must have every component finite and in [0, 1]",
                got: format!("{color:?}"),
            });
        }
        if let Some(period) = self.wave_period
            && !(period.is_finite() && period >= Self::MIN_WAVE_PERIOD)
        {
            return Err(FluidFieldError {
                field: "wavePeriod",
                requirement: "must be a finite value >= 6",
                got: period.to_string(),
            });
        }
        let amplitude = self.wave_amplitude;
        if !(amplitude.is_finite() && (0.0..=Self::MAX_WAVE_AMPLITUDE).contains(&amplitude)) {
            return Err(FluidFieldError {
                field: "waveAmplitude",
                requirement: "must be a finite value in [0, 16]",
                got: amplitude.to_string(),
            });
        }
        let [min, max] = self.wave_length;
        if !(min.is_finite()
            && max.is_finite()
            && min > 0.0
            && min <= max
            && max <= Self::MAX_WAVE_LENGTH)
        {
            return Err(FluidFieldError {
                field: "waveLength",
                requirement: "must be finite with 0 < min <= max <= 128",
                got: format!("{:?}", self.wave_length),
            });
        }
        if min < Self::WAVE_LENGTH_PER_AMPLITUDE * amplitude {
            return Err(FluidFieldError {
                field: "waveLength",
                requirement: "must have min >= 8 x waveAmplitude",
                got: format!("{:?} with waveAmplitude {amplitude}", self.wave_length),
            });
        }
        Ok(())
    }
}

/// The committed fluid declarations, in declaration order. A fluid's index is
/// its position, which is what a resolved environment volume carries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FluidRegistry {
    descriptors: Vec<FluidDescriptor>,
}

impl FluidRegistry {
    /// Every index must fit the `u16` a resolved volume carries.
    pub const MAX_FLUIDS: usize = u16::MAX as usize + 1;

    /// Commit a manifest's `fluids` entries in authored order. Each malformed
    /// or out-of-range entry, each later duplicate name, and each entry past
    /// [`Self::MAX_FLUIDS`] is dropped with a warning naming it; the first
    /// declaration of a name stands and the rest of the manifest is kept.
    /// `scope` labels the manifest in diagnostics.
    pub fn from_declarations(
        scope: &str,
        entries: impl IntoIterator<Item = Result<FluidDescriptor, DescriptorError>>,
    ) -> Self {
        let mut descriptors: Vec<FluidDescriptor> = Vec::new();
        let mut first_index: Vec<usize> = Vec::new();
        for (index, entry) in entries.into_iter().enumerate() {
            let descriptor = match entry {
                Ok(descriptor) => descriptor,
                Err(error) => {
                    // A shape error's reason already names `fluids[i].<field>`;
                    // the generic manifest prefix would only repeat it.
                    let reason = match error {
                        DescriptorError::InvalidShape { reason } => reason,
                        other => other.to_string(),
                    };
                    log::warn!(
                        "[Scripting] {scope}: fluids[{index}] is malformed and was skipped: {reason}"
                    );
                    continue;
                }
            };
            if let Err(error) = descriptor.validate() {
                let subject = if descriptor.name.is_empty() {
                    "the fluid".to_string()
                } else {
                    format!("fluid '{}'", descriptor.name)
                };
                log::warn!("[Scripting] {scope}: fluids[{index}].{error}; {subject} was rejected");
                continue;
            }
            if let Some(position) = descriptors
                .iter()
                .position(|existing| existing.name == descriptor.name)
            {
                log::warn!(
                    "[Scripting] {scope}: fluids[{index}].name duplicates fluid '{}' declared at fluids[{}]; the first declaration stands",
                    descriptor.name,
                    first_index[position],
                );
                continue;
            }
            if descriptors.len() == Self::MAX_FLUIDS {
                log::warn!(
                    "[Scripting] {scope}: fluids[{index}] exceeds the {} fluid limit and was skipped",
                    Self::MAX_FLUIDS,
                );
                continue;
            }
            descriptors.push(descriptor);
            first_index.push(index);
        }
        Self { descriptors }
    }

    /// The fluid index a volume naming `name` resolves to.
    pub fn index_of(&self, name: &str) -> Option<u16> {
        self.descriptors
            .iter()
            .position(|descriptor| descriptor.name == name)
            .map(|index| index as u16)
    }

    /// The declaration at `index`.
    pub fn get(&self, index: u16) -> Option<&FluidDescriptor> {
        self.descriptors.get(usize::from(index))
    }

    /// Every committed declaration, in index order.
    pub fn descriptors(&self) -> &[FluidDescriptor] {
        &self.descriptors
    }

    /// The committed names, in index order.
    pub fn names(&self) -> Vec<String> {
        self.descriptors
            .iter()
            .map(|descriptor| descriptor.name.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn water() -> FluidDescriptor {
        FluidDescriptor {
            name: "water".to_string(),
            viscosity: 2.0,
            view_tint: [0.1, 0.3, 0.4, 0.35],
            muffle: 0.8,
            surface_opacity: 0.6,
            enter_sound: Some("sfx/splash_in".to_string()),
            exit_sound: None,
            fog_distance: None,
            fog_color: None,
            wave_period: None,
            wave_amplitude: FluidDescriptor::DEFAULT_WAVE_AMPLITUDE,
            wave_length: FluidDescriptor::default_wave_length(
                FluidDescriptor::DEFAULT_WAVE_AMPLITUDE,
            ),
        }
    }

    fn named(name: &str) -> FluidDescriptor {
        FluidDescriptor {
            name: name.to_string(),
            ..water()
        }
    }

    #[test]
    fn fluid_registry_indexes_by_declaration_order() {
        let registry = FluidRegistry::from_declarations(
            "test",
            [Ok(named("water")), Ok(named("sludge")), Ok(named("lava"))],
        );
        assert_eq!(registry.index_of("water"), Some(0));
        assert_eq!(registry.index_of("sludge"), Some(1));
        assert_eq!(registry.index_of("lava"), Some(2));
        assert_eq!(registry.index_of("acid"), None);
        assert_eq!(
            registry.get(1).map(|fluid| fluid.name.as_str()),
            Some("sludge")
        );
    }

    #[test]
    fn fluid_validation_names_each_out_of_range_field() {
        type Edit = fn(&mut FluidDescriptor);
        let cases: [(&str, Edit); 9] = [
            ("name", |fluid| fluid.name.clear()),
            ("viscosity", |fluid| fluid.viscosity = -0.5),
            ("viscosity", |fluid| fluid.viscosity = f32::INFINITY),
            ("viewTint", |fluid| fluid.view_tint[2] = 1.5),
            ("viewTint", |fluid| fluid.view_tint[0] = f32::NAN),
            ("muffle", |fluid| fluid.muffle = -0.1),
            ("surfaceOpacity", |fluid| fluid.surface_opacity = 1.01),
            ("enterSound", |fluid| {
                fluid.enter_sound = Some("../escape".to_string())
            }),
            ("exitSound", |fluid| fluid.exit_sound = Some(String::new())),
        ];
        assert!(water().validate().is_ok());
        for (field, edit) in cases {
            let mut fluid = water();
            edit(&mut fluid);
            assert_eq!(fluid.validate().map_err(|error| error.field), Err(field));
        }
    }

    // A NaN reaches a descriptor only from Rust (script conversion rejects
    // non-finite numbers first); validation still names the field.
    #[test]
    fn fluid_validation_names_each_out_of_range_fog_or_wave_field() {
        type Edit = fn(&mut FluidDescriptor);
        let cases: [(&str, Edit); 17] = [
            ("fogDistance", |fluid| fluid.fog_distance = Some(0.0)),
            ("fogDistance", |fluid| fluid.fog_distance = Some(-1.0)),
            ("fogDistance", |fluid| fluid.fog_distance = Some(f32::NAN)),
            ("fogDistance", |fluid| {
                fluid.fog_distance = Some(f32::INFINITY)
            }),
            ("fogColor", |fluid| fluid.fog_color = Some([0.5, 1.01, 0.5])),
            ("fogColor", |fluid| {
                fluid.fog_color = Some([-0.01, 0.5, 0.5])
            }),
            ("fogColor", |fluid| {
                fluid.fog_color = Some([0.5, 0.5, f32::NAN])
            }),
            ("wavePeriod", |fluid| fluid.wave_period = Some(f32::NAN)),
            ("wavePeriod", |fluid| fluid.wave_period = Some(5.99)),
            ("waveAmplitude", |fluid| fluid.wave_amplitude = -1.0),
            ("waveAmplitude", |fluid| fluid.wave_amplitude = f32::NAN),
            ("waveAmplitude", |fluid| fluid.wave_amplitude = 16.01),
            ("waveLength", |fluid| fluid.wave_length = [0.0, 0.0]),
            ("waveLength", |fluid| fluid.wave_length = [f32::NAN, 80.0]),
            ("waveLength", |fluid| fluid.wave_length = [80.0, 64.0]),
            ("waveLength", |fluid| fluid.wave_length = [64.0, 128.01]),
            ("waveLength", |fluid| fluid.wave_length = [63.99, 80.0]),
        ];
        for (field, edit) in cases {
            let mut fluid = water();
            edit(&mut fluid);
            assert_eq!(fluid.validate().map_err(|error| error.field), Err(field));
        }
    }

    #[test]
    fn fluid_validation_accepts_fog_and_wave_boundary_values() {
        type Edit = fn(&mut FluidDescriptor);
        let cases: [Edit; 9] = [
            // The smallest positive f32: "> 0" has no other floor.
            |fluid| fluid.fog_distance = Some(f32::from_bits(1)),
            |fluid| fluid.fog_color = Some([0.0, 1.0, 0.0]),
            |fluid| fluid.wave_amplitude = 0.0,
            |fluid| fluid.wave_period = Some(6.0),
            |fluid| {
                fluid.wave_amplitude = 16.0;
                fluid.wave_length = [128.0, 128.0];
            },
            |fluid| fluid.wave_length = [64.0, 128.0],
            |fluid| {
                fluid.wave_amplitude = 6.0;
                fluid.wave_length = [48.0, 80.0];
            },
            |fluid| {
                fluid.wave_amplitude = 6.5;
                fluid.wave_length = [52.0, 52.0];
            },
            |fluid| {
                fluid.wave_amplitude = 1.0;
                fluid.wave_length = [8.0, 128.0];
            },
        ];
        for edit in cases {
            let mut fluid = water();
            edit(&mut fluid);
            assert_eq!(fluid.validate(), Ok(()), "{fluid:?}");
        }
    }

    // Pin P5: the amplitude range is judged before the length, so a defaulted
    // length never takes the blame for an out-of-range amplitude.
    #[test]
    fn fluid_validation_reports_amplitude_before_a_defaulted_length() {
        for (amplitude, period) in [(30.0, None), (17.0, None), (17.0, Some(63.0))] {
            let mut fluid = water();
            fluid.wave_amplitude = amplitude;
            fluid.wave_period = period;
            fluid.wave_length = FluidDescriptor::default_wave_length(amplitude);
            assert_eq!(
                fluid.validate().map_err(|error| error.field),
                Err("waveAmplitude"),
                "amplitude {amplitude}, period {period:?}"
            );
        }
    }

    #[test]
    fn default_wave_length_floors_at_48_80_and_follows_eight_times_amplitude() {
        assert_eq!(FluidDescriptor::default_wave_length(8.0), [64.0, 80.0]);
        assert_eq!(FluidDescriptor::default_wave_length(4.0), [48.0, 80.0]);
        assert_eq!(FluidDescriptor::default_wave_length(16.0), [128.0, 128.0]);
        assert_eq!(FluidDescriptor::default_wave_length(0.0), [48.0, 80.0]);
        // Every in-range amplitude's default length passes validation.
        for step in 0..=160 {
            let mut fluid = water();
            fluid.wave_amplitude = step as f32 * 0.1;
            fluid.wave_length = FluidDescriptor::default_wave_length(fluid.wave_amplitude);
            assert_eq!(fluid.validate(), Ok(()), "{fluid:?}");
        }
    }

    #[test]
    fn omitted_fog_color_resolves_to_the_view_tint_rgb() {
        let mut fluid = water();
        assert_eq!(fluid.resolved_fog_color(), [0.1, 0.3, 0.4]);
        fluid.fog_color = Some([0.16, 0.22, 0.19]);
        assert_eq!(fluid.resolved_fog_color(), [0.16, 0.22, 0.19]);
    }

    #[test]
    fn fluid_registry_drops_rejected_entries_and_keeps_the_rest_in_order() {
        let mut bad_tint = named("bad");
        bad_tint.view_tint = [2.0, 0.0, 0.0, 1.0];
        let registry = FluidRegistry::from_declarations(
            "test",
            [
                Ok(named("water")),
                Ok(bad_tint),
                Err(DescriptorError::MissingField { field: "muffle" }),
                Ok(named("sludge")),
            ],
        );
        assert_eq!(registry.names(), ["water", "sludge"]);
        assert_eq!(registry.index_of("sludge"), Some(1));
    }

    #[test]
    fn fluid_registry_keeps_the_first_of_two_same_named_declarations() {
        let mut second = water();
        second.viscosity = 9.0;
        let registry = FluidRegistry::from_declarations("test", [Ok(water()), Ok(second)]);
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.get(0).map(|fluid| fluid.viscosity), Some(2.0));
    }
}
