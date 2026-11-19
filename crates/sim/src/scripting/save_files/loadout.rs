// A carried loadout as a save file stores it: names only, never engine ids.
// See: context/lib/networking.md §Session-state ledger

use std::collections::BTreeMap;

use postretro_combat_model::CarriedState;
use postretro_entities::AmmoReserve;
use postretro_entities::components::inventory::WIELDABLE_SLOT_CAPACITY;
use serde::{Deserialize, Serialize};

/// A seat's whole carried loadout, mirroring every `CarriedState` field.
/// Positional arrays omit trailing empty slots.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadoutRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub life_state: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reserve: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wieldables: Vec<Option<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub magazines: Vec<Option<u32>>,
    #[serde(default)]
    pub active_slot: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<LayerRecord>,
}

/// One defense layer's carried value.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LayerRecord {
    pub name: String,
    pub value: f32,
}

impl LoadoutRecord {
    /// Whether the record carries nothing: a fresh character's loadout.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The record of a carried state. `None` is a seat with no carry.
    pub fn from_carried(carried: Option<&CarriedState>) -> Self {
        let Some(carried) = carried else {
            return Self::default();
        };
        Self {
            health: carried.health_current.filter(|health| health.is_finite()),
            life_state: carried.life_state.clone(),
            reserve: carried
                .reserve
                .balances()
                .map(|(ammo, amount)| (ammo.to_owned(), amount))
                .collect(),
            wieldables: trim_trailing_none(carried.wieldables.to_vec()),
            magazines: trim_trailing_none(carried.magazines.to_vec()),
            active_slot: carried.active_slot,
            // JSON has no non-finite numbers: serde_json writes one as
            // `null`, which would make the whole file unreadable. A
            // non-finite layer value saves as empty.
            layers: carried
                .layers
                .iter()
                .map(|(name, value)| LayerRecord {
                    name: name.clone(),
                    value: if value.is_finite() { *value } else { 0.0 },
                })
                .collect(),
        }
    }

    /// The carried state this record restores, or `None` for an empty record,
    /// so a fresh character arrives exactly as a fresh seat does. Entries past
    /// this build's wieldable capacity drop with a warning each kind; an
    /// active slot past it selects slot 0.
    pub fn to_carried(&self, warnings: &mut Vec<String>) -> Option<CarriedState> {
        if self.is_empty() {
            return None;
        }
        let mut reserve = AmmoReserve::new();
        for (ammo, amount) in &self.reserve {
            reserve.set_exact(ammo, *amount);
        }
        let wieldables = fit_capacity(&self.wieldables, "wieldable", warnings);
        let magazines = fit_capacity(&self.magazines, "magazine", warnings);
        let active_slot = if self.active_slot < WIELDABLE_SLOT_CAPACITY {
            self.active_slot
        } else {
            warnings.push(format!(
                "active slot {} is past this build's {WIELDABLE_SLOT_CAPACITY} wieldable slots; selecting slot 0",
                self.active_slot
            ));
            0
        };
        Some(CarriedState {
            health_current: self.health,
            life_state: self.life_state.clone(),
            reserve,
            wieldables,
            magazines,
            active_slot,
            layers: self
                .layers
                .iter()
                .map(|layer| (layer.name.clone(), layer.value))
                .collect(),
        })
    }
}

fn trim_trailing_none<T>(mut entries: Vec<Option<T>>) -> Vec<Option<T>> {
    while entries.last().is_some_and(Option::is_none) {
        entries.pop();
    }
    entries
}

fn fit_capacity<T: Clone>(
    entries: &[Option<T>],
    kind: &str,
    warnings: &mut Vec<String>,
) -> [Option<T>; WIELDABLE_SLOT_CAPACITY] {
    let dropped = entries
        .iter()
        .skip(WIELDABLE_SLOT_CAPACITY)
        .filter(|entry| entry.is_some())
        .count();
    if dropped > 0 {
        warnings.push(format!(
            "dropped {dropped} {kind} entr{} past this build's {WIELDABLE_SLOT_CAPACITY} wieldable slots",
            if dropped == 1 { "y" } else { "ies" }
        ));
    }
    std::array::from_fn(|slot| entries.get(slot).cloned().flatten())
}
