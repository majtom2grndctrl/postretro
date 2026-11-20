# mover-channel-bounded-bake — research

Read at 176d0bf7. Every number below is from a stop-at-gate run of release
`prl-build content/dev/maps/stress-env-volumes.map --no-cache` (default
16 GiB budget; the gate is reached in about two minutes), with the
projection and `MoverChannelCost` printed beside `price_movers`. The
printed projection matched the owner's Windows refusal byte for byte
(21,806,445,163 B).

## 1. Stress map inputs

| Input | Value |
|---|---|
| patches | 2,827,484 |
| static lights | 1,017 |
| Σ patches in reach (`light_reach_total`) | 65,930,085 |
| valid / total probes | 1,537,223 / 3,628,259 |
| movers (all re-baking radiance) | 11 |
| Σ poses (`bounce_pose_marks`) | 473 (two lifts at 140 each) |
| Σ (poses − 2) × domain (`bounce_mid_tiles`) | 853,610 |

## 2. Where the projection comes from

| Phase | Bytes | Dominant terms |
|---|---|---|
| layers | 5.92e9 | `layer_bytes` 2.64e9 (both sides × 20 B) + scratch summed over all lights |
| later bounces | 1.05e10 | patch cache + sum + buffers + held (scenes, bounce one, kept, states) |
| probe gather | 1.41e10 | as above + dense volume, section, atlas, tiles at cap |
| id-34 entry | 1.33e10 | section + 2× encoded + tiles + scenes + states |
| mover bounce | 1.99e10 | resident (incl. states 9.83e9) + kept + bounce one + `pose_bytes` 8.24e9 + sum |
| mover direct | 2.18e10 | resident + section 1.13e9 + **2 × states** 1.97e10 + 2 × direct entries 9.3e8 + solids |

`accumulated_states_bytes` = 9,828,795,944, exactly:

| Term | Formula | Bytes |
|---|---|---|
| bounce, every valid probe per mover | 11 × 1,537,223 × (2 × 216 + 56) | 8.25e9 |
| bounce marks | 1,537,223 × 473 × 1 | 7.27e8 |
| bounce mid tiles | 853,610 × 216 | 1.84e8 |
| depth records | 11 × 32,769 × (2 × 400 + 56) | 3.09e8 |
| depth marks | 32,769 × 473 | 1.55e7 |
| depth mid tiles | 853,610 × 400 | 3.41e8 |

`pose_bytes` = 8.24e9: pairs 2.64e9 (`light_reach_total` × 40 B), one
mover's raw states at bound 2.74e9, gathered + other terminus ≈ 2.3e9
(per valid probe), mid-pose bytes 3.0e8, the rest under 0.3e9.

Measured on the same map (owner, warm, private bytes every 30 s): SH volume
bake 2.17 GB peak; whole-bake peak 5.64 GB in direct-SH delta. Warm is not
proof of slack; the brief requires a cold baseline.

## 3. What the bake actually holds

- **Termini** trace every valid probe (`gather_posed_probes`) and keep a
  `PosedProbe` only where the tile changed or a ray met the pose. Posed
  later bounces reach every patch (`LaterReach::Everywhere`). Nothing
  ray-free bounds which probes change.
- **Mid poses** gather only `NearFieldDomain` probes.
- **Reduction**: `reduce_poses` keeps probes past the round-trip bar and
  the perceptual floor; raw and reduced states co-reside until the memo
  write, then the raw drop before the next mover.
- **Skips** (`skip_mid_poses`) run after every mover resolves: they never
  lower the bounce peak.
- **Direct half**: `drop(mover_bounce)` precedes DeltaSh. `bake_direct_half`
  runs after DeltaSh, DirectSh, AnimatedDirectSh, DirectShDelta, with one
  post-skip `BounceBlocks` copy (built by copying tiles out of the states)
  and one id-57 section (`finish_section` drops the provisional first).
  Alive beside it: id 34, `sh_analyze_base_indirect` (an id-34 clone),
  ids 27, 35 (+ its analyze clone), 41, 45, the entity-shadow section,
  posed solids. The direct phase prices none of the SH sections but id 34.
- **Low fidelity** (1 posed bounce): posed later-bounce gathers never run,
  but `add_posed_later_bounces` still allocates its three buffers at fill
  ≠ 0 and `PoseBaker::near_field` still builds the domain's patches and
  slots. Fidelity-aware pricing of the current allocations saves ≈160 MB.
- **Pairs**: `posed_bounce_one` skips a light failing `light_may_reach` and
  a patch failing `PosedScene::may_shadow`, both ray-free. `light_terms`
  has one entry per planned (deduped) layer; `light_reach_total` sums
  every light.
- **Layers**: a `SparseLayer` holds at most one entry per patch (Lambert
  n·l at the centroid picks the side). `shade_light_layer`'s chunk scratch
  reserves both sides. Permits are per chunk; nothing caps layers in
  flight.
- `pose_bytes`' `other_pose` prices a gathered tile (288 B); the held
  terminus tile is compact RGB (216 B).
- **Layer sides**: one entry per patch holds for point, spot and
  directional lights only (`light_eval.rs`); an area light
  (`plans/ready/area-lights`) crossing a patch's plane lights both.
- **Layer cap mechanics**: `shade_light_layer` takes a governor permit per
  chunk, and layer cache I/O takes none (build_pipeline.md, layer cache
  rule). A permit per layer would hold one while its chunks wait on their
  own; a blocking wait inside a rayon task can deadlock through work
  stealing. Batching layers needs neither.
- **Carried through later stages**: the bounce blocks, posed solids, id 34
  and its analyze clone stay alive through DeltaSh, DirectSh,
  AnimatedDirectSh and DirectShDelta; no gate prices them there. The stress
  map's measured whole-bake peak (warm) falls in DirectShDelta.
- **Entity-shadow section** is sized by its light selection, not the probe
  grid.

## 4. Compact-map baseline

`the_sh_base_gate_covers_every_poses_bake_on_the_setpiece_at_either_fidelity`
(ignored, four workers), at 176d0bf7:

| Mode | Cache | Predicted | Measured | Ratio |
|---|---|---|---|---|
| Low | cold | 708,624,596 | 538,029,235 | 1.32× |
| Low | warm empty | 708,624,596 | 538,029,235 | 1.32× |
| Low | full hit | 708,624,596 | 696,474,015 | 1.02× |
| Release | cold | 708,624,596 | 542,049,973 | 1.31× |
| Release | warm empty | 708,624,596 | 542,049,973 | 1.31× |
| Release | full hit | 708,624,596 | 701,297,793 | 1.01× |

"mover bounce states" measured 502–506 MB. On a compact map the
every-valid-probe bound is nearly reached; the stress map's looseness comes
from summing that bound over every mover, which per-mover finish removes.

## 5. Savings, estimated on the stress map

| Change | Saving |
|---|---|
| pairs through `light_may_reach` ∧ `may_shadow`, deduped layers (not taken) | up to ≈2.6e9 per pose |
| `other_pose` at the compact tile (not taken) | ≈1.1e8 |
| accumulated states → one mover's, blocks at a 256 MiB cap | ≈9.8e9 in every mover phase; ≈1.9e10 in the direct phase |
| layer scratch at a cap of in-flight layers | most of ≈2.9e9 |
| per-mover finish and both caps, no tightening (as decided) | largest phase ≈1.1e10: base and kept ≈1.8e9, one pose 8.24e9, one mover's states ≤1e9, blocks at the cap 0.27e9; under 1.72e10 |

With tightenings alone (no per-mover finish) the mover-bounce phase sits
near 1.72e10, still past the 16 GiB default. With per-mover finish and the
caps and no tightening, the largest phase is about 1.1e10 (last table
row). Estimate only; the stop-at-gate run settles it.

Other gates on the same run (stop-at-gate, 176d0bf7): the SH-delta gate
projects 2,335,108,608 dense bytes (id 27 174,016,512; id 41 2,061,697,536;
id 45 99,394,560) × copy-chain factor 3 = 7,005,325,824. Adding the carried
bytes (blocks at the 256 MiB cap, posed solids ≈3.9e6, id 34's section
≈1.13e9, its analyze clone where kept) leaves it near 8.4–9.5e9. The direct
window (blocks and section at the cap, direct entries ≈4.6e8, dense ids
27/41/45 ≈2.3e9, ids 34 and 35 and clones) sits in the same range. Every
gate stays under the 16 GiB default by estimate; the build's own run
settles it. Layers keep both sides (area lights).

## 6. Per-mover finish and the id-57 path

What spans movers today (read-only research at 176d0bf7):
- **Skip bars** read id 34 and id 59 before the trim, both final before any
  mover bakes; `MoverRecords` knots exist before the SH stage. Per mover.
- **Tile-range verdict** (`refuse_cut_short`) needs the union of depth
  probes across movers before skipping, plus stop and cut flags.
  `bake_mover_bounce` already keeps that union (`depth_probes`). Each mover
  must join it before its own skip, since skipping drops depth records.
- **Blocks**: `BounceBlocks::new` appends per mover with no shared offsets;
  `MoverProbeChannelSection::assemble` sorts bounce by (cell, mover) and
  depth by its order key, so append order never reaches the bytes.
- **Id-59 trim** (`reserve_mover_tiles`) needs only the carrying probe ids
  after skipping, so it runs once at the end.
- **Edge copy and memo writes** are per mover already.
- **Warm**: `PoseLookup` reduces fully-hit movers against final id 34;
  finishing them at their slot in the bake loop keeps the stop order.
- **Direct half** carries the blocks; nothing reads bounce content.
- Tests that drive `skip_mid_poses` and `finish_bounce_half` over a
  `Vec<MoverBounceStates>` change with it.

Id 57 from section to disk:
- `channel_section` clones the blocks into an owned section; placement sets
  flags and copies nothing, so streamed payload stays in the struct.
  `finish_section` holds blocks and the final section together.
- The struct lives until packing ends. Id 50's spool streams the streamed
  rows to a temp file; the PRL writer materializes one section's bytes at a
  time (`to_bytes`), and read-back reads id 57 whole beside the struct.
- No cap on id-57 bytes fails the build today; the 8 MiB bars only choose
  resident or streamed. `enforce_payload_cap` (`delta_sections.rs`) fails
  the build past `--sh-delta-max-size` and never drops or coarsens.

## 7. Commitments touched

- Pre-ray gate: build_pipeline.md step 12, Mover probe channel, *Memory*.
  The id-57 cap is the one post-ray refusal this adds, beside the existing
  tile-range refusal.
- In-bake gating rejected for the SH-delta bakes:
  `plans/done/lighting-scale--compile-peak-ram`, Alternatives rejected.
  Per-mover finish is not in-bake admission: it bounds the working set and
  leaves the gate pre-ray.
- Bake-and-flush precedent:
  `plans/done/lighting-scale--lightmap-bake-incremental-flush`, byte-identical
  output. The compile-peak-ram epic listed a survey of the remaining bakes
  for the hold-everything anti-pattern; the mover channel landed after it.
- Payload-cap precedent: `--sh-delta-max-size`.
- "Fidelity never enters the price": b5ead67c2, build_pipeline.md *Memory*.
  Kept.

## 8. Side note, out of scope

The same warm build warned "spared set 93,635,896,528 (87.21 GiB) across
988,057 entries … exceeds cache budget 2.00 GiB" with 1.9 GB on disk.
`StageCache::spared_set` sums use-record sizes, never on-disk files. Not
investigated further.

## 9. Ordering pins

| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| I1 | Every mover fixture, `low` and `high`, cold and warm, 1 and 4 workers | Per-mover finish and the layer cap against the parent commit | Every emitted section byte-identical |
| I2 | Two or more movers, one past the tile range in one variant | Job order reversed | Same sections; same verdict, by name |
| M1 | Closet's two-door edit, setpiece; cold, warm empty, full hit | Mover k reduces, skips, encodes, drops; mover k+1 bakes | Ledger never holds two movers' raw or reduced states; states kinds' high-water ≤ the largest one mover's |
| L1 | Closet, setpiece, in-memory SH fixtures | Layers resolve under the cap; some hit the cache | In flight ≤ cap; price covers the largest layers in flight, a hit's payload included |
| L2 | Governor at one permit | Cap 1 and cap > 1 | Layer resolution completes |
| C1 | Synthetic payload just past and just under the cap | A mover's blocks join | Past: named failure listing flag, bytes, movers; at or under: admits |
| P1 | Closet and setpiece, `low` and `high`, cold | The estimate is the largest phase | Each priced phase ≥ the ledger peak inside its own window |
| P2 | Closet and setpiece, `low` and `high` | Warm over an empty cache; full hit; `low` over a `high`-primed cache | Each priced phase ≥ the ledger peak inside its own window |
| M2 | Closet's two-door edit, warm, one door's `light_bounces` changed after priming | The lookup plans the fully-hit door and holds nothing; it reads back and reduces at its slot; the other door bakes; both job orders | The held door's states are never live during the other's pose bake |
| M3 | Closet's two-door edit, warm, every pose primed; one door's memo entry made unreadable after lookup | The lookup finds the door whole; its read fails at its slot | The door bakes there; sections equal a cold build's, with or without a pending door |
| M4 | Closet's two-door edit, `low`, warm, every pose primed; the second door's entry unreadable after lookup | The first door finishes and its blocks join; the second's read fails; layers, bounce one and id 34's later bounces build at its slot beside its read-back poses | Mover-bake phase's ledger peak ≤ its price |
| M5 | Closet's two-door edit, `low`, warm; one pose's `high` entry corrupt, its `low` entry whole; separately, its `high` entry unreadable after lookup | Lookup plans the pose; the slot reads | Corrupt at lookup: served from `low`, sections equal the parent's. Failed at the slot: baked at the `high` count, sections equal the read-succeeding build's |
| W1 | Closet, `high` | Id 34 hits; a mover misses its pose memo | Priced ≥ measured |
| D1 | Closet, setpiece | Direct half after the SH delta and direct-SH stages; `finish_section` holds blocks and final section | Window peak ≤ its price; never two id-57 sections at once |
| D2 | A mover fixture with an animated light, `--sh-analyze` | Analysis clones in the window | Window still ≤ its price |
| D3 | Closet, setpiece | Blocks, solids, id 34 and clone live through the SH delta and direct-SH stages | SH-delta projection ≥ ledger peak there |
| I4 | One mover whose depth passes the tile range and whose blocks would pass the cap | Reduce; verdict; skip and encode never run | Fails by the tile range's name; nothing skips, encodes or joins the cap |
| I3 | Mover A's blocks pass the payload cap; mover B's depth passes the tile range | Order A, B and order B, A | Both fail by name; which name follows job order |
| G1 | A fixture whose SH-delta projection sits below the SH base peak | The cheaper SH gate checks first | One byte under refuses by the SH base gate's name; at the peak both admit |
