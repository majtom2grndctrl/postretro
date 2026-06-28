# Asset Layout & Compiled-Output Store — Design Exploration

**Date investigated:** 2026-06-28
**Status:** Pre-spec exploration. Not yet a draft plan. Captures the long-term
target for the engine's on-disk asset layout — three tiers, a content-addressed
compiled store, and an eventual packfile/VFS for shipping — so smaller slices
(the just-landed `.prm` relocation; future maps/per-mod baked trees) keep the
right seams open and don't bake in the wrong layout.

> **Read this when:** moving or adding any *compiled* build output (`.prm` mip
> sidecars, `.prl` maps, future baked trees), deciding where a new
> compiler-emitted artifact lives on disk, or scoping a content-addressed asset
> store / packfile / VFS for shipping.
> **Key invariant:** there are **three tiers** with different lifetimes —
> **source** (`content/<mod>/`, authored, version-controlled), **compiled
> output** (`baked/`, runtime-required, regenerable, dev-local), and
> **disposable cache** (`.build-caches/`, bake-time-only, delete anytime).
> Runtime-required output must never live under a name that says "disposable
> cache." Compiled assets the runtime resolves **by hash** stay
> **content-addressed** — the runtime holds the hash, never the source path.
> **Related:** `context/lib/build_pipeline.md` (§Build Cache, §Baked texture
> mips) · `context/lib/resource_management.md` (§1 texture pipeline) ·
> `context/research/spatial-streaming.md` (cell-cluster residency — the
> packfile's eventual addressing granularity).

---

## 1. The three-tier model

On-disk artifacts split cleanly by **lifetime and who depends on them**:

| Tier | Location | Lifetime | Tracked? | Runtime depends? |
|------|----------|----------|----------|------------------|
| **Source** | `content/<mod>/textures/…`, `content/<mod>/maps/*.map`, models | Authored; the durable input | Yes (git) | No (PNGs/`.map` not read at runtime for world materials) |
| **Compiled output** | `baked/` (e.g. `baked/materials/<hash>.prm`) | Regenerable from source by a bake; **runtime-required** until regenerated | No (gitignored, dev-local) | **Yes** |
| **Disposable cache** | `.build-caches/` (`prl-cache/` stage cache) | Bake-time acceleration only; delete anytime | No (gitignored) | No |

The failure this layout prevents: a runtime-required artifact sitting under a
name that *invites deletion*. `.prm` mip sidecars previously lived in
`.build-caches/prm-cache/` — a path whose name says "disposable cache," yet the
engine loads `.prm` from it at level load. `rm -rf .build-caches/` (an entirely
reasonable thing to do to a cache) silently broke the runtime until the next
bake. Splitting compiled output (`baked/`) from the disposable stage cache
(`.build-caches/`) makes the lifetime legible from the path alone.

---

## 2. Why `.prm` stays content-addressed

The compiled material store is **content-addressed** — each `.prm` is named
`<blake3-hex>.prm`, hashed from the source PNG bytes (`blake3(diffuse)` when a
diffuse slot is present; `blake3(tag_byte || first_present_PNG)` otherwise). The
relocation moved the directory only; the naming and by-hash resolution are
unchanged. Content-addressing is load-bearing, not incidental:

- **The runtime has only the hash, never the texture path.** PRL stores a
  `TextureCacheKeys` section — one 32-byte blake3 per texture name — and *no
  pixel data and no source path*. At level load the engine resolves
  `<prm_root>/<hash>.prm` directly from the key. A path-addressed store would
  require the runtime to reconstruct the source PNG path, which it does not
  have and should not need.
- **Cross-mod dedup falls out for free.** Two mods shipping byte-identical PNGs
  produce the same hash → one `.prm`. A path- or mod-scoped layout would
  duplicate identical bytes per mod.
- **New bytes → new file, so stale mips are never served.** Editing a PNG
  changes its hash, so it resolves to a *different* `.prm`; the old file is
  simply orphaned (and ages out of the dev tree on the next clean). There is no
  in-place mutation that could leave a half-written or stale mip chain at a name
  the runtime still resolves.

### Why co-locating `.prm` next to the source PNG is NOT runtime-addressable

A tempting "simpler" layout is to drop `<name>.prm` beside
`content/<mod>/textures/<collection>/<name>.png`. It does not work, for the same
reason content-addressing is required: **the runtime never has the source
path.** It has a hash. To find a path-co-located `.prm` it would have to invert
the hash back to a `content/<mod>/.../name`, which is impossible, or carry source
paths in the PRL (re-coupling the runtime to the authoring tree and breaking
cross-mod dedup). Co-location also forfeits dedup (per-mod copies) and would
write regenerable output into the version-controlled source tree. The
content-addressed store is the addressing model the runtime can actually use.

---

## 3. Trajectory: content-addressed asset store → packfile / VFS

`baked/materials/` is the first concrete instance of a general idea: a
**content-addressed asset store** for compiled output. The natural extension,
**out of scope today**, is to fold all compiled output behind a **packfile +
virtual filesystem (VFS)** for shipping:

- A shipped build bundles compiled assets into one (or a few) packfiles rather
  than a loose `baked/` tree. The VFS resolves a hash to a `(packfile, offset,
  length)` span — the same by-hash contract the loose store already honors, so
  the runtime's resolution model does not change, only the backing store.
- Content-addressing is exactly what a packfile wants as its index key:
  dedup across mods is automatic, and the pack can be append-only (new bytes →
  new entry, never an in-place rewrite).

This **dovetails with `spatial-streaming.md`'s cell-cluster residency.** That
note argues for one spatial residency substrate (clustered cells) that every
baked *spatial* section subscribes to, loading/evicting baked data by area.
A packfile/VFS is the on-disk counterpart: the streaming loader asks the VFS for
a cluster's section spans by key, and the pack serves them. The two compose —
**streaming decides *what's resident*; the packfile/VFS decides *how it's stored
and addressed*.** Texture/material residency stays off the *spatial* axis (it
keys on material + mip, per that note's "off-axis" call-out), but it still lives
in the same content-addressed store / packfile — the material store and the
spatial sections share the addressing substrate even though they have different
residency policies.

---

## 4. Slices: landed and deferred

**Landed (first slice):**

1. **`.prm` relocation** — move the compiled material store out of
   `.build-caches/prm-cache/` into `baked/materials/`, a top-level
   compiled-output tree sibling to the disposable stage cache. Naming
   (`<hash>.prm`) and by-hash resolution unchanged; only the directory moved.
   The writer (`resolve_prm_root_via_cargo`, `crates/level-compiler/src/main.rs`)
   and reader (`derive_prm_root_dev_layout`,
   `crates/postretro/src/startup/worker.rs`) both repointed at
   `<workspace>/baked/materials`, with a test in each crate guarding that they
   **agree in the dev layout** (divergence silently degrades every texture to a
   placeholder). Done.

**Deferred (still out of scope):**

2. **Maps → `baked/maps/`** — `.prl` compiled maps are also runtime-required
   compiled output and conceptually belong under `baked/`, not loose beside the
   `.map` source. Out of scope for the first slice (it touches map-load paths and
   CLI conventions); a clean follow-on once the `baked/` tier is established.
3. **Per-mod baked trees** — if/when compiled output needs per-mod scoping (e.g.
   for shippable mod packages), `baked/<mod>/…` layering. Content-addressing
   makes this *optional* for dedup (identical bytes already share), so this is
   driven by packaging/distribution needs, not correctness.
4. **Packfile / VFS for shipping** (§3) — the content-addressed store behind a
   packfile with a hash→span VFS. The largest slice; gated on the shipping story
   and dovetails with streaming residency.

---

## 5. Open questions / seams to keep open

- **Workspace-root resolution divergence.** Writer and reader find the workspace
  by *different* walks — the compiler via a `Cargo.toml` ancestor walk, the
  runtime via a fixed two-parent walk from `content_root`. They intentionally
  stay separate (collapsing them breaks shipping layouts that have no
  `Cargo.toml`), so the agreement is enforced by tests, not by shared code. Any
  third place that ever computes the `.prm` root must join this contract or
  silently break textures.
- **Shipping layout (no `Cargo.toml`).** Both functions fall back conservatively
  when the dev-layout assumption fails (compiler → `<map parent>/baked/materials`;
  runtime → `content_root`). These need not coincide outside the dev layout
  because shipping is unspecced — the packfile/VFS work (§3) is where the
  shipping path gets a real answer.
- **`baked/` GC.** Orphaned `.prm` (from edited PNGs) accumulate until a clean.
  Loose, the tree just grows; a packfile would compact on rebuild. Low priority
  while dev-local.
