// Behavior IR substrate: the typed, serializable command-buffer node tree,
// its value model, and the `BakedIr` wire envelope.
// See: context/lib/scripting.md §11 (Typed Command Buffer / IR substrate)

// Authored behavior that depends on live state crosses the FFI as this IR tree.
// The VM drops; a Rust total evaluator binds named leaves to live state (the
// `BindingScope` seam in `scope`) and evaluates each tick. This module owns the
// node model + value type + envelope; `scope` owns the binding seam; `bind`
// owns the once-per-program type-check + name-resolution pass.

pub mod bind;
pub mod eval;
pub mod load;
pub mod scope;
#[cfg(test)]
pub mod test_scope;

use serde::{Deserialize, Serialize};

// The substrate's public surface. This plan ships the IR substrate with no
// behavior adopting it yet (the first adopter is plan 3, movement), so these
// re-exports have no in-crate consumer outside tests — `allow(unused_imports)`
// holds the API stable until an adopter lands rather than deleting it now.
#[allow(unused_imports)]
pub use bind::{BindError, BoundNode, BoundProgram, bind};
#[allow(unused_imports)]
pub use eval::{eval_and_write, eval_value};
#[allow(unused_imports)]
pub use load::load_baked_ir;
#[allow(unused_imports)]
pub use scope::{BindingScope, ResolvedInput, ResolvedOutput};

/// Current IR wire-format version. Stamped into every [`BakedIr`] envelope.
///
/// The load-time version *check* (reject/ignore unsupported versions with a
/// warning, the adopter falls back to native behavior) lives in
/// `load::load_baked_ir`; this constant is the seam it validates against.
/// Bumping it requires a defined migration path, shared with the state-store
/// persist format (`state_persistence`).
pub const CURRENT_IR_VERSION: u32 = 1;

/// The two value types the evaluator computes over. Every node has a static
/// result type drawn from this set; `bind` type-checks against it once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrType {
    Number,
    Bool,
}

/// A runtime IR value. Two value types only: `number` (`f32`) and `boolean`.
///
/// `#[serde(untagged)]` makes this emit a *bare* JSON scalar — a number or a
/// bool, never an object. `Const { value }` therefore serializes as
/// `{ "op": "const", "value": 3.5 }` / `{ "op": "const", "value": true }`.
///
/// Under `untagged`, serde_json treats `Bool` and `Number` as disjoint: JSON
/// `true`/`false` never match the `Number` variant, and a bare numeric literal
/// never matches `Bool`, so variant ordering is not load-bearing here.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IrValue {
    Bool(bool),
    Number(f32),
}

impl IrValue {
    pub fn ir_type(self) -> IrType {
        match self {
            IrValue::Number(_) => IrType::Number,
            IrValue::Bool(_) => IrType::Bool,
        }
    }
}

/// A node in the authored behavior IR tree.
///
/// Closed vocabulary (cf. scripting.md §11): expressiveness comes from
/// composition, not from shipping code the engine runs. Every node is pure,
/// total, and bounded — no wall-clock, no RNG, no loops, no per-eval heap
/// allocation.
///
/// **Wire format (pinned — Task 3 byte-matches this):** internally-tagged on
/// `op` with snake_case tags, struct variants only. Internally-tagged serde
/// cannot represent a newtype variant carrying a primitive, so `Const` is a
/// struct variant carrying an untagged [`IrValue`] that emits a bare scalar.
///
/// | op | fields | result type |
/// |----|--------|-------------|
/// | `const` | `value` | the literal's type |
/// | `input` | `name` | the bound source's projected type |
/// | `add`/`sub`/`mul`/`div` | `a`,`b`: number | number |
/// | `clamp` | `x`,`lo`,`hi`: number | number |
/// | `lerp` | `a`,`b`,`t`: number | number |
/// | `lt`/`le`/`gt`/`ge` | `a`,`b`: number | boolean |
/// | `eq`/`ne` | `a`,`b`: same T | boolean |
/// | `select` | `cond`: boolean; `a`,`b`: same T | T |
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum IrNode {
    Const {
        value: IrValue,
    },
    Input {
        name: String,
    },

    Add {
        a: Box<IrNode>,
        b: Box<IrNode>,
    },
    Sub {
        a: Box<IrNode>,
        b: Box<IrNode>,
    },
    Mul {
        a: Box<IrNode>,
        b: Box<IrNode>,
    },
    Div {
        a: Box<IrNode>,
        b: Box<IrNode>,
    },

    Clamp {
        x: Box<IrNode>,
        lo: Box<IrNode>,
        hi: Box<IrNode>,
    },
    Lerp {
        a: Box<IrNode>,
        b: Box<IrNode>,
        t: Box<IrNode>,
    },

    Lt {
        a: Box<IrNode>,
        b: Box<IrNode>,
    },
    Le {
        a: Box<IrNode>,
        b: Box<IrNode>,
    },
    Gt {
        a: Box<IrNode>,
        b: Box<IrNode>,
    },
    Ge {
        a: Box<IrNode>,
        b: Box<IrNode>,
    },

    Eq {
        a: Box<IrNode>,
        b: Box<IrNode>,
    },
    Ne {
        a: Box<IrNode>,
        b: Box<IrNode>,
    },

    Select {
        cond: Box<IrNode>,
        a: Box<IrNode>,
        b: Box<IrNode>,
    },
}

/// The IR wire envelope. Carries a version stamp, an optional named output, and
/// the program root.
///
/// `output` absent ⇒ a read-only (value-producing) buffer: bind resolves no
/// write handle and the adopter reads the root's value. `output` present ⇒ the
/// root's result type must match the output slot's projected type, and bind
/// resolves a write handle (subject to the scope granting one).
///
/// Wire shape: `{ "version": 1, "output"?: "player.shield", "root": <node> }`.
/// `output` is omitted from the wire form when `None`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BakedIr {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    pub root: IrNode,
}

#[cfg(test)]
mod wire_format_tests {
    use super::*;

    /// Round-trips a node through its JSON form and asserts both directions:
    /// the JSON deserializes to the expected node, and the node re-serializes
    /// to byte-identical JSON. `expected_json` must already be in serde_json's
    /// canonical compact form so the equality check is meaningful.
    fn assert_round_trip(node: &IrNode, expected_json: &str) {
        let serialized = serde_json::to_string(node).expect("node should serialize");
        assert_eq!(
            serialized, expected_json,
            "serialized form must pin exactly"
        );

        let deserialized: IrNode =
            serde_json::from_str(expected_json).expect("json should deserialize");
        assert_eq!(&deserialized, node, "deserialized node must match original");

        // Re-serialize the deserialized node to prove the round-trip is stable.
        let reserialized = serde_json::to_string(&deserialized).expect("re-serialize");
        assert_eq!(reserialized, expected_json);
    }

    #[test]
    fn const_number_emits_bare_scalar_value() {
        assert_round_trip(
            &IrNode::Const {
                value: IrValue::Number(3.5),
            },
            r#"{"op":"const","value":3.5}"#,
        );
    }

    #[test]
    fn const_bool_emits_bare_scalar_value() {
        assert_round_trip(
            &IrNode::Const {
                value: IrValue::Bool(true),
            },
            r#"{"op":"const","value":true}"#,
        );
    }

    #[test]
    fn input_carries_name() {
        assert_round_trip(
            &IrNode::Input {
                name: "speed".to_string(),
            },
            r#"{"op":"input","name":"speed"}"#,
        );
    }

    #[test]
    fn arithmetic_ops_round_trip_with_struct_variants() {
        let leaf = |v: f32| {
            Box::new(IrNode::Const {
                value: IrValue::Number(v),
            })
        };
        assert_round_trip(
            &IrNode::Add {
                a: leaf(1.0),
                b: leaf(2.0),
            },
            r#"{"op":"add","a":{"op":"const","value":1.0},"b":{"op":"const","value":2.0}}"#,
        );
        assert_round_trip(
            &IrNode::Sub {
                a: leaf(1.0),
                b: leaf(2.0),
            },
            r#"{"op":"sub","a":{"op":"const","value":1.0},"b":{"op":"const","value":2.0}}"#,
        );
        assert_round_trip(
            &IrNode::Mul {
                a: leaf(1.0),
                b: leaf(2.0),
            },
            r#"{"op":"mul","a":{"op":"const","value":1.0},"b":{"op":"const","value":2.0}}"#,
        );
        assert_round_trip(
            &IrNode::Div {
                a: leaf(1.0),
                b: leaf(2.0),
            },
            r#"{"op":"div","a":{"op":"const","value":1.0},"b":{"op":"const","value":2.0}}"#,
        );
    }

    #[test]
    fn comparison_ops_round_trip() {
        let leaf = |v: f32| {
            Box::new(IrNode::Const {
                value: IrValue::Number(v),
            })
        };
        for (node, json) in [
            (
                IrNode::Lt {
                    a: leaf(1.0),
                    b: leaf(2.0),
                },
                r#"{"op":"lt","a":{"op":"const","value":1.0},"b":{"op":"const","value":2.0}}"#,
            ),
            (
                IrNode::Le {
                    a: leaf(1.0),
                    b: leaf(2.0),
                },
                r#"{"op":"le","a":{"op":"const","value":1.0},"b":{"op":"const","value":2.0}}"#,
            ),
            (
                IrNode::Gt {
                    a: leaf(1.0),
                    b: leaf(2.0),
                },
                r#"{"op":"gt","a":{"op":"const","value":1.0},"b":{"op":"const","value":2.0}}"#,
            ),
            (
                IrNode::Ge {
                    a: leaf(1.0),
                    b: leaf(2.0),
                },
                r#"{"op":"ge","a":{"op":"const","value":1.0},"b":{"op":"const","value":2.0}}"#,
            ),
            (
                IrNode::Eq {
                    a: leaf(1.0),
                    b: leaf(2.0),
                },
                r#"{"op":"eq","a":{"op":"const","value":1.0},"b":{"op":"const","value":2.0}}"#,
            ),
            (
                IrNode::Ne {
                    a: leaf(1.0),
                    b: leaf(2.0),
                },
                r#"{"op":"ne","a":{"op":"const","value":1.0},"b":{"op":"const","value":2.0}}"#,
            ),
        ] {
            assert_round_trip(&node, json);
        }
    }

    #[test]
    fn ternary_ops_round_trip() {
        let num = |v: f32| {
            Box::new(IrNode::Const {
                value: IrValue::Number(v),
            })
        };
        assert_round_trip(
            &IrNode::Clamp {
                x: num(5.0),
                lo: num(0.0),
                hi: num(1.0),
            },
            r#"{"op":"clamp","x":{"op":"const","value":5.0},"lo":{"op":"const","value":0.0},"hi":{"op":"const","value":1.0}}"#,
        );
        assert_round_trip(
            &IrNode::Lerp {
                a: num(0.0),
                b: num(10.0),
                t: num(0.5),
            },
            r#"{"op":"lerp","a":{"op":"const","value":0.0},"b":{"op":"const","value":10.0},"t":{"op":"const","value":0.5}}"#,
        );
        assert_round_trip(
            &IrNode::Select {
                cond: Box::new(IrNode::Const {
                    value: IrValue::Bool(true),
                }),
                a: num(1.0),
                b: num(2.0),
            },
            r#"{"op":"select","cond":{"op":"const","value":true},"a":{"op":"const","value":1.0},"b":{"op":"const","value":2.0}}"#,
        );
    }

    #[test]
    fn baked_ir_envelope_omits_absent_output() {
        let envelope = BakedIr {
            version: CURRENT_IR_VERSION,
            output: None,
            root: IrNode::Const {
                value: IrValue::Number(1.0),
            },
        };
        let json = serde_json::to_string(&envelope).expect("serialize envelope");
        assert_eq!(
            json, r#"{"version":1,"root":{"op":"const","value":1.0}}"#,
            "absent output must be omitted from the wire form"
        );

        let back: BakedIr = serde_json::from_str(&json).expect("deserialize envelope");
        assert_eq!(back, envelope);
    }

    #[test]
    fn baked_ir_envelope_carries_present_output() {
        let envelope = BakedIr {
            version: CURRENT_IR_VERSION,
            output: Some("player.shield".to_string()),
            root: IrNode::Input {
                name: "speed".to_string(),
            },
        };
        let json = serde_json::to_string(&envelope).expect("serialize envelope");
        assert_eq!(
            json,
            r#"{"version":1,"output":"player.shield","root":{"op":"input","name":"speed"}}"#,
        );

        let back: BakedIr = serde_json::from_str(&json).expect("deserialize envelope");
        assert_eq!(back, envelope);
    }
}
