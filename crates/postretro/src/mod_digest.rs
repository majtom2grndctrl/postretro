//! Compatibility digest for the mod-global trigger lanes that both peers evaluate.
//!
//! The recipe is intentionally narrow. Entity descriptors are replicated as tuning
//! values, while per-level declarations, reactions, and events stay outside this
//! digest by design.

use postretro_entities::{
    CrossingCondition, CrossingDescriptor, ScopedCrossing, TriggerEventDescriptor, TriggerPoolArm,
    TriggerPoolDescriptor,
};

use crate::content_hash::{hash_f32, hash_f64, hash_ir_node, hash_len, hash_str, hash_u32};

/// Produce a deterministic digest over the three mod-global trigger lanes.
///
/// Every entry receives an independent canonical hash before lane ordering is
/// erased. The exhaustive walks below are a denylist: adding a field or enum
/// variant in the reached domain fails compilation until its representation is
/// chosen here.
pub(crate) fn mod_compatibility_digest(
    trigger_events: &[TriggerEventDescriptor],
    trigger_pools: &[TriggerPoolDescriptor],
    crossings: &[ScopedCrossing],
) -> [u8; 32] {
    const MOD_DIGEST_EPOCH: u32 = 1;

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"postretro-mod-compatibility");
    hasher.update(&MOD_DIGEST_EPOCH.to_le_bytes());
    hash_lane(&mut hasher, trigger_events, hash_trigger_event_descriptor);
    hash_lane(&mut hasher, trigger_pools, hash_trigger_pool_descriptor);
    hash_lane(&mut hasher, crossings, hash_scoped_crossing);
    *hasher.finalize().as_bytes()
}

fn hash_lane<T>(
    hasher: &mut blake3::Hasher,
    entries: &[T],
    hash_entry: fn(&mut blake3::Hasher, &T),
) {
    let mut digests: Vec<[u8; 32]> = entries
        .iter()
        .map(|entry| {
            let mut entry_hasher = blake3::Hasher::new();
            hash_entry(&mut entry_hasher, entry);
            *entry_hasher.finalize().as_bytes()
        })
        .collect();
    digests.sort_unstable();

    hash_len(hasher, digests.len());
    for digest in digests {
        hasher.update(&digest);
    }
}

fn hash_trigger_event_descriptor(hasher: &mut blake3::Hasher, descriptor: &TriggerEventDescriptor) {
    let TriggerEventDescriptor {
        tag,
        event,
        fire,
        levels,
    } = descriptor;
    hash_str(hasher, tag);
    hash_str(hasher, event);
    hash_strings(hasher, fire);
    hash_strings(hasher, levels);
}

fn hash_trigger_pool_descriptor(hasher: &mut blake3::Hasher, descriptor: &TriggerPoolDescriptor) {
    let TriggerPoolDescriptor { tag, arm, levels } = descriptor;
    hash_str(hasher, tag);
    hash_trigger_pool_arm(hasher, arm);
    hash_strings(hasher, levels);
}

fn hash_trigger_pool_arm(hasher: &mut blake3::Hasher, arm: &TriggerPoolArm) {
    match arm {
        TriggerPoolArm::Count(count) => {
            hasher.update(&[0]);
            hash_u32(hasher, *count);
        }
        TriggerPoolArm::Percentage(percentage) => {
            hasher.update(&[1]);
            hash_f64(hasher, *percentage);
        }
    }
}

fn hash_scoped_crossing(hasher: &mut blake3::Hasher, scoped: &ScopedCrossing) {
    let ScopedCrossing { crossing, levels } = scoped;
    hash_crossing_descriptor(hasher, crossing);
    hash_strings(hasher, levels);
}

fn hash_crossing_descriptor(hasher: &mut blake3::Hasher, descriptor: &CrossingDescriptor) {
    let CrossingDescriptor {
        slot,
        condition,
        max,
        edge,
        fire,
    } = descriptor;
    hash_option_string(hasher, slot);
    hash_crossing_condition(hasher, condition);
    hash_f32(hasher, *max);
    hash_option_string(hasher, edge);
    hash_strings(hasher, fire);
}

fn hash_crossing_condition(hasher: &mut blake3::Hasher, condition: &CrossingCondition) {
    match condition {
        CrossingCondition::Below { threshold } => {
            hasher.update(&[0]);
            hash_f32(hasher, *threshold);
        }
        CrossingCondition::Above { threshold } => {
            hasher.update(&[1]);
            hash_f32(hasher, *threshold);
        }
        CrossingCondition::Ir(node) => {
            hasher.update(&[2]);
            hash_ir_node(hasher, node);
        }
    }
}

fn hash_option_string(hasher: &mut blake3::Hasher, value: &Option<String>) {
    match value {
        Some(value) => {
            hasher.update(&[1]);
            hash_str(hasher, value);
        }
        None => {
            hasher.update(&[0]);
        }
    }
}

fn hash_strings(hasher: &mut blake3::Hasher, values: &[String]) {
    hash_len(hasher, values.len());
    for value in values {
        hash_str(hasher, value);
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::fs;
    use std::path::PathBuf;

    use postretro_foundation::ir::{IrNode, IrValue};

    use super::*;

    const BLESS_ENV: &str = "POSTRETRO_BLESS_COMPATIBILITY_FIXTURES";
    const FIXTURE_DIGEST_HEX: &str =
        "7e92147feab99827d4824b740070f164d94963fa211852a460b2c8017a5c19e0";

    fn events() -> Vec<TriggerEventDescriptor> {
        vec![
            TriggerEventDescriptor {
                tag: "level-start".to_string(),
                event: "entered".to_string(),
                fire: vec!["raise-gate".to_string(), "play-sting".to_string()],
                levels: vec!["arena".to_string()],
            },
            TriggerEventDescriptor {
                tag: "level-start".to_string(),
                event: "after-entered".to_string(),
                fire: vec!["arm-pool".to_string()],
                levels: Vec::new(),
            },
        ]
    }

    fn pools() -> Vec<TriggerPoolDescriptor> {
        vec![
            TriggerPoolDescriptor {
                tag: "ambushes".to_string(),
                arm: TriggerPoolArm::Percentage(0.125),
                levels: vec!["arena".to_string()],
            },
            TriggerPoolDescriptor {
                tag: "rewards".to_string(),
                arm: TriggerPoolArm::Count(3),
                levels: Vec::new(),
            },
        ]
    }

    fn crossing(condition: CrossingCondition) -> ScopedCrossing {
        ScopedCrossing {
            crossing: CrossingDescriptor {
                slot: Some("player.shield".to_string()),
                condition,
                max: 100.0,
                edge: Some("both".to_string()),
                fire: vec!["shield-warning".to_string()],
            },
            levels: vec!["arena".to_string()],
        }
    }

    fn crossings() -> Vec<ScopedCrossing> {
        vec![
            crossing(CrossingCondition::Below { threshold: 0.25 }),
            crossing(CrossingCondition::Ir(IrNode::Gt {
                a: Box::new(IrNode::Input {
                    name: "player.speed".to_string(),
                }),
                b: Box::new(IrNode::Const {
                    value: IrValue::Number(3.5),
                }),
            })),
        ]
    }

    fn digest() -> [u8; 32] {
        mod_compatibility_digest(&events(), &pools(), &crossings())
    }

    #[test]
    fn digest_is_order_independent_within_each_lane() {
        let events = events();
        let pools = pools();
        let crossings = crossings();
        let expected = mod_compatibility_digest(&events, &pools, &crossings);

        let mut reversed_events = events;
        let mut reversed_pools = pools;
        let mut reversed_crossings = crossings;
        reversed_events.reverse();
        reversed_pools.reverse();
        reversed_crossings.reverse();

        assert_eq!(
            expected,
            mod_compatibility_digest(&reversed_events, &reversed_pools, &reversed_crossings)
        );
    }

    #[test]
    fn digest_changes_for_crossing_event_and_pool_edits() {
        let baseline = digest();

        let mut changed_crossings = crossings();
        changed_crossings[0].crossing.condition = CrossingCondition::Below { threshold: 0.5 };
        assert_ne!(
            baseline,
            mod_compatibility_digest(&events(), &pools(), &changed_crossings)
        );

        let mut changed_crossings = crossings();
        changed_crossings[0].crossing.edge = None;
        assert_ne!(
            baseline,
            mod_compatibility_digest(&events(), &pools(), &changed_crossings)
        );

        let mut changed_crossings = crossings();
        changed_crossings[1].crossing.condition = CrossingCondition::Ir(IrNode::Ge {
            a: Box::new(IrNode::Input {
                name: "player.speed".to_string(),
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(3.5),
            }),
        });
        assert_ne!(
            baseline,
            mod_compatibility_digest(&events(), &pools(), &changed_crossings)
        );

        let mut changed_events = events();
        changed_events[0].event = "entered-late".to_string();
        assert_ne!(
            baseline,
            mod_compatibility_digest(&changed_events, &pools(), &crossings())
        );

        let mut changed_pools = pools();
        changed_pools[0].arm = TriggerPoolArm::Percentage(0.25);
        assert_ne!(
            baseline,
            mod_compatibility_digest(&events(), &changed_pools, &crossings())
        );
    }

    #[test]
    fn digest_ir_is_structural() {
        let first = crossing(CrossingCondition::Ir(IrNode::Add {
            a: Box::new(IrNode::Input {
                name: "player.speed".to_string(),
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(1.0),
            }),
        }));
        let equal = first.clone();
        let different = crossing(CrossingCondition::Ir(IrNode::Sub {
            a: Box::new(IrNode::Input {
                name: "player.speed".to_string(),
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(1.0),
            }),
        }));

        assert_eq!(
            mod_compatibility_digest(&[], &[], &[first]),
            mod_compatibility_digest(&[], &[], &[equal])
        );
        assert_ne!(
            mod_compatibility_digest(
                &[],
                &[],
                &[crossing(CrossingCondition::Ir(IrNode::Add {
                    a: Box::new(IrNode::Input {
                        name: "player.speed".to_string(),
                    }),
                    b: Box::new(IrNode::Const {
                        value: IrValue::Number(1.0),
                    }),
                }))]
            ),
            mod_compatibility_digest(&[], &[], &[different])
        );
    }

    #[test]
    fn committed_digest_fixture_is_stable() {
        let actual = hex(digest());
        if std::env::var_os(BLESS_ENV).is_some() {
            bless_digest_constant(&actual);
            return;
        }

        assert_eq!(
            actual, FIXTURE_DIGEST_HEX,
            "mod digest changed; if intentional, bump MOD_DIGEST_EPOCH and re-bless with {BLESS_ENV}=1"
        );
    }

    fn hex(digest: [u8; 32]) -> String {
        let mut output = String::with_capacity(64);
        for byte in digest {
            write!(output, "{byte:02x}").expect("writing to a String cannot fail");
        }
        output
    }

    fn bless_digest_constant(actual: &str) {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/mod_digest.rs");
        let source = fs::read_to_string(&path).expect("read mod digest source for bless");
        let old = format!("const FIXTURE_DIGEST_HEX: &str = \"{FIXTURE_DIGEST_HEX}\";");
        let new = format!("const FIXTURE_DIGEST_HEX: &str = \"{actual}\";");
        assert!(
            source.contains(&old),
            "digest bless marker missing from {}",
            path.display()
        );
        fs::write(&path, source.replacen(&old, &new, 1)).expect("write mod digest bless result");
    }

    // This is deliberately inert. It breaks when a reached descriptor field or
    // enum variant is added; update the recipe and this sentinel together, never
    // widen either destructuring pattern with `..` or a wildcard arm.
    #[allow(dead_code)]
    fn exhaustive_domain_sentinel(
        scoped: ScopedCrossing,
        crossing: CrossingDescriptor,
        condition: CrossingCondition,
        event: TriggerEventDescriptor,
        pool: TriggerPoolDescriptor,
        arm: TriggerPoolArm,
        node: IrNode,
        value: IrValue,
    ) {
        let ScopedCrossing {
            crossing: _,
            levels: _,
        } = scoped;
        let CrossingDescriptor {
            slot: _,
            condition: _,
            max: _,
            edge: _,
            fire: _,
        } = crossing;
        let TriggerEventDescriptor {
            tag: _,
            event: _,
            fire: _,
            levels: _,
        } = event;
        let TriggerPoolDescriptor {
            tag: _,
            arm: _,
            levels: _,
        } = pool;
        match condition {
            CrossingCondition::Below { threshold: _ } => {}
            CrossingCondition::Above { threshold: _ } => {}
            CrossingCondition::Ir(_) => {}
        }
        match arm {
            TriggerPoolArm::Count(_) => {}
            TriggerPoolArm::Percentage(_) => {}
        }
        match node {
            IrNode::Const { value: _ } => {}
            IrNode::Input { name: _ } => {}
            IrNode::Add { a: _, b: _ } => {}
            IrNode::Sub { a: _, b: _ } => {}
            IrNode::Mul { a: _, b: _ } => {}
            IrNode::Div { a: _, b: _ } => {}
            IrNode::Clamp { x: _, lo: _, hi: _ } => {}
            IrNode::Lerp { a: _, b: _, t: _ } => {}
            IrNode::Lt { a: _, b: _ } => {}
            IrNode::Le { a: _, b: _ } => {}
            IrNode::Gt { a: _, b: _ } => {}
            IrNode::Ge { a: _, b: _ } => {}
            IrNode::Eq { a: _, b: _ } => {}
            IrNode::Ne { a: _, b: _ } => {}
            IrNode::Select {
                cond: _,
                a: _,
                b: _,
            } => {}
        }
        match value {
            IrValue::Bool(_) => {}
            IrValue::Number(_) => {}
        }
    }
}
