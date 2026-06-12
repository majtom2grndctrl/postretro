// The load-time version seam: deserialize a baked IR envelope from JSON and
// validate its version stamp BEFORE the tree is ever bound or evaluated.
// See: context/lib/scripting.md §11 (Typed Command Buffer; IR substrate)

// This module owns the *sole* place the IR wire version is inspected. `bind`
// (and eval) assume an already-version-validated tree — they never re-check.
// The behavior mirrors the state-store persist loader
// (`state_persistence::overlay_persisted_state`): an unsupported version is
// ignored with a single `log::warn!`, never an error or panic, so the adopter
// falls back to its native behavior exactly as the persist loader leaves
// declared defaults standing when it cannot use a persisted file.
//
// # Opcode-vocabulary evolution rule
//
// The version stamp ([`CURRENT_IR_VERSION`]) tracks the *wire shape* of the IR,
// and opcode changes fall into two buckets:
//
// - **Additive (no version bump):** introducing a new opcode — a new [`IrNode`]
//   variant — is backward-compatible. An older engine simply never emitted it,
//   and a newer engine deserializes every prior tree unchanged. Adding nodes
//   does not move [`CURRENT_IR_VERSION`].
// - **Breaking (requires a version bump + migration path):** removing an opcode,
//   renaming its `op` tag, or changing an existing opcode's field shape or
//   runtime semantics breaks previously-baked trees. Any such change requires
//   incrementing [`CURRENT_IR_VERSION`] and defining a migration path for the
//   prior version.
//
// This is the **same versioning story** as the state-store persist format
// (`state_persistence`, `CURRENT_STATE_VERSION`) and the deferred `setState` IR:
// one scheme stamped into the envelope and checked once at load — not three
// separate schemes. An unsupported version is ignored and the adopter falls
// back to native behavior; the substrate never executes a tree it cannot vouch
// for.

use super::{BakedIr, CURRENT_IR_VERSION};

/// Deserialize a baked IR envelope from JSON and validate its version stamp.
///
/// Returns `Some(envelope)` only when the JSON deserializes into a [`BakedIr`]
/// *and* its `version` equals [`CURRENT_IR_VERSION`] (today, the only supported
/// version is `1`). Otherwise returns `None` after a single `log::warn!`:
///
/// - **Malformed / undeserializable JSON** → `None` + warn. The tree is never
///   constructed.
/// - **Unsupported version** → `None` + warn naming found vs. expected. Never an
///   error or panic — the adopter falls back to its native behavior, mirroring
///   the persist loader ignoring an unsupported persisted state.
///
/// This is the only seam that inspects the version; `bind` assumes the tree it
/// receives has already passed through here.
pub(crate) fn load_baked_ir(json: &str) -> Option<BakedIr> {
    let envelope: BakedIr = match serde_json::from_str(json) {
        Ok(envelope) => envelope,
        Err(error) => {
            log::warn!("[Scripting] failed to deserialize baked IR envelope; ignoring it: {error}");
            return None;
        }
    };

    if envelope.version != CURRENT_IR_VERSION {
        log::warn!(
            "[Scripting] baked IR version {} is not supported (current version is {}); ignoring it",
            envelope.version,
            CURRENT_IR_VERSION
        );
        return None;
    }

    Some(envelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ir::{IrNode, IrValue};

    fn sample_envelope() -> BakedIr {
        BakedIr {
            version: CURRENT_IR_VERSION,
            output: Some("player.shield".to_string()),
            root: IrNode::Add {
                a: Box::new(IrNode::Const {
                    value: IrValue::Number(1.0),
                }),
                b: Box::new(IrNode::Input {
                    name: "speed".to_string(),
                }),
            },
        }
    }

    #[test]
    fn current_version_envelope_round_trips() {
        let envelope = sample_envelope();
        let json = serde_json::to_string(&envelope).expect("serialize envelope");

        let loaded = load_baked_ir(&json).expect("current-version envelope loads");
        assert_eq!(loaded, envelope);
    }

    #[test]
    fn unsupported_version_envelope_is_ignored() {
        let mut envelope = sample_envelope();
        envelope.version = 999;
        let json = serde_json::to_string(&envelope).expect("serialize envelope");

        // No panic: the adopter falls back to native behavior. The warning is
        // review-observable; the test does not gate on capturing it.
        assert!(load_baked_ir(&json).is_none());
    }

    #[test]
    fn malformed_json_is_ignored() {
        assert!(load_baked_ir("{ this is not valid json ]").is_none());
        // Structurally valid JSON that is not a BakedIr envelope also yields None.
        assert!(load_baked_ir(r#"{"unexpected":true}"#).is_none());
    }
}
