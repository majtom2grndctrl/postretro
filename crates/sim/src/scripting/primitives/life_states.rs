//! Registered authoring types for the closed player capability vocabulary.
//! See: context/lib/scripting.md §12.

use postretro_scripting_core::primitives_registry::PrimitiveRegistry;

pub(super) fn register_types(registry: &mut PrimitiveRegistry) {
    registry
        .register_enum("LifeStatePosture")
        .doc("Posture imposed by a player life state. Omission leaves crouch under input control.")
        .variant("stand", "Normal crouch input control; the default.")
        .variant(
            "crouch",
            "Hold the existing crouch posture. Requires components.movement.crouch.",
        )
        .finish();
    registry
        .register_type("LifeStateMovementProfile")
        .doc("Movement capabilities for one life state. Omitted keys preserve full movement; no new movement state is registered.")
        .field("speedScale?", "f32", "Finite locomotion speed multiplier in [0, 1]. Defaults to 1.")
        .field("abilities?", "bool", "Allow movement abilities, including vault entry. Defaults to true; false breaks an active vault.")
        .field("posture?", "LifeStatePosture", "Forced posture. Defaults to stand, which keeps normal crouch input and stand-up clearance.")
        .finish();
    registry
        .register_type("LifeStateProfile")
        .doc("Closed engine capabilities for one author-named player life state. Every boolean defaults to true. Declaring life states hands zero-HP behavior to authored transitions; HP never changes the life state automatically.")
        .field("movement?", "LifeStateMovementProfile", "Movement scaling, abilities and posture. Omission permits full movement.")
        .field("look?", "bool", "Allow the owning player's look input. Defaults to true.")
        .field("fire?", "bool", "Allow weapon activations at any HP. Defaults to true; false cancels existing activations and charge.")
        .field("switchWeapon?", "bool", "Allow new weapon-switch requests and weapon drops, since a drop changes the active weapon. Defaults to true.")
        .field("reload?", "bool", "Allow new reload requests. Defaults to true.")
        .field("use?", "bool", "Allow trigger Use activation. Defaults to true.")
        .field("touch?", "bool", "Allow new trigger Touch entries, pickups and teleporter entry. Defaults to true; disabling it preserves volume membership and paired Exit behavior.")
        .field("occupancy?", "bool", "Include this pawn in trigger occupant counts. Defaults to true.")
        .field("damageable?", "bool", "Permit spatial damage targeting at any HP. Defaults to true. False excludes the pawn from projectile impacts, splash, AI attack checks and hitscan knockback. It does not gate hitscan or pellet damage, explicit script damage or mover crush, which keep their existing behavior.")
        .finish();
    registry
        .register_type("LifeStatesDescriptor")
        .doc("Author-named player life states; requires components.movement. Names use the engine ASCII identifier syntax. At most 255 distinct names may appear across the mod. Unknown profile keys reject content.")
        .field("initial", "String", "Declared state entered on spawn, level restart and ordinary respawn.")
        .field("states", "LifeStateProfiles", "State name to capability profile. An empty profile permits every capability.")
        .field("rejoin?", "LifeStateRejoinMap", "Leaving state to return state after disconnect or slot demotion. Both names must be declared here. A missing entry enters initial; level change always enters initial.")
        .finish();
}
