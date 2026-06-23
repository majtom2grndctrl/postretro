# Texture Pack Format (.prpak)

**Status:** Optional, lower priority. Part of Epic 8 (Material Optimization). Earns its place only if a shipping scenario surfaces filesystem-traversal cost from loose `.prm` files. Drop from the epic if shipping pressure doesn't materialise.

## Goal

Consolidate a mod's per-texture `.prm` sidecar files into a single `.prpak` archive for shipping. Runtime reads either form — loose `.prm` for iteration, packed `.prpak` for distribution. No re-encoding: payload bytes are copied verbatim. The pack is a thin index + concatenation.

Depends on the `baked-texture-mips` plan landing first. That plan moves baked mip pyramids out of the PRL into per-texture `.prm` files under `content/<mod>/.prl-cache/tex/<blake3>.prm`. This plan packages those files.

## Scope

### In scope

- `.prpak` wire format: little-endian, magic `"PRPK"`, version `u16`, count `u32`, per-entry `[u8; 32] blake3 + u64 offset + u64 length`, then concatenated payload. Layout in *Wire format*.
- Pack-build CLI command. Scans `content/<mod>/.prl-cache/tex/`, writes one `<mod>.prpak` alongside (or under `content/<mod>/`). Subcommand of `prl-build` or a standalone binary — pick during implementation; both are reasonable.
- Runtime loader change in the texture resource path: try `.prpak` first, fall back to loose `.prm`. Per-entry override: a loose `.prm` whose blake3 also appears in the pack **wins** over the packed copy. Supports overriding shipped content during development.
- Pack is opened read-only. Memory-map when convenient; an indexed read into a `Vec<u8>` is acceptable if mmap adds platform complexity. Runtime never mutates the file.
- Same in-memory representation as the loose path. Pack only adds an index lookup; the decode path downstream of "got `.prm` bytes" is unchanged.

### Out of scope

- Compression of the pack itself. Entries are already compressed where appropriate (BC5 normals, future BCn diffuse). A second compression layer over a content-addressed archive of compressed payloads pays nothing.
- Patch packs, delta packs, layered packs.
- Encryption, signing, DRM.
- Hot-swap of which pack is active during a session. Pack selection is a load-time decision.
- Multi-mod merging into one pack. Each mod gets its own `.prpak`.
- Legacy format migration. Pre-stable; one wire version, no v0 compatibility.
- Packing non-texture assets (audio, scripts, levels). Scoped to `.prl-cache/tex/`. If other asset classes earn the same treatment later, write a sibling plan with the same shape.

## Acceptance criteria

- [ ] Given a mod with N baked textures in `.prl-cache/tex/`, the pack-build command produces a `.prpak` containing exactly N entries. Re-extracting each entry by blake3 yields byte-identical bytes to the source `.prm`.
- [ ] Loading a packed mod renders identically to loading the same mod loose. Diff the framebuffer for `content/dev/maps/campaign-test.prl` between the two paths; expect bit-identical output.
- [ ] Override test: with `<mod>.prpak` present and a single loose `.prm` whose blake3 also appears in the pack, the runtime reads the loose file. Confirmable via a modified-payload sentinel that would change rendering.
- [ ] Missing-entry test: requesting a blake3 absent from both the pack and the loose directory fails the same way it does today (texture missing → load error at the same call site).
- [ ] Pack file opened by the runtime is read-only. Process-level write attempt against the open handle fails (sanity check, not a security boundary).
- [ ] Pack build time on a mod with ~500 textures is bounded by I/O — no decode, no re-encode, no per-entry CPU work beyond reading bytes and writing the index. Record build time in the PR; flag if it exceeds raw copy bandwidth by more than a small constant.

## Tasks

### Task 1: `.prpak` format crate

Define the wire format and a small reader / writer in a new module (probably `postretro-texture-pack` or a submodule of `postretro-level-format` — pick during implementation based on which crate the runtime texture path already pulls in). Round-trip test: build a pack from N synthetic entries, read it back, compare blake3 and payload bytes per entry. Reject unknown magic and unsupported version with distinct errors.

### Task 2: Pack-build CLI

Scan `content/<mod>/.prl-cache/tex/` for `*.prm`. Extract blake3 from each filename (the file is named after its content hash; verify by re-hashing the payload during build to catch corruption). Write entries in sorted blake3 order so the output is deterministic for a given input set. Atomically replace the destination `.prpak` (write to a temp file, rename on success) so a crashed build never leaves a half-written pack on disk.

### Task 3: Runtime loader

The texture resource path currently reads `.prm` directly from `.prl-cache/tex/`. Wrap that read in a small lookup layer: on first access for a mod, probe for `<mod>.prpak`, mmap (or read) it, build an in-memory `HashMap<blake3, (offset, length)>`. Per-texture read: check loose `.prl-cache/tex/<blake3>.prm` first; if absent, slice the pack at the indexed offset. Same `&[u8]` shape returned in both branches — the decode path downstream is unchanged.

## Sequencing

Task 1 defines the format both other tasks depend on. Tasks 2 and 3 then run concurrently: the pack-build CLI reads `.prm` files and emits `.prpak`; the runtime loader reads `.prpak` and emits `.prm` bytes. They meet at the wire format defined in Task 1 and can be tested independently against a hand-built fixture pack.

## Rough sketch

- Filenames in `.prl-cache/tex/` are `<blake3-hex>.prm`. The 32-byte blake3 in the pack index is the same hash, raw bytes (not hex). Parse the filename to recover it; verify against the payload during build.
- Index lives at the head of the file (after the fixed header), not the tail. Lets a streaming reader resolve any entry after a single bounded read of `header + count * 48` bytes.
- Offsets are absolute from the start of the file. Simpler than payload-relative offsets and the size cost is negligible — even a 4 GB pack fits in `u64` trivially.
- Entries sorted by blake3 in the on-disk index. Allows binary search if the in-memory `HashMap` is later swapped for a sorted slice + bsearch to cut allocator pressure on cold loads. Out of scope for v1; the sort is free at build time and preserves the option.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Pack magic | `PRPAK_MAGIC` | `b"PRPK"` (4 bytes) | n/a | n/a | n/a |
| Pack version | `PRPAK_VERSION` | `u16 = 1` | n/a | n/a | n/a |
| Entry record | `PackEntry` | `[u8; 32] + u64 + u64` (48 bytes) | n/a | n/a | n/a |

## Wire format

Little-endian throughout.

```
header:
  [u8; 4]  magic              -- "PRPK"
  u16      version            -- 1
  u16      reserved           -- must be zero
  u32      entry_count
  u32      reserved2          -- must be zero; pads header to 16 bytes

index (entry_count entries, sorted by blake3 ascending):
  [u8; 32] blake3             -- payload hash; matches `.prm` filename
  u64      offset             -- absolute byte offset from file start
  u64      length             -- payload byte length

payload:
  concatenated `.prm` bytes in index order; no per-entry framing,
  no inter-entry padding.
```

Payload bytes are byte-identical to the source `.prm` file. Per-entry blake3 hashes that payload (not the file on disk; identical in practice).

## Open questions

- **Pack location.** `content/<mod>/<mod>.prpak`, `content/<mod>/.prl-cache/<mod>.prpak`, or `content/<mod>/.prl-cache/tex.prpak`? First is most obvious for shipping; second keeps the cache directory self-contained; third generalises if other asset classes get packed later. Decide when Task 3 lands and the loader's mod-root resolution is in front of us.
- **Probe cost.** Stat'ing `.prl-cache/tex/<blake3>.prm` on every texture read to honour the loose-override rule defeats some of the point of packing on slow filesystems. Mitigation: build a one-shot directory listing of `.prl-cache/tex/` at mod-load time and consult that `HashSet<blake3>` per read. Cheap, and the directory is small in shipping scenarios (usually empty when a pack is present).
- **mmap portability.** `memmap2` is the obvious crate but adds an `unsafe` boundary (the OS can invalidate the mapping under us). A plain `Vec<u8>` read on mod load is safe, simple, and costs one allocation of the pack size — acceptable for the shipping case where the pack is the whole texture set. Default to the `Vec<u8>` path unless profiling shows it matters.
- **Whether to ship at all.** This plan is conditional. If no shipping mod ever demonstrates filesystem-traversal cost from loose `.prm` files, the plan stays in `drafts/` and eventually moves to an archive. Don't promote to `ready/` without a measured motivating case.
