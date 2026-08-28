# Sprite PNG-Decode Retirement

## Goal

Retire the runtime PNG-decode-and-stitch path for sprites so that **every** sprite
— map-placed `billboard_emitter` collections, data-script descriptor-spawned
sprites (projectiles, trails), the hardcoded weapon/engine effects, and
single-`.png` references — renders from a baked, mipped `.prm`, sampled through
one path. Today sprites split across two rendering paths: `billboard_emitter`
collections can be baked (via `billboard-sprite-prm-baking`), while everything
else decodes a single-mip PNG at load through `load_sprite_frames`. Two paths for
one asset class is incoherent, blocks uniform mip quality, and — because the
specular-shimmer model classifies a sprite by its baked `NORMAL` slot — leaves
descriptor/projectile sprites unable to ever shimmer. This spec makes baked PRM
the sole sprite path and removes the runtime decode.

## Status

**Direction-capture draft.** The central design decision (how the build
identifies the set of sprite assets to bake) is unresolved and load-bearing; it
is written up under *Direction* and *Open questions* rather than settled. Run
`/validate-plan sprite-png-retirement` before detail review — the discovery-model
choice is a direction call, not a detail.

## Scope

### In scope

- A build-time bake pass that produces a `.prm` sidecar for **every sprite asset**
  the build can see, reusing the `bake_sprite_collection` entry, the content-hash
  contract, and the PRM-load runtime path established by
  `billboard-sprite-prm-baking`. Single-`.png` sprites bake as one-frame
  collections (N=1); `_NN` directories bake as multi-frame collections; optional
  spec/normal companions ride along as in the prerequisite.
- Retiring `load_sprite_frames`, `load_collection_frames`, and the runtime
  `SpriteFrame` decode/stitch/upload path from the draw path. The runtime
  content-hashes a sprite reference and loads its sidecar; a reference with no
  baked sidecar is a **content error** — the 1×1 white placeholder plus one
  warning — not a silent runtime decode.
- Whatever content reorganization and reference updates the chosen discovery
  model requires (see *Direction*), applied to the base game's dev content.
- Documentation of the single sprite path in `context/lib/` and the
  authoring-facing rule for where sprite assets live.

### Out of scope

- **The `billboard_emitter`-collection bake mechanics** — the bake entry, hash
  contract, geometry validation, per-frame mips, and PRM-load runtime — are
  built by `billboard-sprite-prm-baking`. This spec extends *which* assets that
  machinery is pointed at and removes the fallback; it does not re-derive the
  bake.
- **The shimmer shader path.** Owned by `billboard-specular-shimmer`. This spec
  only ensures descriptor/projectile sprites *can* carry baked slots so they
  become eligible for shimmer.
- **Changing the `sprite` primitive's authored contract.** A modder still writes
  `sprite: "projectiles/bolt.png"` or `sprite: "smoke"`; both keep resolving.
  This spec changes how those references are *served* (baked, not decoded), not
  how they are *authored* — unless the discovery model requires a content-root
  convention, in which case the authored path prefix changes and the primitive
  docs update in the same pass (`crates/postretro/src/scripting/primitives/mod.rs`).
- **Runtime baking / bake-on-first-load.** An explicit engine non-goal; sprites
  are baked at build time like every other texture.

## Direction

**Problem.** Retiring the runtime decode path requires that every sprite asset be
baked at build time. But the build cannot enumerate every sprite *reference*:
descriptor-spawned sprites are produced by evaluating data scripts, and the
compiler embeds only compiled bytecode (`DataScriptSection` carries
`compiled_bytes` + `source_path`, not extracted sprite fields), so a sprite name
a data script computes is invisible to a reference-discovery pass. Reference
discovery — the model `billboard-sprite-prm-baking` uses for `billboard_emitter`
placements — therefore cannot cover the sprites this spec must bake. The bake
must be driven by asset *presence*, not by reference.

**The load-bearing decision — how the build identifies sprite assets.** Sprites
are not in a dedicated tree: `texture_root = content_root/textures`
(`crates/postretro/src/startup/lifecycle.rs`) intermixes sprite collections
(`smoke_puff/smoke_puff_00.png`, `plasma_bolt/plasma_bolt_00.png`) with world
texture packs (`neon/`, `metal/`, large third-party packs). A content scan cannot
tell a sprite collection from a world texture by location alone. One reduction
helps: a single-frame sprite `.png` bakes to the *same* content-addressed diffuse
`.prm` as any other single image, so only **multi-frame `_NN` collections** need
sprite-specific stitching, and those are identifiable by the
`<dir>/<dir>_NN.png` pattern. Loose single-`.png` sprites referenced only by
descriptors remain the hard case.

Three candidate models (Q6 alternatives, to be judged by `/validate-plan`):

- **A — Dedicated sprite content root (recommended).** Require every sprite asset
  under `textures/sprites/`; the bake pass scans only that subtree (both
  `<name>/<name>_NN.png` collections and loose `<name>.png` frames). Bounds the
  scan, guarantees every sprite — descriptor loose-PNG included — is baked, and
  gives modders one clear rule. Cost: a one-time content reorg (move
  `smoke_puff/`, `plasma_bolt/`, `projectiles/`, … under `sprites/`) and a
  reference/primitive-doc update. This is the "aggressive, all-in" move.
- **B — Collection-pattern scan + reference-discovered loose PNGs.** Scan
  `textures/` for `<dir>/<dir>_NN.png` collections; bake loose single-`.png`
  sprites only where a reference is build-visible. Rejected: reintroduces the
  discovery gap for descriptor-referenced loose PNGs — the exact hole this spec
  exists to close.
- **C — Bake every image under `textures/`.** Simplest scan; bakes the whole
  texture library (world packs included) as diffuse `.prm`, stitching only
  `_NN` collections. Rejected pending cost analysis: bakes large unused
  third-party packs, inflating the prm-cache and build time; world textures are
  already baked by their own reference-discovery path, so this double-covers
  them.

**Prior commitments.** `billboard-sprite-prm-baking` establishes the bake entry,
the sprite content-hash contract, and the PRM-load runtime — this spec consumes
them unchanged. "Baked over computed" and "runtime baking is a non-goal" (index
architectural principles) both point at a build-time bake, not a runtime one. The
`sprite` primitive is a documented modder contract
(`primitives/mod.rs`); model A changes its authored path prefix, which per the
"primitive surface is a contract" invariant requires updating the SDK docs and
any validation in the same pass — called out so the reviewer weighs that cost.

**Alternatives rejected.** Keeping a runtime decode fallback for un-baked sprites
(the safe half-measure) is rejected by the goal itself: a sometimes-baked,
sometimes-decoded asset class is the incoherence this spec removes, and it leaves
descriptor sprites permanently unmippable and unable to shimmer. Reference
discovery (model B) is rejected as above.

## Acceptance criteria

(Provisional — sharpen after the discovery model is chosen.)

- [ ] Every sprite reference exercised by `content/dev` — a `billboard_emitter`
      collection, a descriptor-spawned projectile/trail, a single-`.png`
      reference, and the weapon-impact effect — resolves to a baked `.prm` at
      runtime and renders mipped, with no call into the PNG decode path.
- [ ] `load_sprite_frames`, `load_collection_frames`, and the runtime
      `SpriteFrame` decode/upload path are removed from the draw path; grepping
      the renderer and startup crates finds no remaining caller on the sprite
      draw path.
- [ ] A sprite reference with no baked sidecar renders the 1×1 white placeholder
      and logs exactly one warning naming the missing asset — no panic, load
      continues.
- [ ] The `content/dev` regression that previously guarded direct-`.png`
      rendering (`crates/render-cpu/src/fx/smoke.rs` test) is updated to assert
      the reference now renders from its baked sidecar rather than the
      placeholder.
- [ ] A distant projectile/trail sprite no longer shimmer-crawls — the coarse
      mips are present and selected — the same stability `billboard-sprite-prm-baking`
      delivers for map-emitter smoke, now extended to descriptor sprites.

## Tasks

(Sketch — depends on the discovery-model decision; sequencing and paragraphs
firm up after `/validate-plan`.)

### Task 1: Sprite-asset discovery + bake pass (per chosen model)
Build the presence-driven bake: enumerate sprite assets by the chosen model
(recommended A: scan `textures/sprites/`), and call the prerequisite's
`bake_sprite_collection` per asset (single-`.png` as N=1, `_NN` as N-frame,
companions as discovered). Wire it into the build next to the existing
sprite/model bake pass.

### Task 2: Content reorganization + reference/primitive-doc update (model A)
Move sprite assets under the sprite content root, update every authored reference
(map `billboard_emitter.sprite` KVPs, data-script descriptor `sprite` fields, the
hardcoded engine references), and update the `sprite` primitive documentation
(`primitives/mod.rs`) to state the new authored path rule. Scope the reference
inventory from the sprite-reference sweep already gathered this session.

### Task 3: Retire the runtime decode path
Remove `load_sprite_frames`/`load_collection_frames`/`SpriteFrame` from the draw
path; make the runtime resolve every sprite via content-hash → sidecar, with the
white-placeholder-plus-warning content-error path as the only fallback. Update
the direct-`.png` regression test to the baked expectation.

### Task 4: Documentation
Document the single sprite path and the authoring rule in `context/lib`
(`rendering_pipeline.md` §7.4 and/or `build_pipeline.md`).

## Dependencies

- **`billboard-sprite-prm-baking`** (must land first): provides `bake_sprite_collection`,
  the sprite content-hash contract, geometry validation, and the PRM-load runtime
  this spec points at every sprite asset and whose fallback this spec removes.

## Cross-spec coordination

`billboard-sprite-prm-baking` deliberately keeps the runtime decode path as a
fallback for un-baked sprites (direct-`.png`, descriptor, weapon). This spec owns
removing that fallback and baking those sources; the two must not both claim the
retirement. `billboard-specular-shimmer` gains projectile/descriptor shimmer
eligibility only once this spec bakes their normal slots — until then shimmer is
limited to map-emitter collections. Neither dependency changes shape; this spec
widens coverage and flips the fallback.

## Open questions

- **Discovery model (A/B/C).** The central decision; A recommended. `/validate-plan`
  owns the call. Determines whether Task 2 (content reorg + primitive-doc change)
  exists.
- **Third-party/world-texture double-bake (model C only).** If C is chosen, bound
  the scan to avoid re-baking world texture packs already baked by their own
  reference-discovery path.
- **Mod sprite content.** A mod ships its own sprites; they are baked when the
  mod's content is built by the same pass. Confirm the sprite content root
  convention (model A) composes with mod content roots the way the texture root
  already does.
