//! HUD visibility resolves the catalog's local String through the frozen map.
use crate::descriptor::{BindSource, PredicateValue};
use crate::tree::predicate::resolve_predicate;
use postretro_entities::{SlotTable, SlotValue};
use std::collections::HashMap;

#[test]
fn life_state_hud_visible_when_down_resolves_only_for_the_named_state() {
    let mut slots = SlotTable::new();
    let source = BindSource::Slot {
        slot: "player.lifeState".into(),
    };
    let equals = PredicateValue::String("down".into());
    for (name, expected) in [
        ("alive", 0.0),
        ("down", 1.0),
        ("spectating", 0.0),
        ("", 0.0),
    ] {
        slots
            .get_mut("player.lifeState")
            .unwrap()
            .write_value(Some(SlotValue::String(name.into())));
        let frozen = HashMap::from([(
            "player.lifeState".to_owned(),
            slots
                .get("player.lifeState")
                .unwrap()
                .value
                .clone()
                .unwrap(),
        )]);
        assert_eq!(
            resolve_predicate(&source, Some(&equals), None, &frozen, &HashMap::new()),
            expected
        );
    }
}
