// Client-apply call-site glue: routes an applied snapshot's `armed_local_pawn` to the
// descriptor-presentation materialization seam (Task 6 extends it to remote enemies).
// See: context/lib/networking.md

use crate::scripting::data_descriptors::EntityTypeDescriptor;
use crate::scripting::registry::EntityRegistry;

use super::client::ArmedLocalPawn;

/// Materialize the descriptor-backed presentation for a `local_player` baseline this
/// snapshot armed (M15 Phase 3 Task 3 + Task 7). `apply_snapshot` spawned the pawn
/// Transform-only; the descriptor-immutable movement tuning never crosses the wire, so
/// the client materializes the matching `PlayerMovementComponent` locally from the same
/// descriptor table both peers share — then the wire mutable subset has a component to
/// merge onto and prediction/reconciliation light up.
///
/// Defaults to the `"player"` class when the host stamped none (defensive). Must run
/// BEFORE reconcile (which merges onto the existing component); the underlying helper is
/// idempotent, so a re-arm of the same pawn keeps its live state.
///
/// Task 6 extends this seam with the remote-enemy presentation call so descriptor
/// materialization for replicated entities lives in one focused place, off the
/// `client_receive_and_apply` hot path.
pub(super) fn materialize_armed_local_pawn(
    armed: &ArmedLocalPawn,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
) {
    let entity_class = armed.entity_class.as_deref().unwrap_or("player");
    crate::scripting::builtins::net_descriptor::materialize_net_local_movement_component(
        entity_class,
        descriptors,
        registry,
        armed.entity_id,
    );
}
