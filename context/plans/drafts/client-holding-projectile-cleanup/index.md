# Client holding projectile cleanup

Status: draft follow-up. Found during E16 review; present before E16.

## Problem

Moving a client to Holding clears prediction bookkeeping and replicated entities,
but locally predicted projectiles are not replication-mapped. They remain in the
registry and can produce flight visuals, impact effects and sounds until contact
or expiry. Transport suppresses their declarations while participation is inactive;
they do not mutate local target health.

Entry points: the Holding control branch in `crates/postretro/src/main.rs`, client
demotion in `crates/netcode/src/endpoint.rs`, and predicted flight advancement in
`crates/sim/src/sim/projectile_stage.rs`. Confirmed against baseline `1c7bda3c7`.

## Scope

Retire unmapped predicted flights when active participation ends through Holding.
Preserve host shot authority and ordinary in-session projectile lifetime.

## Acceptance

- Demote a participating client with a live predicted projectile through the real
  control-message path. No old flight, impact effect, sound or declaration survives.
- Repeated demotion is harmless. A later active participation can predict new
  projectiles normally, without restoring old shots.
- Existing replicated-entity teardown and host-authoritative damage remain intact.

## Non-goals

Weapon activation changes, new wire messages, and gameplay projectile lifetime tuning.
