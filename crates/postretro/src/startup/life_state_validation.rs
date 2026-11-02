//! Validate authored life-state references before reactions are partitioned.
//! See: context/lib/scripting.md §12.

use postretro_entities::DataRegistry;
use postretro_scripting_core::data_descriptors::ReactionDescriptor;

pub(super) fn validate_life_state_references(data: &mut DataRegistry) {
    fn reference<'a>(primitive: &str, args: &'a serde_json::Value) -> Option<&'a str> {
        matches!(primitive, "setLifeState" | "respawn")
            .then(|| args.get("state").and_then(serde_json::Value::as_str))
            .flatten()
    }
    for reaction in &mut data.reactions {
        let missing = match &reaction.descriptor {
            ReactionDescriptor::Primitive(p) => reference(&p.primitive, &p.args)
                .filter(|state| !data.life_state_names.iter().any(|name| name == state)),
            ReactionDescriptor::Sequence(steps) => steps
                .iter()
                .filter_map(|step| reference(&step.primitive, &step.args))
                .find(|state| !data.life_state_names.iter().any(|name| name == state)),
            ReactionDescriptor::Progress(_) => None,
        };
        if let Some(state) = missing {
            log::error!(
                "[LifeState] reaction `{}` names unknown state `{state}`; dropping the reaction",
                reaction.name
            );
            reaction.descriptor = ReactionDescriptor::Sequence(Vec::new());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::{GroupKind, NamedReaction, PrimitiveDescriptor};
    use postretro_test_log_capture::LogCapture;
    use serde_json::json;

    #[test]
    fn life_state_install_rejects_unknown_state_and_names_each_reaction() {
        let capture = LogCapture::start();
        let mut data = DataRegistry::new();
        data.life_state_names = vec!["alive".into()];
        for (name, primitive, state) in [
            ("wrong-set", "setLifeState", "missing"),
            ("wrong-respawn", "respawn", "spectating"),
            ("valid", "setLifeState", "alive"),
        ] {
            data.reactions.push(NamedReaction {
                name: name.into(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: primitive.into(),
                    kind: Some(GroupKind::Player),
                    tag: None,
                    target: None,
                    on_complete: None,
                    args: json!({"state":state}),
                }),
            });
        }
        validate_life_state_references(&mut data);
        assert!(
            matches!(&data.reactions[0].descriptor, ReactionDescriptor::Sequence(steps) if steps.is_empty())
        );
        assert!(
            matches!(&data.reactions[1].descriptor, ReactionDescriptor::Sequence(steps) if steps.is_empty())
        );
        assert!(matches!(
            &data.reactions[2].descriptor,
            ReactionDescriptor::Primitive(_)
        ));
        let records = capture.records();
        for (reaction, state) in [("wrong-set", "missing"), ("wrong-respawn", "spectating")] {
            assert!(
                records
                    .iter()
                    .any(|record| record.level == log::Level::Error
                        && record.message.contains(reaction)
                        && record.message.contains(state))
            );
        }
        assert!(
            !records
                .iter()
                .any(|record| record.level == log::Level::Error
                    && record.message.contains("`valid`"))
        );
    }
}
