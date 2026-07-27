# Research — Session Lifecycle

Findings behind the spec's decisions, including why its scope changed once.

## Code grounding

| Claim | Source |
|---|---|
| The app gate early-returns until a fingerprint is installed, so every handshake queues until a level installs — the ordering inversion this spec fixes | `crates/net/src/transport.rs` — `process_control_messages` |
| A fingerprint change closes every slot and retains the close for the next poll; the client half disconnects itself | `crates/net/src/transport.rs` — `NetServer::set_kinematic_static_fingerprint`, `NetClient::set_kinematic_static_fingerprint` |
| The client sends its handshake exactly once, gated on a fingerprint being present, and never re-arms | `crates/net/src/transport.rs` — `NetClient::update`, `handshake_sent` |
| Slot states are `Pending`/`Accepted`/`Closed`; `Closed` is terminal and only an `Accepted → Closed` transition emits an event | `crates/net/src/slots.rs` — `SlotState`, `SlotTable::on_close` |
| Entity state is gated on accepted slots at the send call | `crates/net/src/transport.rs` — `send_snapshot`, `accepted_clients` |
| An app-level reject closes the slot and disconnects in the same call, so no reliable message can reach the peer first | `crates/net/src/transport.rs` — `NetServer::reject` |
| The handshake carries three fields and is `Copy` | `crates/net/src/wire.rs` — `ProtocolVersion` |
| Both protocol constants are hand-bumped and packed into the transport gate | `crates/net/src/transport.rs` — `PROTOCOL_ID`, `WIRE_VERSION`, `transport_protocol_id` |
| The server→client envelope carries time-sync and shot verdicts only — there is no relevel vocabulary | `crates/net/src/wire.rs` — `ServerMessage` |
| The fingerprint hashes the mover list, mover collision vertices/indices, and waypoints — nothing identifying the level. Signature is `kinematic_static_fingerprint(geometry: &KinematicGeometry) -> [u8; 32]`, `pub(crate)`, and its `FINGERPRINT_EPOCH` is a **function-local** `const` = 1, not a shared one | `crates/postretro/src/runtime_movers.rs` — `kinematic_static_fingerprint` |
| Its hash helpers — `hash_len`, `hash_str`, `hash_vec3`, `hash_f32` — are **private free functions** in that same file. A sibling recipe module cannot call them without a visibility change | `crates/postretro/src/runtime_movers.rs` |
| The level's own identity is resolved one line before the fingerprint is computed, on the same `&mut self` | `crates/postretro/src/startup/lifecycle.rs` — `retain_active_level_tags_for_install` sets `active_level_source`, then `install_level_payload` computes the fingerprint |
| A catalog load resolves against the engine-global map registry, which survives level unload — so one string is enough to name a map on both peers | `context/lib/boot_sequence.md` §4, §6 |
| The level-scoped client reset early-returns for the host role, so no host-side table is cleared on unload | `crates/postretro/src/netcode/mod.rs` — `reset_level_scoped_client_state` |
| The net endpoint is advanced only from the Running gameplay block; the loading frame polls the level worker and paints the splash | `crates/postretro/src/main.rs` (snapshot-apply stage), `crates/postretro/src/startup/lifecycle.rs` (loading frame) |
| The manifest requires `name` and carries no id or version; four parse sites read it (two runtimes × initial/staged) | `crates/scripting-core/src/runtime/mod_init_exec.rs`, `crates/scripting-core/src/staged_manifest.rs` |
| The net endpoint is built during `Session::build`, and mod init runs after — so mod identity cannot be a construction argument | `context/lib/boot_sequence.md` §1 |
| The accept lane spawns the slot pawn; `lifecycle` carries closes only | `crates/postretro/src/main.rs` — the `HandshakeOutcome` match, `host_handle_lifecycle` |
| The client's local static-collision trimesh is built from `LevelWorld` vertices and indices, and nothing hashes them — the second, larger fail-open | `crates/postretro/src/collision/mod.rs` — `CollisionWorld::populate_from_level` |
| Client movement prediction runs against that local collision source, and client-authoritative hit declaration casts against the world the client renders while the host validates against its own static geometry | `MovementCollisionSource` is defined `pub(crate) trait` in `crates/postretro/src/movement/mod.rs`; `crates/postretro/src/netcode/prediction.rs` consumes it as `&impl MovementCollisionSource`, `context/lib/networking.md` §Combat authority |
| Player movement tuning is descriptor-authored, so a manifest edit changes what the client predicts with | `crates/postretro/src/movement/mod.rs` — `PlayerMovementComponent::from_descriptor` |
| Clients suppress AI-enemy spawns entirely and attach mesh presentation only, which is why enemy placement and brain tuning cannot break compatibility | `context/lib/networking.md` §Phase boundaries |
| A staged reload re-commits nearly every manifest lane — `entities`, `store_declarations`, `maps`, `reactions`, `crossings`, `trigger_events`, `trigger_pools`, `events`, the `render` profile, `ui_trees`, `theme`, `frontend`. `fonts` is the only lane never re-committed; it is absent from `StagedManifest`. So a non-atomic-replace manifest lane already ships, and it is exactly one | `crates/scripting-core/src/staged_manifest/transfer.rs`; `commit_staged_manifest_result` in `crates/scripting-core/src/runtime/core.rs`; `App::poll_staged_manifest_results` in `crates/postretro/src/startup/staged_manifest_lifecycle.rs`; `App::commit_staged_ui_manifest` in `crates/postretro/src/main.rs` |
| Most of `EntityTypeDescriptor` is host-authoritative (`health`, `weapon`, `default_weapon`, `behavior`) or presentation (`light`, `emitter`, `mesh`); only `canonical_name` and `movement` feed client simulation | `crates/entities/src/data_descriptors/types/entity.rs` — `EntityTypeDescriptor` |
| `EntityTypeDescriptor` has **9** fields, not the 10 an earlier draft enumerated: `ai` was retired by `E10--retire-legacy-ai`, landing on main after this spec's digest table was written. `behavior` (`BehaviorGraphDescriptor`) is the surviving host-authoritative AI mechanism | `crates/entities/src/data_descriptors/types/entity.rs` |
| State-slot parity already ships: both peers compare a schema fingerprint derived from the replicated slot declarations | `crates/postretro/src/netcode/state_slots.rs` — `ReplicatedSlotSchema` |
| Scripts are 160K of the dev mod's 337M (textures 291M, models 43M, maps 3.0M), so script sync is cheap on bytes and still does not cover the breaking surface | measured under `content/dev/` |
| `PlayerMovementDescriptor`'s hashed fields are structs, not scalars: `CapsuleParams`, `GroundParams` (which nests `SpeedParams`), `AirParams`, `FallParams`, `DashParams`, `ForgivenessParams`, `CrouchParams`. `view_feel: Option<ViewFeelParams>` is the render-only one | `crates/foundation/src/data_descriptors/types/movement.rs` |
| `DashParams` carries five `NumberOrIr` (`boost_speed`, `momentum_retention`, `steer_control`, `dash_drag`, `cooldown_ms`), one `BoolOrIr` (`preserve_vertical`), and `air_dashes: u32` — so `movement` reaches the IR | same file |
| `NumberOrIr` / `BoolOrIr` are `enum { Literal(..), Ir(IrNode) }`. Their doc comments scope them to dash *today*, and they sit in the shared typedef surface | same file; `crates/scripting-core/src/typedef/common.rs` |
| `IrNode` has **15** variants (`Const`, `Input`, `Add`, `Sub`, `Mul`, `Div`, `Clamp`, `Lerp`, `Lt`, `Le`, `Gt`, `Ge`, `Eq`, `Ne`, `Select`) and is tree-recursive through `Box<IrNode>`; `IrValue` is `Bool(bool)` / `Number(f32)`. The module doc names movement as **"the first adopter"** of the substrate | `crates/foundation/src/ir/mod.rs` |
| `HealthDescriptor::zone_multipliers` is a `std::collections::HashMap<String, f32>`, but it is reachable only through `EntityTypeDescriptor::health`, which the digest skips as host-authoritative — so **no map-valued field is reachable in the hashed domain** | `crates/foundation/src/data_descriptors/types/combat.rs` |
| `serde_json`'s `preserve_order` feature is **not** enabled in this workspace, so `serde_json::Map` is a `BTreeMap` and iterates key-sorted deterministically | workspace `Cargo.toml` |
| `EntityId` is a newtype over `u32` — a runtime allocation handle, not content | `crates/entities/src/registry.rs` |
| `TriggerEventDescriptor { tag, event, fire, levels }` is `String`/`Vec<String>` throughout and already derives `Hash`; `TriggerPoolDescriptor { tag, arm, levels }`, with `TriggerPoolArm::{Count(u32), Percentage(f64)}` | `crates/entities/src/data_descriptors/types/reactions.rs` |
| `CrossingDescriptor { slot: Option<String>, condition, max: f32, edge: Option<String>, fire: Vec<String> }`, with `CrossingCondition::{Below { threshold }, Above { threshold }, Ir(IrNode)}` — the same `IrNode` `movement` reaches | same file |
| `PrimitiveDescriptor { primitive: String, target, tag, on_complete, args: serde_json::Value }`; `SequenceStep { id: SequenceTarget, primitive: String, args: serde_json::Value }`; `SequenceTarget::{Entity(EntityId), Activators, FiredTrigger}` | same file |
| `DataRegistry` keeps per-level `reactions`/`crossings`/`trigger_events`/`trigger_pools`, committed by `setupLevel()`, separate from mod-global `global_reactions`/`global_crossings`/`global_trigger_events`/`global_trigger_pools`, committed from the manifest | `crates/entities/src/data_registry.rs` |
| The control decode skips clients already accepted — `if self.slots.is_accepted(client_id)` — under a comment reading "A client already accepted may send later control traffic" | `crates/net/src/transport.rs` — `NetServer::process_control_messages` |
| The `defineMod` declaration and its doc comment are **hand-written templates**, not registry-generated | `crates/scripting-core/src/typedef/templates/sdk_lib.d.ts`, `sdk_lib.luau` |

## Slot lifecycle

The change in one picture. Today's `Accepted` splits into two states, and a level change
becomes a demotion rather than a close — which is what lets a connection outlive the map
it joined on.

```mermaid
stateDiagram-v2
    [*] --> Pending: transport connect
    Pending --> Admitted: admission matches<br/>(protocol constants + mod id)
    Pending --> Closed: admission mismatch<br/>(typed reason sent, disconnect deferred one poll)
    Admitted --> Participating: content parity matches<br/>(mod digest + level identity + level digest)
    Admitted --> Admitted: parity mismatch —<br/>no state flows, connection survives
    Participating --> Admitted: host replaces the parity triple<br/>(level install, or staged commit moving the mod digest;<br/>demotion runs the existing close cleanup)
    Admitted --> Closed: disconnect or timeout
    Participating --> Closed: disconnect or timeout
    Closed --> [*]
```

Three properties fall out and become acceptance criteria: no entity state reaches a slot
below `Participating`; a demotion runs the same per-slot cleanup a close runs, because
level unload invalidates every id those tables hold; and the slot survives the whole
transition, which is only true if the transport is polled across it.

**The self-loop is the load-bearing edge.** `Admitted → Admitted` on a parity mismatch —
rather than `Admitted → Closed` — is what makes the two gate stages structurally different
rather than merely sequential. Admission facts can never become true later, so a mismatch
there closes. Parity values are all designed to become true later, so closing on one is a
category error, and it would race the spec's own criteria: a client's parity for level A can
still be in flight when the host installs level B, and a host that closed on mismatch would
tear down a client it demoted one frame earlier. The first draft of the index carried the
content cause in the reject-and-close lane beside protocol and mod, contradicting this
diagram; direction review caught it, and the index now matches. Consequence worth noting:
the deferred-disconnect mechanism serves admission only, which shrinks the spec's own
"trimmable part."

**Which lane a value belongs in is decided by mutability, and the first draft got one
wrong.** The rule that survives is: admission carries a value only if a mismatch on it can
never become a match. That is true of the protocol constants, and true of the mod id because
identity is frozen at first commit. It was *not* true of the mod compatibility digest, which
the first draft nonetheless gated at admission — a staged reload re-commits `entities`, which
the digest reads, so its value changes under a live connection. The draft made its own premise true by declining to observe the change, freezing
the digest at first commit, which bought the premise at the price of gating live connections
on a stale value in exactly the builds where co-op is developed and playtested.

The trap was framing the choice as freeze-versus-rehash-and-close. Both options are bad, and
the spec's own new mechanism supplies a third: **rehash and demote**. That option only exists
because this spec invents a state to demote *to*, which is why the first draft could not see
it — it was reasoning with the vocabulary the shipped code had. Worth recording as a pattern:
when a spec adds a state, re-examine every decision it made before the state existed.

The dev-loop objection that killed a whole-mod content hash does not transfer to the
rehashing digest, and the distinction is worth keeping straight. That hash moved on every
byte of every script and its consequence was a **closed** connection; this one moves only
when a hashed field changes and its consequence is a **hold** that resolves the moment the
peers agree again. Same mechanism, two orders of magnitude apart in both trigger frequency
and blast radius.

## Why this merged with the level-transition spec

Drafted first as admission alone, with server-authoritative level transitions as a
separate follow-on. Direction review returned *under-scoped*, and the evidence was
entirely self-generated:

- The admission draft pulled the fingerprint fail-open fix forward from the transition
  spec, on the argument that a deferred fix making its own invariant fail silently is not
  deferrable.
- It then left the unpolled unload→install window in the transition spec — the identical
  case, unapplied. Its two headline criteria ("the connection outlives the map", "a
  demoted client re-participates without reconnecting") are both claims about surviving
  that window.
- The transition spec was already described as likely to split, along a seam different
  from the one dividing it from admission.

Three signals that the work divides by *layer* — gate, wire, engine lifecycle — not by
*capability*. Merging removes the seam; the task breakdown supplies the structure the two
specs were providing.

## Why level identity is a field, not more hash

The fingerprint covers mover authoring and collision, so two maps with no movers hash
identically: the gate passes, no cleanup runs, and clients stay attached to a host where
their pawns no longer exist. Bound once per connection that is a latent bug; evaluated at
every level install it becomes this spec's central invariant failing silently.

The first draft widened the hash domain to include the level's identity. Rejected on
review, and the rejection is right: the fail-open is an **identity** failure ("different
maps"), not a **content parity** failure ("prediction inputs diverge"). Fixing the first
by making a content hash accidentally identity-sensitive mixes two questions into one
opaque value. Two questions, two fields:

- Carrying identity separately makes the common mismatch readable — "the host is on
  `city-03`, you are on `city-04`" rather than a 32-byte diff. The typed reject reason's
  whole point is an actionable expected-vs-received payload.
- The two answer different questions. Identity is "which map"; the digest is "is the
  content the same". Keeping them apart is what lets the digest's domain be widened later
  on its own merits — which this spec then does, adding static world collision — without
  that widening being confused for an identity fix.
- The relevel message names a catalog id. Once parity already carries the host's level
  identity, relevel is adding a *direction* to a value the protocol moves, not a new noun.

Hashing the `.prl` bytes was the other candidate and stays rejected under both shapes:
strictly stronger, and it makes a cross-platform bake difference a hard connection
failure — a bake-determinism question this spec has no standing to answer.

## What widening the fingerprint retired

Adding static world collision to the parity digest closed a hole, and it also weakened an
argument this spec had been leaning on. Recorded because the swap should be visible.

The merge was partly argued on catalog ids being mod-scoped: two mods can declare the same
map id over different `.prl` files, so two peers on different mods compare level identity
equal — and, if both maps were mover-less, compare fingerprints equal too. The second half
of that no longer holds. Differing brushwork now diverges on content regardless of whether
the mods match.

What replaces it is stronger, not weaker. The two digests are halves of one policy computed
at the two moments the spec already installs values, and neither covers for the other: a mod
fork that retunes player movement ships identical map bytes and moves only the mod digest;
differing brushwork moves only the level digest. That is a structural reason they belong in
one spec, where the earlier argument was a contingent one about a specific hole.

## What shrinking the mod digest retired

Detail review cut the mod digest's domain from two whole manifest lanes to, per entity type,
the `canonical_name` and the `PlayerMovementDescriptor` under `movement`, minus that
descriptor's render-only `view_feel` field. Unnamed descriptors are excluded. Recorded for
the same reason as the section above: an argument was retired, not just a number.

Hashing `entities` wholesale contradicted the spec's own tiering. Most of
`EntityTypeDescriptor` is host-authoritative — `health`, `weapon`, `default_weapon`,
`behavior` — and clients suppress AI-enemy spawns entirely, so those fields are Tier 2 in
`context/research/coop-content-compatibility.md`. The rest — `light`, `emitter`, `mesh` — is
presentation. Hashing the lane would have demoted every peer on an enemy retune or a light
tweak: the same failure mode the spec rejects a declared mod version for, reintroduced by
the mechanism meant to replace it.

State-slot parity left the digest in the same pass. It is owned by the shipped
`ReplicatedSlotSchema` fingerprint, which both peers already compare, so hashing
`store_declarations` would have duplicated a live gate with a coarser diagnostic.

The denylist mechanism survives; its granularity moved. It is no longer "hash every lane
except the named exclusions" but "hash every field of a small named set of descriptor types
except the named exclusions." Exhaustive destructuring still makes a field added later fail
the build rather than default to unhashed, which is the whole reason to prefer a denylist —
provided the destructuring reaches all the way down. It has to be stated at the right depth:
the values it protects sit one and two levels below the two types the spec names, inside
`DashParams` and the `IrNode` beneath it. What it no longer buys is coverage of descriptor
*types* nobody named — a new client-simulated descriptor is still a manual widening, caught
by the rule in `context/research/coop-content-compatibility.md` §6 rather than by the compiler.

## What generalizing the IR hasher bought

The spec's original framing treated `DashParams`'s IR-valued fields as an obstacle: the
movement descriptor was assumed to be scalars, and it is not. That framing had the direction
of travel wrong. The IR is a substrate mid-adoption — its module doc calls movement "the first
adopter", `E18--ir-valued-reactions` shipped, and `E10--enemy-stagger` is a planned adopter
currently deferred on `CombatScope`: it lists IR-authored stagger tuning as out of scope,
shipping plain descriptor scalars upgradeable additively to `NumberOrIr` per the dash precedent
once Epic 16's `CombatScope` lands, not a spec drafting against the wrappers today. A recipe
shaped around dash specifically would be correct for exactly one
adoption step, and would then fail open on the next one, silently, on a field added by someone
who never read the recipe. That is the same failure the static-collision hole already
demonstrated.

Building a general `IrNode`/`IrValue` walker instead paid for itself immediately.
`global_crossings` carries `CrossingCondition::Ir(IrNode)` — the same type — so covering that
lane became nearly free once the walker existed.

**The design rule that fell out: hash the IR structurally, not by serializing it.**
Serializing is the tempting shortcut. `IrNode`'s serde format is pinned and byte-matched, so a
hash over the serialized form would be stable *and* would auto-cover every variant added
later. That auto-coverage is the defect. A new variant the walker does not handle is a compile
error; a new variant a serializer handles is nothing at all. The compile error is the entire
mechanism, and serializing trades it away.

## What the digests do not cover

An earlier draft named five knowingly-uncovered manifest lanes: `reactions`, `crossings`,
`events`, `trigger_events`, and `trigger_pools`. Scoping brought three inside the mod digest.
`global_trigger_events` and `global_trigger_pools` are `String`/`Vec<String>` plus one small
enum; `global_crossings` came with the IR walker. Two remain: **`reactions` and `events`**.

The reason previously recorded for deferring them — that reactions carry "their own
IR-encoding question" — is wrong. They do carry IR, and it is the question the walker already
answers. The two real blockers are different:

- **`SequenceTarget::Entity(EntityId)` is a runtime handle**, a newtype over `u32` the registry
  hands out, not content. Two peers on byte-identical mods can hold different values there.
- **Prediction relevance is keyed by `PrimitiveDescriptor::primitive`, an open string
  namespace** rather than a struct shape. This is the decisive one. Every other digest domain
  is decided by exhaustive destructuring, which turns an unclassified addition into a compile
  error. A string key admits no such error: a primitive added later is just a string the recipe
  has never seen.

Both ways out are mechanisms this spec already rejected. Hashing all reactions wholesale
reintroduces the Tier 2 false refusal the disposition table exists to prevent. Hashing an
allowlist of prediction-relevant primitives is the allowlist mechanism that produced the
static-collision fail-open. The `serde_json::Value` payloads are **not** the blocker:
`preserve_order` is off in this workspace, so `serde_json::Map` is a `BTreeMap` and iterates
key-sorted.

**A gap nobody had recorded: level-local reactions and crossings.** Scripts declare reactions
and crossings from `setupLevel()`, and `DataRegistry` keeps those per-level lanes separate from
the mod-global ones the manifest commits. Neither digest reaches them. The mod digest reads the
`global_*` lanes; the level digest hashes `.prl` geometry and mover data, which is not where
script-declared content lives. So two peers running the same map under mods whose level scripts
differ agree on both digests and diverge on locally-simulated behavior. Named here on the same
rule that makes the uncovered lane set explicit at all.

**Tier 3 item 5 changed disposition: replicate, not hash.** World gravity set from a reaction
was cited as the concrete cost of the uncovered reaction lanes. Hashing is the wrong instrument
for it. Gravity is world state the server owns, so it is absorbed by server authority — Tier 2
under the project's own model — and replication makes peers *agree*, where a digest only makes
them refuse when they do not. The E15 spec names it out of scope rather than implementing it.

## Which corroborating documents are independent, and which are not

Direction review flagged this and it belongs on the record, because the next reviewer will
otherwise read agreement where there is only restatement.

Three documents now state that co-op compatibility is decided by content rather than by a
declared version: this spec, `context/research/coop-content-compatibility.md`, and
`context/research/coop-session-lobby.md` §4. The third is **not** independent support. Before commit
`f9a8973` it said the opposite — "the manifest declares an id and a version; the client sends
them at admission; the host compares" — and it was rewritten in the same commit that made the
decision. The roadmap's Phase 3.75 sub-bullet (line ~201) was rewritten in that commit too.

So the honest inventory is: one argument (in `context/research/coop-content-compatibility.md`), stated in three
places. It is a good argument and it survives review on its merits, but a reader must not
count it three times. Two corollaries for anyone validating this spec later:

- The roadmap's **Phase 3.75 paragraph** and the three-spec decomposition are owner-set and
  are legitimate external referents. The **sub-bullet for this spec** is not — it is drafter
  output and tracks the draft.
- The genuinely independent evidence for the policy is in the code, not the prose: the
  suppression list in `networking.md` §Phase boundaries (which is what makes Tier 2 large)
  and `CollisionWorld::populate_from_level` (which is what makes Tier 3 item 1 real).

## Drafting errors, and the pattern behind each

Three from this review round. The correction is cheap; the pattern is the part worth keeping.

- **The spec claimed its chosen digest domain avoided the IR enum.** It does not. `movement`
  reaches `dash`, and `DashParams` reaches the same `IrNode` the spec cites elsewhere as a
  reason to leave state-slot parity to `ReplicatedSlotSchema`. The pattern: *a rationale for
  choosing between two designs was written without opening the types the chosen one reaches.*
  This is the third consecutive review in which Task 6 failed on an unopened type.
- **The cross-process determinism criterion, and the invariant supporting it, were anchored to
  `zone_multipliers`** — a field the spec's own disposition table excludes, since it is
  reachable only through host-authoritative `health`. The pattern: *an example survived a
  domain change that removed it from the domain.*
- **The exhaustive-destructure guarantee was stated over `EntityTypeDescriptor` and
  `PlayerMovementDescriptor`**, when the values it protects live one and two levels below
  them. The pattern: *a guarantee was scoped to the types the recipe names rather than the
  types it reaches.*

## Rejected while drafting

- **Telling the client only that it was admitted.** Superseded by the merge: the relevel
  message is the useful form, and a bare admitted-acknowledgement would have been
  redesigned into it immediately.
- **Semver ranges on the mod version.** Invites a compatibility policy with no way to
  test it. Exact string match; the author bumps the version when the contract changes.
- **A content hash over the *whole* mod.** Breaks hot reload mid-session, makes legitimate
  client-side differences fatal, and buys tamper detection, an explicit non-goal
  (`index.md` §4). Superseded rather than simply rejected: the spec now hashes a *scoped*
  surface — per entity type, the `canonical_name` and the movement descriptor a client
  predicts with — which keeps the compatibility property and drops the breakage. Note the two
  independent reasons the breakage is gone, since collapsing them invites a bad inference: the
  domain shrank from every byte to two fields per named entity type, *and* the consequence
  softened from close to demote.
  Either alone would leave a usable dev loop; neither is a reason to widen the domain back. `E16--impact-policy-substrate`'s
  rule (explicit author-assigned ids, not content-derived ones) still governs **identity**;
  it never governed parity, which has been content-derived since the fingerprint shipped.
- **Exact mod-version equality as the admission gate.** The first draft's rule, dropped
  after the tiering analysis in `context/research/coop-content-compatibility.md`. It refuses on a
  value that does not track the breaking surface: an author who edits a light bumps it and
  blocks a friend, and an author who retunes player movement may not bump it at all. The
  version is still required and still crosses the wire — for display, never for comparison.
- **Reusing `ProtocolVersion` with the mod fields appended.** It is `Copy` and the mod id
  is a string. More importantly the two stages fire at different times, so one message
  re-creates the ordering inversion the spec exists to remove.
- **Making the relevel message carry a path rather than a catalog id.** A path is
  machine-local and resolves against a filesystem the peer does not share. The catalog is
  the only namespace in which one string resolves on both peers. The catalog's *mod-scoped*
  half of that argument — two mods may declare the same map id over different `.prl` files,
  so level identity does not discriminate until admission has proven the mods match — moved
  into the index's Decisions, beside level identity, because it is the argument that carries
  the merge rather than a note about message payloads.
