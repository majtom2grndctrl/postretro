// M15 Phase 3 regression: a connected client must NOT spawn a boot player pawn at
// level install. The host's `local_player` baseline arms exactly one PlayerMovement
// pawn; a boot pawn would be a second, never-replicated, never-despawned pawn (camera
// glued to the frozen boot pawn pre-arm, an entity switch + spurious boot-pos →
// host-pos reconcile teleport at arm). These tests pin the gated invariant the Task 6
// `LoopbackHarness` otherwise skips: it starts the client registry empty (the
// post-fix state) and never exercises the install boot-spawn that produced the bug.
// See: context/lib/networking.md · context/plans/in-progress/M15--p3-... Task 3/6

#![cfg(test)]

use std::collections::HashMap;

use glam::Vec3;

use postretro_net::harness::LinkConfig;

use super::predict_reconcile_harness_test_fixtures::{
    ENTITY_CLASS, LoopbackHarness, entity_descriptors, forward_command,
};
use super::role_suppresses_boot_player_spawn;
use crate::netcode::NetRole;
use crate::scripting::builtins::MapEntity;
use crate::scripting::builtins::data_archetype::spawn_from_player_starts;
use crate::scripting::registry::{ComponentKind, EntityRegistry};

/// A near-perfect link (a small fixed delay, no jitter or loss) so the boot → arm
/// sequence converges deterministically without the latency profile obscuring the
/// pawn-count assertions. Mirrors the harness's own scenario link.
fn light_link() -> LinkConfig {
    LinkConfig {
        delay: 32,
        jitter: 0,
        loss_probability: 0.0,
        seed: 0x6010,
    }
}

/// Count the `PlayerMovement` pawns currently in `registry`.
fn player_movement_pawn_count(registry: &EntityRegistry) -> usize {
    registry
        .iter_with_kind(ComponentKind::PlayerMovement)
        .count()
}

/// A single `player_spawn` placement the install path would route through
/// `spawn_from_player_starts` (default `entity_class` → `"player"`).
fn player_start() -> MapEntity {
    MapEntity {
        classname: "player_spawn".to_string(),
        origin: Vec3::new(0.0, 1.21, 0.0),
        angles: Vec3::ZERO,
        key_values: HashMap::new(),
        tags: vec![],
    }
}

// --- The gate's truth table. The install path suppresses the boot player spawn for
// a connected client ONLY; single-player and the listen host keep theirs. ---
#[test]
fn boot_spawn_suppressed_for_connected_client_only() {
    use std::net::{Ipv4Addr, SocketAddr};

    let addr: SocketAddr = (Ipv4Addr::LOCALHOST, 7777).into();
    assert!(
        role_suppresses_boot_player_spawn(&NetRole::Connect { addr }),
        "a connected client must defer its pawn to the host baseline"
    );
    assert!(
        !role_suppresses_boot_player_spawn(&NetRole::SinglePlayer),
        "single-player must keep its boot pawn"
    );
    assert!(
        !role_suppresses_boot_player_spawn(&NetRole::Host { port: 7777 }),
        "the listen host must keep its own / authoritative boot pawn"
    );
}

// --- Post-fix integrated invariant: the gated client owns ZERO PlayerMovement pawns
// until the host baseline arms EXACTLY ONE, and the marked local pawn never switches
// entity (there is no boot pawn to switch from). Drives the real arm path. ---
#[test]
fn connected_client_owns_no_pawn_until_baseline_then_exactly_one() {
    let mut h = LoopbackHarness::new(light_link());

    // Pre-arm: the gated install path spawned NO boot pawn, so the client registry
    // holds zero PlayerMovement pawns and no local-player marker (only the bystander).
    assert_eq!(
        player_movement_pawn_count(&h.client_registry),
        0,
        "a gated connected client owns no PlayerMovement pawn before its baseline"
    );
    assert_eq!(
        h.client_registry.local_player_pawn(),
        None,
        "no local-player marker exists before the host baseline arms one"
    );

    // Drive the real apply/arm path until the host's `local_player` baseline arrives.
    let steps = h.step_until_armed(&forward_command(false));
    let armed_pawn = h
        .client_pawn
        .expect("the host baseline armed the local pawn");
    assert!(steps > 0, "arming took at least one round trip");

    // Post-arm: EXACTLY ONE PlayerMovement pawn, and the marked local pawn IS the net
    // pawn (no boot→net entity switch — there was no boot pawn).
    assert_eq!(
        player_movement_pawn_count(&h.client_registry),
        1,
        "after arming, the connected client owns exactly one PlayerMovement pawn"
    );
    assert_eq!(
        h.client_registry.local_player_pawn(),
        Some(armed_pawn),
        "the marked local pawn is the host-armed net pawn, not a boot pawn"
    );
}

// --- Fail-before evidence, captured as a live assertion: if the install boot spawn
// HAD run on a connected client (the pre-fix behavior), the invariant is violated —
// two PlayerMovement pawns coexist and the local-player marker lands on the boot pawn
// (a DIFFERENT EntityId than the net pawn the baseline later arms). This is exactly
// what the gate prevents; running the spawn here reproduces the bug so the regression
// is anchored to observable state, not just the gate boolean. ---
#[test]
fn pre_fix_boot_spawn_would_create_a_second_pawn_and_steal_the_marker() {
    let mut h = LoopbackHarness::new(light_link());
    let descriptors = entity_descriptors();

    // Reproduce the un-gated install behavior: spawn a boot pawn into the client
    // registry exactly as `spawn_from_player_starts` does at level install.
    let starts = [player_start()];
    let result = spawn_from_player_starts(&starts, &descriptors, &mut h.client_registry, None);
    assert_eq!(result.spawned, 1, "the boot spawn materialized a pawn");

    // The boot pawn is a PlayerMovement pawn and it grabbed the local-player marker.
    assert_eq!(
        player_movement_pawn_count(&h.client_registry),
        1,
        "the un-gated boot spawn created a PlayerMovement pawn (the bug's first pawn)"
    );
    let boot_pawn = h
        .client_registry
        .local_player_pawn()
        .expect("the boot spawn marked itself local (the symptom)");

    // Now drive the host baseline arm. The bug: the baseline arms a DIFFERENT entity,
    // so the client ends up owning TWO PlayerMovement pawns and the camera-followed
    // marker SWITCHES from the frozen boot pawn to the net pawn.
    h.step_until_armed(&forward_command(false));
    let net_pawn = h.client_pawn.expect("the baseline armed a net pawn");

    assert_ne!(
        net_pawn, boot_pawn,
        "the host baseline arms a DIFFERENT EntityId than the boot pawn (the entity switch)"
    );
    assert_eq!(
        player_movement_pawn_count(&h.client_registry),
        2,
        "un-gated: a boot pawn and the net pawn coexist (the orphaned second pawn bug)"
    );

    // The descriptor table is shared content (silences the unused-import lint in the
    // ENTITY_CLASS re-export and documents the class the boot/net pawns share).
    assert_eq!(ENTITY_CLASS, "player");
}
