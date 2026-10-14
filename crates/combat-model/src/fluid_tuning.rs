// The host's fluid table on the tuning payload: movement fields only, in host declaration order.
// See: context/lib/networking.md §What gates, and what replicates instead

use std::fmt;

use postretro_foundation::{FluidDescriptor, FluidRegistry};
use serde::{Deserialize, Serialize};

/// One fluid's movement fields as the host sends them. Presentation fields
/// (tint, muffle, surface opacity, sounds) never ride the payload: a client
/// resolves those locally by `name`, as it does sound keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FluidMovementTuning {
    pub name: String,
    /// Drag while swimming, 1/s. Finite and ≥ 0.
    pub viscosity: f32,
}

/// A fluid table that level fluid names resolve against: a fluid's index is
/// its position, which is what a resolved environment volume carries. On the
/// wire it is a list of [`FluidMovementTuning`] in host declaration order; in
/// memory it is held column-wise so a movement view borrows the viscosities as
/// one slice without allocating. Every constructor enforces the registry's
/// invariants, so an index past [`FluidRegistry::MAX_FLUIDS`], an empty or
/// repeated name, or a non-finite or negative viscosity is unrepresentable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(
    try_from = "Vec<FluidMovementTuning>",
    into = "Vec<FluidMovementTuning>"
)]
pub struct FluidMovementTable {
    names: Vec<String>,
    viscosity: Vec<f32>,
}

/// Why a received fluid table was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum FluidTableError {
    TooMany { count: usize },
    EmptyName { index: usize },
    DuplicateName { index: usize, name: String },
    InvalidViscosity { index: usize, viscosity: f32 },
}

impl fmt::Display for FluidTableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooMany { count } => write!(
                f,
                "fluid table has {count} entries, more than the {} a fluid index addresses",
                FluidRegistry::MAX_FLUIDS
            ),
            Self::EmptyName { index } => write!(f, "fluids[{index}].name is empty"),
            Self::DuplicateName { index, name } => {
                write!(f, "fluids[{index}].name repeats fluid '{name}'")
            }
            Self::InvalidViscosity { index, viscosity } => write!(
                f,
                "fluids[{index}].viscosity must be finite and >= 0, got {viscosity}"
            ),
        }
    }
}

impl FluidMovementTable {
    /// Validate `entries` as a table, index = position.
    pub fn new(entries: Vec<FluidMovementTuning>) -> Result<Self, FluidTableError> {
        if entries.len() > FluidRegistry::MAX_FLUIDS {
            return Err(FluidTableError::TooMany {
                count: entries.len(),
            });
        }
        let mut table = Self {
            names: Vec::with_capacity(entries.len()),
            viscosity: Vec::with_capacity(entries.len()),
        };
        for (index, entry) in entries.into_iter().enumerate() {
            if entry.name.is_empty() {
                return Err(FluidTableError::EmptyName { index });
            }
            if !entry.viscosity.is_finite() || entry.viscosity < 0.0 {
                return Err(FluidTableError::InvalidViscosity {
                    index,
                    viscosity: entry.viscosity,
                });
            }
            if table.names.contains(&entry.name) {
                return Err(FluidTableError::DuplicateName {
                    index,
                    name: entry.name,
                });
            }
            table.names.push(entry.name);
            table.viscosity.push(entry.viscosity);
        }
        Ok(table)
    }

    /// The movement half of a committed registry, in its index order. The
    /// registry already enforces every table invariant at commit.
    pub fn from_registry(registry: &FluidRegistry) -> Self {
        let descriptors = registry.descriptors();
        Self {
            names: descriptors.iter().map(|fluid| fluid.name.clone()).collect(),
            viscosity: descriptors.iter().map(|fluid| fluid.viscosity).collect(),
        }
    }

    /// Fluid names in index order: what level fluid names resolve against.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Drag per fluid index, 1/s, parallel to [`Self::names`].
    pub fn viscosities(&self) -> &[f32] {
        &self.viscosity
    }

    /// The name of the fluid at `index`.
    pub fn name(&self, index: u16) -> Option<&str> {
        self.names.get(usize::from(index)).map(String::as_str)
    }

    /// The local presentation for the fluid at `index` in this table: the
    /// `local` registry's declaration of the same name, whatever index or
    /// numbers it has there. `None` when `local` does not declare it, so that
    /// machine shows no tint, muffle, surface or sounds for it, while movement
    /// still resolves the fluid through this table.
    pub fn local_presentation<'r>(
        &self,
        index: u16,
        local: &'r FluidRegistry,
    ) -> Option<&'r FluidDescriptor> {
        let name = self.name(index)?;
        local
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.name == name)
    }
}

impl TryFrom<Vec<FluidMovementTuning>> for FluidMovementTable {
    type Error = FluidTableError;

    fn try_from(entries: Vec<FluidMovementTuning>) -> Result<Self, Self::Error> {
        Self::new(entries)
    }
}

impl From<FluidMovementTable> for Vec<FluidMovementTuning> {
    fn from(table: FluidMovementTable) -> Self {
        table
            .names
            .into_iter()
            .zip(table.viscosity)
            .map(|(name, viscosity)| FluidMovementTuning { name, viscosity })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fluid(name: &str, viscosity: f32, tint: [f32; 4]) -> FluidDescriptor {
        FluidDescriptor {
            name: name.to_string(),
            viscosity,
            view_tint: tint,
            muffle: 0.5,
            surface_opacity: 0.6,
            enter_sound: None,
            exit_sound: None,
            fog_distance: None,
            fog_color: None,
            wave_period: None,
            wave_amplitude: 8.0,
            wave_length: [64.0, 80.0],
        }
    }

    fn registry(fluids: Vec<FluidDescriptor>) -> FluidRegistry {
        FluidRegistry::from_declarations("test", fluids.into_iter().map(Ok))
    }

    fn entry(name: &str, viscosity: f32) -> FluidMovementTuning {
        FluidMovementTuning {
            name: name.to_string(),
            viscosity,
        }
    }

    #[test]
    fn fluid_table_from_registry_keeps_declaration_order_and_drops_presentation() {
        let host = registry(vec![
            fluid("water", 2.0, [0.1, 0.3, 0.4, 0.35]),
            fluid("sludge", 6.5, [0.3, 0.3, 0.1, 0.8]),
        ]);
        let table = FluidMovementTable::from_registry(&host);
        assert_eq!(table.names(), ["water", "sludge"]);
        assert_eq!(table.viscosities(), [2.0, 6.5]);
        assert_eq!(
            Vec::<FluidMovementTuning>::from(table.clone()),
            [entry("water", 2.0), entry("sludge", 6.5)]
        );
        assert_eq!(
            FluidMovementTable::new(vec![entry("water", 2.0), entry("sludge", 6.5)]),
            Ok(table)
        );
    }

    #[test]
    fn fluid_table_rejects_empty_repeated_or_non_finite_entries() {
        let cases = [
            (
                vec![entry("water", 2.0), entry("", 1.0)],
                FluidTableError::EmptyName { index: 1 },
            ),
            (
                vec![entry("water", 2.0), entry("water", 1.0)],
                FluidTableError::DuplicateName {
                    index: 1,
                    name: "water".to_string(),
                },
            ),
            (
                vec![entry("water", -0.5)],
                FluidTableError::InvalidViscosity {
                    index: 0,
                    viscosity: -0.5,
                },
            ),
        ];
        for (entries, expected) in cases {
            assert_eq!(FluidMovementTable::new(entries), Err(expected));
        }
        assert!(matches!(
            FluidMovementTable::new(vec![entry("water", f32::NAN)]),
            Err(FluidTableError::InvalidViscosity { index: 0, .. })
        ));
        assert!(matches!(
            FluidMovementTable::new(vec![entry("water", f32::INFINITY)]),
            Err(FluidTableError::InvalidViscosity { index: 0, .. })
        ));
        assert!(FluidMovementTable::new(vec![entry("still", 0.0)]).is_ok());
    }

    // A client resolves presentation by name against its own manifest: the
    // host's index picks the name, the client's declaration supplies the look.
    #[test]
    fn local_presentation_resolves_the_table_name_against_the_local_registry() {
        let host = FluidMovementTable::new(vec![entry("water", 2.0), entry("sludge", 6.5)])
            .expect("valid table");
        let client_tint = [0.9, 0.1, 0.1, 0.5];
        let client = registry(vec![
            fluid("sludge", 1.0, [0.2; 4]),
            fluid("water", 7.0, client_tint),
        ]);

        let water = host
            .local_presentation(0, &client)
            .expect("the client declares water");
        assert_eq!(water.view_tint, client_tint);
        assert_eq!(
            host.local_presentation(1, &client).map(|f| f.name.as_str()),
            Some("sludge")
        );
        assert_eq!(host.local_presentation(2, &client), None, "past the table");

        let without_water = registry(vec![fluid("sludge", 1.0, [0.2; 4])]);
        assert_eq!(host.local_presentation(0, &without_water), None);
    }
}
