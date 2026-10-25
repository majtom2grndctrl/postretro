// Mod-manifest ghost style declarations, the engine floor style, and a link's
// style resolution by name.
// See: context/lib/scripting.md §2 (Ghost style declarations) · context/lib/entity_model.md §7d

use std::fmt;

use crate::data_descriptors::DescriptorError;

/// How a ghost looks and whether its sounds carry: a declared style's values
/// with omitted fields filled from [`Self::FLOOR`], or the floor itself. A
/// link's look starts here and `setColorFilter` overrides it field by field;
/// `audio` and `impact_audio` come from the style alone and never change at
/// runtime.
///
/// `impact_audio` is stored as authored. It takes effect only with `audio`:
/// consumers read [`Self::carries_impact_audio`], never the field alone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GhostStyle {
    /// Multiplier on the ghost's colour, `[r, g, b]`, each in `[0, 1]`.
    pub tint: [f32; 3],
    /// Presented opacity, `[0, 1]`.
    pub opacity: f32,
    /// Colour saturation, `[0, 1]`: 0 is greyscale, 1 the member's own colour.
    pub saturation: f32,
    /// A member's sounds also play a second voice on the Ghost bus.
    pub audio: bool,
    /// Impact sounds from a member's shots ghost too; only with `audio`.
    pub impact_audio: bool,
}

impl GhostStyle {
    /// The engine floor style: neutral and legible, carrying no sound. A link
    /// naming no style, or an undeclared one, presents it, and every field a
    /// declaration omits takes its value from it. The one source of these
    /// numbers. Provisional opacity: tuned at the in-engine legibility check.
    pub const FLOOR: Self = Self {
        tint: [1.0, 1.0, 1.0],
        opacity: 0.5,
        saturation: 0.0,
        audio: false,
        impact_audio: false,
    };

    /// Whether a member's sounds play a ghost voice.
    pub const fn carries_audio(&self) -> bool {
        self.audio
    }

    /// Whether impact sounds play a ghost voice: `impact_audio` gated by
    /// `audio`.
    pub const fn carries_impact_audio(&self) -> bool {
        self.audio && self.impact_audio
    }
}

impl Default for GhostStyle {
    fn default() -> Self {
        Self::FLOOR
    }
}

/// One `defineGhostStyle` declaration: the name a map's `ghost_link` refers
/// to through its `ghost_style` key, and the style it names. Omitted fields
/// were filled from [`GhostStyle::FLOOR`] at drain.
#[derive(Debug, Clone, PartialEq)]
pub struct GhostStyleDescriptor {
    pub name: String,
    pub style: GhostStyle,
}

/// The first out-of-range field on a ghost style declaration, by its
/// authored key.
#[derive(Debug, Clone, PartialEq)]
pub struct GhostStyleFieldError {
    /// Authored (camelCase) field name.
    pub field: &'static str,
    /// What the field must be.
    pub requirement: &'static str,
    /// The rejected value, formatted for the diagnostic.
    pub got: String,
}

impl fmt::Display for GhostStyleFieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}, got {}", self.field, self.requirement, self.got)
    }
}

/// A finite value in `[0, 1]`.
fn unit(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

impl GhostStyleDescriptor {
    /// Check every field's range. A declaration that fails is rejected whole.
    pub fn validate(&self) -> Result<(), GhostStyleFieldError> {
        if self.name.is_empty() {
            return Err(GhostStyleFieldError {
                field: "name",
                requirement: "must be a non-empty string",
                got: "\"\"".to_string(),
            });
        }
        let style = &self.style;
        if !style.tint.iter().all(|&component| unit(component)) {
            return Err(GhostStyleFieldError {
                field: "tint",
                requirement: "must have every component finite and in [0, 1]",
                got: format!("{:?}", style.tint),
            });
        }
        for (field, value) in [("opacity", style.opacity), ("saturation", style.saturation)] {
            if !unit(value) {
                return Err(GhostStyleFieldError {
                    field,
                    requirement: "must be a finite value in [0, 1]",
                    got: value.to_string(),
                });
            }
        }
        Ok(())
    }
}

/// A link named a style no committed declaration has. The link presents the
/// floor style; the install path logs this once per link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndeclaredGhostStyle {
    /// The name the link's `ghost_style` carries.
    pub name: String,
}

impl fmt::Display for UndeclaredGhostStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ghost_link style `{}` is not declared; using the floor style",
            self.name
        )
    }
}

impl UndeclaredGhostStyle {
    /// Log the warning with the `[Ghost]` tag. The wording is contract: an
    /// acceptance test asserts it.
    pub fn warn(&self) {
        log::warn!("[Ghost] {self}");
    }
}

/// The committed ghost style declarations, in declaration order. Links
/// resolve against it by name at level install and on each committed staged
/// reload; it never rides the wire and stays out of every digest.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GhostStyleRegistry {
    descriptors: Vec<GhostStyleDescriptor>,
}

impl GhostStyleRegistry {
    /// Commit a manifest's `ghostStyles` entries in authored order. Each
    /// malformed or out-of-range entry, and each later duplicate name, is
    /// dropped with a warning naming `ghostStyles[i]` (and the field, where
    /// one is at fault); the first declaration of a name stands and the rest
    /// of the manifest is kept. `scope` labels the manifest in diagnostics.
    pub fn from_declarations(
        scope: &str,
        entries: impl IntoIterator<Item = Result<GhostStyleDescriptor, DescriptorError>>,
    ) -> Self {
        let mut descriptors: Vec<GhostStyleDescriptor> = Vec::new();
        let mut first_index: Vec<usize> = Vec::new();
        for (index, entry) in entries.into_iter().enumerate() {
            let descriptor = match entry {
                Ok(descriptor) => descriptor,
                Err(error) => {
                    // A shape error's reason already names
                    // `ghostStyles[i].<field>`.
                    let reason = match error {
                        DescriptorError::InvalidShape { reason } => reason,
                        other => other.to_string(),
                    };
                    log::warn!(
                        "[Scripting] {scope}: ghostStyles[{index}] is malformed and was skipped: {reason}"
                    );
                    continue;
                }
            };
            if let Err(error) = descriptor.validate() {
                let subject = if descriptor.name.is_empty() {
                    "the ghost style".to_string()
                } else {
                    format!("ghost style '{}'", descriptor.name)
                };
                log::warn!(
                    "[Scripting] {scope}: ghostStyles[{index}].{error}; {subject} was rejected"
                );
                continue;
            }
            if let Some(position) = descriptors
                .iter()
                .position(|existing| existing.name == descriptor.name)
            {
                log::warn!(
                    "[Scripting] {scope}: ghostStyles[{index}].name duplicates ghost style '{}' declared at ghostStyles[{}]; the first declaration stands",
                    descriptor.name,
                    first_index[position],
                );
                continue;
            }
            descriptors.push(descriptor);
            first_index.push(index);
        }
        Self { descriptors }
    }

    /// The style declared as `name`, if any.
    pub fn get(&self, name: &str) -> Option<&GhostStyle> {
        self.descriptors
            .iter()
            .find(|descriptor| descriptor.name == name)
            .map(|descriptor| &descriptor.style)
    }

    /// Resolve a link's `ghost_style`. Empty (the link names no style) is the
    /// floor with no warning; a declared name is its style; an undeclared
    /// name is the floor plus the warning the caller logs once
    /// ([`UndeclaredGhostStyle::warn`]). Pure, so the install path re-runs it
    /// against each committed table.
    pub fn resolve(&self, name: &str) -> (GhostStyle, Option<UndeclaredGhostStyle>) {
        if name.is_empty() {
            return (GhostStyle::FLOOR, None);
        }
        match self.get(name) {
            Some(style) => (*style, None),
            None => (
                GhostStyle::FLOOR,
                Some(UndeclaredGhostStyle {
                    name: name.to_string(),
                }),
            ),
        }
    }

    /// Every committed declaration, in declaration order.
    pub fn descriptors(&self) -> &[GhostStyleDescriptor] {
        &self.descriptors
    }

    /// The committed names, in declaration order.
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

    fn echo() -> GhostStyleDescriptor {
        GhostStyleDescriptor {
            name: "echo".to_string(),
            style: GhostStyle {
                tint: [0.6, 0.8, 1.0],
                opacity: 0.45,
                saturation: 0.2,
                audio: true,
                impact_audio: false,
            },
        }
    }

    fn registry(entries: Vec<GhostStyleDescriptor>) -> GhostStyleRegistry {
        GhostStyleRegistry::from_declarations("test", entries.into_iter().map(Ok))
    }

    // Acceptance: the floor style carries no sound.
    #[test]
    fn the_floor_style_carries_no_sound() {
        let floor = GhostStyle::FLOOR;
        assert!(!floor.audio);
        assert!(!floor.impact_audio);
        assert!(!floor.carries_audio());
        assert!(!floor.carries_impact_audio());
        assert_eq!(GhostStyle::default(), floor);
        // The plan's provisional floor values.
        assert_eq!(floor.tint, [1.0, 1.0, 1.0]);
        assert_eq!(floor.opacity, 0.5);
        assert_eq!(floor.saturation, 0.0);
    }

    #[test]
    fn impact_audio_takes_effect_only_with_audio() {
        for (audio, impact_audio, carries) in [
            (false, false, false),
            (false, true, false),
            (true, false, false),
            (true, true, true),
        ] {
            let style = GhostStyle {
                audio,
                impact_audio,
                ..GhostStyle::FLOOR
            };
            assert_eq!(style.impact_audio, impact_audio, "stored as authored");
            assert_eq!(style.carries_impact_audio(), carries);
        }
    }

    // Acceptance (resolver half): a declared name presents that style, an
    // empty name the floor with no warning, an undeclared name the floor plus
    // the warning.
    #[test]
    fn a_link_style_resolves_to_its_declaration_the_floor_or_the_floor_with_a_warning() {
        let styles = registry(vec![echo()]);
        assert_eq!(styles.resolve("echo"), (echo().style, None));
        assert_eq!(styles.resolve(""), (GhostStyle::FLOOR, None));
        let (style, warning) = styles.resolve("phantom");
        assert_eq!(style, GhostStyle::FLOOR);
        let warning = warning.expect("an undeclared name warns");
        assert_eq!(warning.name, "phantom");
        assert_eq!(
            warning.to_string(),
            "ghost_link style `phantom` is not declared; using the floor style"
        );
        // Names match exactly.
        assert!(styles.resolve("Echo").1.is_some());
        assert_eq!(
            GhostStyleRegistry::default().resolve("echo").0,
            GhostStyle::FLOOR
        );
    }

    #[test]
    fn ghost_style_validation_names_each_out_of_range_field() {
        type Edit = fn(&mut GhostStyleDescriptor);
        let cases: [(&str, Edit); 9] = [
            ("name", |d| d.name.clear()),
            ("tint", |d| d.style.tint[1] = 1.5),
            ("tint", |d| d.style.tint[0] = -0.1),
            ("tint", |d| d.style.tint[2] = f32::NAN),
            ("opacity", |d| d.style.opacity = 1.01),
            ("opacity", |d| d.style.opacity = f32::INFINITY),
            ("saturation", |d| d.style.saturation = -0.5),
            ("saturation", |d| d.style.saturation = f32::NAN),
            ("opacity", |d| d.style.opacity = -0.0001),
        ];
        assert!(echo().validate().is_ok());
        for (field, edit) in cases {
            let mut descriptor = echo();
            edit(&mut descriptor);
            assert_eq!(
                descriptor.validate().map_err(|error| error.field),
                Err(field)
            );
        }
        // Both ends of every range are accepted.
        for value in [0.0, 1.0] {
            let descriptor = GhostStyleDescriptor {
                name: "edge".to_string(),
                style: GhostStyle {
                    tint: [value; 3],
                    opacity: value,
                    saturation: value,
                    ..GhostStyle::FLOOR
                },
            };
            assert!(descriptor.validate().is_ok(), "{value}");
        }
    }

    #[test]
    fn a_later_duplicate_name_is_dropped_and_the_first_declaration_stands() {
        let mut second = echo();
        second.style.opacity = 0.9;
        let styles = registry(vec![echo(), second]);
        assert_eq!(styles.names(), ["echo"]);
        assert_eq!(styles.get("echo").map(|style| style.opacity), Some(0.45));
    }
}
