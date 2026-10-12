//! Position in the authored PRL map-entity list, stamped before filtering.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "MapOrdinalFields", into = "MapOrdinalFields")]
pub struct MapOrdinal(pub u32);

// ComponentValue's internally tagged envelope needs a struct-shaped payload.
// Keep the tuple API for engine identity lookups; diagnostics name its ordinal.
#[derive(Serialize, Deserialize)]
struct MapOrdinalFields {
    ordinal: u32,
}

impl From<MapOrdinal> for MapOrdinalFields {
    fn from(value: MapOrdinal) -> Self {
        Self { ordinal: value.0 }
    }
}

impl From<MapOrdinalFields> for MapOrdinal {
    fn from(value: MapOrdinalFields) -> Self {
        Self(value.ordinal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ComponentValue;

    // Regression: scalar MapOrdinal serialization failed inside diagnostic component records.
    #[test]
    fn map_ordinal_component_value_round_trip_preserves_authored_ordinal() {
        for ordinal in [0, 41, u32::MAX] {
            let value = ComponentValue::MapOrdinal(MapOrdinal(ordinal));
            let json = serde_json::to_value(&value).unwrap();
            assert_eq!(
                json,
                serde_json::json!({ "kind": "map_ordinal", "ordinal": ordinal })
            );
            let restored: ComponentValue = serde_json::from_value(json).unwrap();
            assert_eq!(restored, value);
        }
    }
}
