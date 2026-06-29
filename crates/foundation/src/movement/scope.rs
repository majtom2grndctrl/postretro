// Movement-local `BindingScope`: the fixed, read-only input namespace dash value
// expressions bind against. Movement owns its scope the way the renderer owns the
// GPU — the IR module stays adopter-agnostic.
// See: context/lib/movement.md §2 (trigger vocabulary), context/lib/scripting.md §11

// The scope exposes a movement-local input set mirroring the §2 trigger-vocabulary
// nouns (`speed`, `grounded`, `chargesRemaining`, `elapsedMs`, …) over a
// fixed-size snapshot array. Binding is engine-side: the names resolve to indices
// into `values`, never through the script-facing slot table — so the
// `entity_model.md` §7b script-opacity invariant holds by construction (scripts
// cannot read or write the movement component through `worldQuery`).
//
// Read-only: dash value expressions consume movement state, they do not write it.
// `resolve_output` returns `None` for every name; `OutputHandle` exists only to
// satisfy the trait and `write` is unreachable.

use crate::ir::{BindingScope, IrType, IrValue, ResolvedInput, ResolvedOutput};
use crate::movement::PlayerMovementComponent;

/// The fixed movement input namespace, in handle order (index `0..6`). Each entry
/// is a `(name, projected IR type)` pair; `resolve_input` scans this table and the
/// matched index *is* the read handle into [`MovementScope::values`]. The order is
/// load-bearing: the handle is the array index, so refresh must write the same
/// slots in the same order.
///
/// Names mirror movement.md §2's trigger vocabulary and use the camelCase idiom of
/// the script surface (scripting.md §4 naming convention).
const INPUTS: [(&str, IrType); 6] = [
    ("speed", IrType::Number),            // horizontal |velocity.xz|
    ("verticalSpeed", IrType::Number),    // velocity.y
    ("grounded", IrType::Bool),           // is_grounded
    ("chargesRemaining", IrType::Number), // air_dashes_remaining as f32
    ("cooldownMs", IrType::Number),       // dash_cooldown_ms
    ("elapsedMs", IrType::Number),        // Dash state's elapsed_ms, 0.0 elsewhere
];

/// A movement-local binding scope: a fixed, read-only namespace over a six-value
/// snapshot array, mirroring the trigger-vocabulary nouns in movement.md §2. Dash
/// descriptor value fields bind their expressions against this.
///
/// Indexed handles (`usize` into [`MovementScope::values`]) keep binding engine
/// internal — the script-facing slot table is never consulted, so the
/// script-opacity invariant (entity_model.md §7b) holds by construction.
#[derive(Debug)]
pub struct MovementScope {
    /// The snapshot the bound expressions read. Slot `i` holds the value for
    /// `INPUTS[i]`. [`MovementScope::refresh`] repopulates it each tick before
    /// eval; [`MovementScope::for_validation`] zero-fills it for bind-only use.
    values: [IrValue; 6],
}

// Some typedef and descriptor-validation builds bind the movement vocabulary
// without ticking a live component, so `refresh` can be dead code there. The
// allow suppresses that warning without touching runtime builds.
#[cfg_attr(not(test), allow(dead_code))]
impl MovementScope {
    /// A scope with no live values, for declaration-time validation. Bind only
    /// consults names and types (never values), so a descriptor's dash
    /// expressions can type-check against this with no component in hand. The
    /// slots are type-correct zeros (`0.0` / `false`) so an accidental read would
    /// still be total.
    ///
    /// Used at descriptor declaration time (dash expression validation), at
    /// component construction (`DashPrograms`), and at the hot-path eval sites
    /// in `movement/mod.rs` where the scope is zero-initialised here then
    /// immediately populated via [`MovementScope::refresh`] before eval.
    pub fn for_validation() -> Self {
        Self {
            values: [
                IrValue::Number(0.0), // speed
                IrValue::Number(0.0), // verticalSpeed
                IrValue::Bool(false), // grounded
                IrValue::Number(0.0), // chargesRemaining
                IrValue::Number(0.0), // cooldownMs
                IrValue::Number(0.0), // elapsedMs
            ],
        }
    }

    /// Fill the snapshot from live movement state. `elapsed_ms` is passed in
    /// because it lives on the active `Dash` state, not the component; callers
    /// pass the dash state's `elapsed_ms` while dashing and `0.0` otherwise.
    ///
    /// Allocation-free: every slot is a stack scalar written into the owned array.
    pub fn refresh(&mut self, component: &PlayerMovementComponent, elapsed_ms: f32) {
        // Horizontal speed is |velocity.xz| — the magnitude on the ground plane.
        let v = component.velocity;
        let horizontal_speed = (v.x * v.x + v.z * v.z).sqrt();
        self.values = [
            IrValue::Number(horizontal_speed),
            IrValue::Number(v.y),
            IrValue::Bool(component.is_grounded),
            IrValue::Number(component.air_dashes_remaining as f32),
            IrValue::Number(component.dash_cooldown_ms),
            IrValue::Number(elapsed_ms),
        ];
    }
}

impl BindingScope for MovementScope {
    // Indexed handles: the snapshot-array slot. Deliberately unlike the store
    // scope's owned-name handles, proving the binding seam is pluggable.
    type InputHandle = usize;
    // Read-only scope: no output is ever resolved, so this is never constructed.
    // `usize` satisfies the trait with the same shape as `InputHandle`.
    type OutputHandle = usize;

    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<usize>> {
        INPUTS
            .iter()
            .position(|(input_name, _)| *input_name == name)
            .map(|handle| ResolvedInput {
                handle,
                ir_type: INPUTS[handle].1,
            })
    }

    fn resolve_output(&self, _name: &str) -> Option<ResolvedOutput<usize>> {
        // Read-only: dash expressions consume movement state, never write it.
        None
    }

    fn read(&self, handle: &usize) -> IrValue {
        // Total: the handle came from a successful `resolve_input`, so the index
        // is in bounds and the slot holds a value of the resolved type.
        self.values[*handle]
    }

    fn write(&mut self, _handle: &usize, _value: IrValue) {
        // Unreachable: `resolve_output` never grants a write handle, so bind never
        // produces an output for this scope and eval never calls `write`.
        unreachable!("MovementScope is read-only; resolve_output never grants a write handle");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_descriptors::{
        AirParams, CapsuleParams, FallParams, ForgivenessParams, GroundParams,
        PlayerMovementDescriptor, SpeedParams,
    };
    use crate::ir::test_scope::StubScope;
    use crate::ir::{BakedIr, CURRENT_IR_VERSION, IrNode, bind, eval_value};
    use glam::Vec3;

    const EPSILON: f32 = 1e-6;

    fn num(v: f32) -> Box<IrNode> {
        Box::new(IrNode::Const {
            value: IrValue::Number(v),
        })
    }

    fn input(name: &str) -> Box<IrNode> {
        Box::new(IrNode::Input {
            name: name.to_string(),
        })
    }

    fn read_only(root: IrNode) -> BakedIr {
        BakedIr {
            version: CURRENT_IR_VERSION,
            output: None,
            root,
        }
    }

    fn assert_number(value: IrValue, expected: f32) {
        match value {
            IrValue::Number(actual) => assert!(
                (actual - expected).abs() <= EPSILON,
                "expected {expected}, got {actual}"
            ),
            other => panic!("expected a number, got {other:?}"),
        }
    }

    /// A minimal valid movement descriptor (no `Default` impl exists). Mirrors the
    /// canonical descriptor in `movement/mod.rs`'s tests; only the fields the scope
    /// reads matter here, the rest are plausible defaults.
    fn minimal_descriptor() -> PlayerMovementDescriptor {
        PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.4,
                half_height: 0.8,
                eye_height: 0.5,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 11.0,
                    crouch: 3.0,
                },
                accel: 10.0,
                step_height: 0.3,
                max_slope: 45.0,
            },
            air: AirParams {
                forward_steer: 0.0,
                accel: 0.7,
                max_control_speed: 0.5,
                bunny_hop: false,
                jumps: 0,
                jump_velocity: 5.5,
                jump_ceiling: 0.0,
            },
            fall: FallParams {
                terminal_velocity: 40.0,
            },
            stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
            stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
            dash: None,
            forgiveness: Some(ForgivenessParams {
                coyote_ms: 0.0,
                jump_buffer_ms: 0.0,
            }),
            crouch: None,
            view_feel: None,
        }
    }

    /// A component with a known velocity, grounded flag, charges, and cooldown so
    /// refreshed snapshot slots are observable.
    fn seeded_component() -> PlayerMovementComponent {
        let mut component = PlayerMovementComponent::from_descriptor(&minimal_descriptor());
        // 3-4-5 triangle on the XZ plane → horizontal speed exactly 5.0.
        component.velocity = Vec3::new(3.0, 12.0, 4.0);
        component.is_grounded = true;
        component.air_dashes_remaining = 2;
        component.dash_cooldown_ms = 150.0;
        component
    }

    #[test]
    fn movement_scope_resolves_every_vocabulary_noun_in_order() {
        let scope = MovementScope::for_validation();
        for (index, (name, ir_type)) in INPUTS.iter().enumerate() {
            let resolved = scope
                .resolve_input(name)
                .unwrap_or_else(|| panic!("`{name}` must resolve"));
            assert_eq!(resolved.handle, index, "`{name}` handle is its array index");
            assert_eq!(
                resolved.ir_type, *ir_type,
                "`{name}` projects its declared type"
            );
        }
        assert!(
            scope.resolve_input("notAName").is_none(),
            "unknown names do not resolve"
        );
    }

    #[test]
    fn movement_scope_denies_every_output() {
        // Read-only: no name resolves to a write handle.
        let scope = MovementScope::for_validation();
        for (name, _) in INPUTS {
            assert!(
                scope.resolve_output(name).is_none(),
                "`{name}` must not resolve as a writable output"
            );
        }
    }

    #[test]
    fn refresh_projects_movement_state_into_snapshot_slots() {
        let mut scope = MovementScope::for_validation();
        scope.refresh(&seeded_component(), 80.0);

        // Bind one expression per noun and read it back through eval.
        let cases: [(&str, IrValue); 6] = [
            ("speed", IrValue::Number(5.0)),
            ("verticalSpeed", IrValue::Number(12.0)),
            ("grounded", IrValue::Bool(true)),
            ("chargesRemaining", IrValue::Number(2.0)),
            ("cooldownMs", IrValue::Number(150.0)),
            ("elapsedMs", IrValue::Number(80.0)),
        ];
        for (name, expected) in cases {
            let program =
                bind(&read_only(*input(name)), &scope).unwrap_or_else(|_| panic!("`{name}` binds"));
            let value = eval_value(&program, &scope);
            match expected {
                IrValue::Number(n) => assert_number(value, n),
                IrValue::Bool(b) => assert_eq!(value, IrValue::Bool(b), "`{name}`"),
            }
        }
    }

    #[test]
    fn for_validation_binds_without_a_component() {
        // Declaration-time validation path: bind consults only names and
        // types, so a dash expression type-checks against a value-less scope.
        let scope = MovementScope::for_validation();
        let expr = read_only(IrNode::Add {
            a: input("speed"),
            b: input("cooldownMs"),
        });
        bind(&expr, &scope).expect("a numeric dash expression binds with no component");
    }

    #[test]
    fn same_tree_binds_against_movement_and_stub_scopes() {
        // Portability AC (scripting.md §11): one IR tree binds against
        // `MovementScope` AND against the store-shaped `StubScope`. If anything
        // store-shaped had leaked into the movement scope, the same tree could not
        // resolve against an indexed stub seeded only with `speed`.
        let tree = read_only(IrNode::Add {
            a: input("speed"),
            b: num(1.0),
        });

        let mut movement_scope = MovementScope::for_validation();
        movement_scope.refresh(&seeded_component(), 0.0);
        let movement_program = bind(&tree, &movement_scope).expect("movement binds");
        assert_number(eval_value(&movement_program, &movement_scope), 6.0);

        // StubScope seeds `speed` = 4.0 by construction — same tree, distinct scope.
        let stub_scope = StubScope::new();
        let stub_program = bind(&tree, &stub_scope).expect("stub binds");
        assert_number(eval_value(&stub_program, &stub_scope), 5.0);
    }
}
