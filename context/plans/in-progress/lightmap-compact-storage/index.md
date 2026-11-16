# lightmap-compact-storage

Brief · resumable · reads: `context/lib/build_pipeline.md` §PRL section IDs, §Build Cache · `context/lib/rendering_pipeline.md` §4 · `context/lib/resource_management.md` §8.2 · `context/lib/development_guide.md` §1.4, §1.6 · `context/lib/testing_guide.md` §Resource bounds · read at 17c2f1e

## Problem
The owner reported that lightmaps dominate level size: four-room levels with heavy verticality exceed 3 GB. The showcase map baked at 0.025 m is 1,152 MB here, and its streamed lightmap pool bound is 550 MiB against a 384 MiB budget warning. Causes, all measured (`research.md`):
- The streamed lightmap, shadowmask, SH-payload and cell-residency sections (ids 22, 42, 50, 51) store their bytes with no entropy coding, though they are highly redundant. Together they are nearly the whole file.
- Charts pack poorly because a chart can never turn.
- Each present mask plane takes its block's full extent, though only a minority of faces own a mask channel.

When done:
- Levels are much smaller on disk.
- The pool needs much less VRAM.
- Irradiance, shadow masks and animated weight maps are bit-identical before BC6H/BC5 compression to the same chart baked at 17c2f1e. Direction is bit-identical to the same chart baked at an even origin.
- Late lightmap blocks are no more frequent than at 17c2f1e.
- A light edit never repacks the lightmap or re-bakes other lights' layers.

## Decisions
- **No backward compatibility.** Ids 22, 42, 50, 51 and 58 bump their versions, and old levels are rejected with a "re-bake" message (`development_guide.md` §1.6). Ids 22 and 42 drop their current layouts outright, as SMB7 was dropped.
- **Lossless coding is zstd, with checksums.** Level 7 is used for ids 22 and 42 (one frame per plane), id 50 (one frame per chunk) and id 51 (one frame for the whole section). Each frame records its content size and a checksum.
  - lz4 is rejected: it has no integrity check and decodes only about 15% faster.
  - Higher zstd levels decode slower.
  - The `zstd` crate joins `postretro-level-format` with a Cargo.toml rationale, against that crate's dependency-light default.
- **The compiler compresses at encode time, before the memos.** `lightmap_section` and `shadowmask_atlas` therefore hold the stored form, and the one-copy rule in `build_pipeline.md` step 12 holds over stored bytes. Both memo stage versions bump.
  - Pack compresses id 50 per chunk inside its spool and id 51 whole.
  - zstd output is not guaranteed stable across library versions, so both memo keys fold the codec's identity (library version and level), as `mover_probe_bounce` folds the BC6H codec version. With that fold, warm and cold builds still produce the same stored bytes.
- **Decode moves off the issuer to one shared decode pool that lives and retires with the issuer.** It starts whenever the issuer spawns with a route that needs decode, so lightmap decode does not depend on SH streaming async. Replacing either session retires the issuer and the pool together, as today; queued pairs are released and re-requested. SH and lightmap share it rather than each keeping a standing pool. A thread takes the highest-tier queued job, so a mandatory decode of either resource is taken before every queued optional decode and waits at most for the decode already running (`rendering_pipeline.md` §4, one issuer and one budget).
  - The issuer only reads. A decoded pair arrives already split per plane, so the frame thread no longer splits payloads. This supersedes the draft seed `lightmap-payload-split-frame-thread.md`.
  - All-resident loads decode on the loading thread; one thread already beats today's raw read.
- **Budgets count decoded bytes.** The per-drain budget, the in-hand caps and the route ledger charge a pair at its decoded size from the moment it is queued for decode.
  - This diverges from `rendering_pipeline.md` §4 "blocks upload … with no decode". Upload still takes BC data unchanged; only a lossless stage sits before it.
- **Corrupt coded data fails by name.** A block that fails decode is never uploaded, and its read completes as failed. A corrupt id-51 frame fails the level load by name. A corrupt id-50 chunk takes the existing failed-chunk path.
- **Charts may be stored transposed.** A chart keeps its own texel grid and every baked value. Only where its texels sit in the block changes.
  - Chart origins sit on even texels, so the half-resolution direction plane is the same wherever a chart lands. A chart at an odd origin at 17c2f1e groups different 2×2 texels once moved, so its direction baseline is an even-origin bake of the same chart, not 17c2f1e.
  - The packer tries several heuristics with width trim and keeps the smallest result.
  - The atlas layout fingerprint folds the transposed flag. One cold rebake follows, and stage versions bump.
- **Shadow masks get their own per-cell layout.** Only charts whose face has a mask owner are packed, each copied in its lightmap orientation, so the copy is a pure translation. Plane B is a sub-rect at the mask block's origin that holds every chart whose face owns channel 2 or 3, even when that chart's plane-B values are all zero, so no face samples another chart's mask.
  - The copy happens after owner ranking, padding and dilation, before BC5 encoding, so mask values are byte-copies of today's.
  - The lightmap layout and its caches stay independent of lights. The rejected rival packed mask-owning charts first inside the lightmap layout, which ties that layout to lights and turns a light edit into a near-full rebake.
- **Mask addressing:**
  - The block table entry grows to two `vec4<u32>`: today's lightmap word, plus per-plane mask extents.
  - Each owned face carries an i16 x/y offset from its lightmap texel to its mask texel. It is stamped at load onto the existing per-vertex owner stream, so world-vertex layouts and id 17 are untouched.
  - The shader adds the offset and clamps each plane to its own extent.
- **Pool sizing is set by late blocks, not by the budget.** Choose the first-generation layer rule (a ratio, an absolute floor, or both) so that late blocks are no worse than 17c2f1e on both walk routes. Report the resulting MiB.
  - The 384 MiB figure stays a warning, not a target, and the resulting MiB is measured on showcase but not gated.
  - The 5/4 ratio alone is too thin for the tighter layout (`research.md` §Pool headroom).
- **Non-goals:**
  - unlit-face skip and constant-plane flags (550 → 542 MiB);
  - pool placement changes (worth one layer at most);
  - never-visible face culling (the compiler already culls it);
  - padding 1 (a visible-quality risk the owner ruled out);
  - unread SH bodies in ids 34/35/41 (a separate suggested task; `research.md` §Follow-ups).

## Acceptance
### Automated
**Lossless storage**
- [ ] Every coded plane, chunk and section decodes to bytes identical to its unencoded form, through the all-resident load, the streaming read and capture's synchronous preload, on showcase and campaign-test.
- [ ] Load or read rejects each of these by name, never uploading the block: a single flipped byte in a frame, a short read, a declared raw length that disagrees with the extent, a stored length that breaks adjacency, and a decoded size that differs from the declared one.
- [ ] A level baked at 17c2f1e fails to load with a re-bake message for each bumped section.
- [ ] A corrupt id-51 frame fails the level load by name. A corrupt id-50 chunk takes the same path as a chunk whose hash fails today: async streaming marks it failed with one warning and one retry; a sync-proof read returns the error; the whole-load mover-probe read fails the level load by name.

**Bit-identity before compression**
- [ ] For every chart, transposed or not: irradiance, both mask planes and animated weight maps read through the chart's own grid are bit-identical to the 17c2f1e bake of the same map, before BC encoding. This holds on showcase and campaign-test, including charts carried across a bake-layer close.
- [ ] Direction read through each chart's own grid is bit-identical across two layouts that place charts differently, and bit-identical to 17c2f1e for every chart that sat at an even origin there.
- [ ] A synthetic chart whose 2×2 direction inputs sum differently in transposed order reduces to identical direction texels stored transposed and untransposed.
- [ ] Every owned face's mask texels, read through its stamped offset, equal its lightmap-layout mask texels at 17c2f1e. A face with no owner never samples a mask.
- [ ] Every face whose chart lies outside its block's plane-B sub-rect owns no channel 2 or 3 (pin P1).
- [ ] A filtered mask sample anywhere an owned face draws reads only that chart's texels and padding, as at 17c2f1e (pin P8).
- [ ] A block with no mask-owning face stores no mask plane and takes no mask placement. The compiled mask bound and the runtime's all-resident mask layers agree on showcase and campaign-test (pins P9, P10).
- [ ] Two faces sharing a vertex with equal owners but different mask offsets fail the stamp by name, as conflicting owners do today.
- [ ] On showcase, some charts are stored transposed, and the kept layout is no larger than any single heuristic's. Two cold bakes at different worker counts produce the same layout.
- [ ] The shader passes naga validation. Mask sampling clamps to each plane's own extent: a sample at a plane-B sub-rect edge never reads outside it.

**Streaming**
- [ ] In completed mode, decoding on the pool gives the same installs, uploads, late blocks and order-independent payload hash as an uncoded build of the same layout.
- [ ] A decoded pair reaches the frame thread already split per plane; admitting it copies no payload bytes.
- [ ] Each drain admits coded pairs up to the shared budget by their decoded size, never their stored size, so a drain installs no more bytes than an uncoded build of the same layout.
- [ ] With SH streaming off, or no id 50, lightmap pairs still decode off the issuer (pin P2). Lightmap late blocks are no worse than with SH on, on both walks.
- [ ] The chosen pool rule changes the pool policy identity. Growth reserve keeps 5/4 or reports its new rule, and the late-block row runs with both pools under the chosen rule.
- [ ] With the chosen pool rule, late blocks on the showcase in completed mode are ≤ 17c2f1e on both the far-point tour and the random walk. Realtime-mode medians over 3 interleaved runs are reported beside 17c2f1e's.
- [ ] With SH and lightmap both streaming, a mandatory decode of either resource is taken before every queued optional decode of either resource, and waits at most for the decode already running (pins P3, P4).
- [ ] Pinned 1- and 2-core realtime walks with SH streaming on show late SH chunks and lightmap blocks no worse than 17c2f1e, as medians over 3 interleaved runs.
- [ ] Queued and decoding pairs count at decoded size against the in-hand cap. Dropping the pool, replacing SH's session while the lightmap's is kept, declining the lightmap, unloading the level, or a block leaving its target with work queued each deliver or release every pair, and the ledger returns to zero (pins P5–P7).
- [ ] A block evicted and its placements re-tenanted in one drain has both block-table words written together; no frame samples a stale mask address (pin P11).

**Cache**
- [ ] A change of zstd level or library version misses both memos.
- [ ] A warm rebuild with no edit writes ids 22, 42, 50 and 51 byte-identical to a cold build.
- [ ] The bumped stage versions and the fingerprint miss the cache once.
- [ ] A warm rebuild after moving one light re-bakes only that light's layers: the atlas layout fingerprint is unchanged and every other light's layer memo hits.

### Manual
- [ ] Report, for showcase and campaign-test: section sizes, file size, id-58 MiB per pool, pack time, all-resident load time, and pinned 1- and 2-core issuer busy p95/max (measure-and-report).
- [ ] Visual A/B against 17c2f1e at fixed poses on both maps: static, animated and mover-shadowed surfaces show no visible difference.
- [ ] Tracy GPU frame time on the showcase is reported against 17c2f1e.

## Path
- **Spike code was not kept.** `research.md` carries its measurements; the seams below are where the spike changed code.
- **Seams:**
  - section lengths: `build_finalized_section_plan`, then `write_and_validate_sections_with_cluster_directory_validation`;
  - id-50 spool: `build_cluster_payload_spool`;
  - mask copy point: `OwnershipFill::close_layer` after `finish_face`, before `encode_blocks_bc5`, with carried blocks through `copy_unit_rect` and the `hold_until` field;
  - stream decode: `LightmapReadRoute::deliver` hands pairs to a pool generalized from `ShAsyncWorkers`' `decode_loop`, its `DecodeJob` and `CpuPhaseLedger`. The rival is the spike's separate mirror pool;
  - one coded-plane framing helper in `postretro-level-format`, shared by all four sections, so the queued follow-up on ids 34/35/41 can reuse it;
  - offset stamp: `build_vertex_spec_owners` (`SPEC_OWNER_STRIDE`);
  - bound: `first_generation_layers`, `PoolDemand::layers_of`, `place_all_resident`.
- **First slice:** the separate mask layout with a real bake on showcase, checking the 378 MiB simulation and the bit-identity row. It is the only result not yet confirmed by a real bake.
- **Then:**
  1. the transposed packer;
  2. zstd for ids 22/42 with the decode pool;
  3. ids 50/51;
  4. the pool-rule sweep last, on the final layout.
- **Regenerate** `content/dev/maps/test-fixtures/sh-streaming-hinted-door.prl` when id 50's epoch bumps.
- **At landing,** fold each "decided, not yet built" note naming this brief into the text around it, and update what the notes leave out:
  - `build_pipeline.md`: the version paragraph, the id-22, id-42 and id-58 wire layouts, step 10's slice wording, the memo bullets and the CLI table;
  - `rendering_pipeline.md` §4: the no-decode line and the block-table layout;
  - `resource_management.md` §8.2.

## Open questions
- Whether codec value 0 (stored raw) stays writable or is load-only for tests — **delegated**.
- Whether the runtime decodes with the same `zstd` crate or a lighter decoder behind a compiler-only encode feature — **delegated**: report decode speed if a different decoder is used.
- Whether SH keeps charging encoded bytes while queued on the shared pool, or switches to decoded size like lightmap pairs — **delegated**: the ledger must balance either way.
- The exact pool rule (ratio, floor, or both) — **delegated**: chosen by the late-block row and reported with its MiB.
- Whether the owner's 2.8 GB showcase came from an older mask format — **delegated**: owner to run `prl_sections.py` on that file; it doesn't change this brief.

## Boundary inventory
| Name | Rust | Wire | WGSL |
|---|---|---|---|
| block codec | `BlockCodec` | u32: 0 stored raw, 1 zstd frame | n/a |
| id-42 format tag | `SHADOWMASK_FORMAT_*` | ASCII `SMB9` | n/a |
| mask offset | per-face `i16` x, y | in the id-42 owner entry | flat `u32` varying, two packed i16 |
| block table entry | 32 B | n/a | `{ lightmap: vec4<u32>, mask: vec4<u32> }` |

## Wire format
Little-endian throughout, as for every PRL section. A stored length of 0 means the plane is absent; its raw length must be 0 too, and it has no frame.
- **Id 22 v4 record, 48 B** (mirrors v3's 36 B record):
  - cell_id u32, width u16, height u16;
  - irradiance_offset u64, irradiance_stored_len u32;
  - direction_offset u64, direction_stored_len u32;
  - codec u32, irradiance_raw_len u32, direction_raw_len u32, reserved u32.
  - Raw lengths must equal the lengths derived from the extent.
- **Id 42 `SMB9`:**
  - The header is unchanged.
  - Owner entry, 12 B: four u16 owners, then i16 dx, i16 dy (mask texel = lightmap texel + delta). The delta is 0 for faces with no owner.
  - Block record, 44 B:
    - group_a_offset u64, group_a_stored_len u32;
    - group_b_offset u64, group_b_stored_len u32;
    - codec u32, group_a_raw_len u32, group_b_raw_len u32;
    - mask_width u16, mask_height u16, plane_b_width u16, plane_b_height u16.
  - Plane B lies at the mask block's origin, within its extent.
- **Id 50, epoch 3:** the chunk's former flags word holds the codec, and `payload_len` counts stored bytes. The chunk hash covers stored bytes. Layout otherwise mirrors the spike's epoch-3 chunk.
- **Id 51:** version bump; u32 codec, u32 reserved, u64 raw_len, then one zstd frame holding today's body.
- **Id 58:** version bump. Pool demand reads per-plane mask extents.
