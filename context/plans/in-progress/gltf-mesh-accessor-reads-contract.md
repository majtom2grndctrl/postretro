# glTF Mesh Accessor Reads: Design Contract

## Goal

Mesh and skin accessor reads in `crates/model/src/gltf_loader.rs` trust stride and count, so a malformed file panics or aborts the loader whose contract is never to panic. Give them the protection animation sampler reads already have, and fail the load with a `ModelLoadError`.

## Decisions

- One gate, `validate_readable_accessor`, checks shape and then readability. Every mesh and skin read passes it, so no read site can carry one check without the other.
- `accessor_read_refusal` and `view_read_refusal` move to `gltf_loader.rs`, beside the other accessor helpers. `gltf_clips.rs` imports them and keeps its warn-and-skip policy.
- A new variant, `ModelLoadError::UnreadableAccessor`, carries the attribute and the refusal reason.
- Present optional streams (`TEXCOORD_0`, `NORMAL`, `TANGENT`) fail the load, as their shape and finiteness checks already do.
- A sparse accessor with no buffer view is refused for mesh data, though glTF allows it. Its `count` sizes an allocation whatever the file's size. No model in the repo uses one.
- `inverseBindMatrices` checks its count before the gate, so a count that disagrees with the joints stays `InverseBindCountMismatch`.

## Invariants

- The gate runs before the matching `read_*` call. Reader construction is lazy and safe; the read is not.
- `accessor.size()` is the element size only once the shape is known good, so shape is checked first.
- `gltf_clips.rs` behavior is unchanged, including its warning text.

## Ownership and acceptance

One track, `crates/model/src/gltf_loader.rs` and `gltf_clips.rs`.

- `cargo test -p postretro-model --lib` passes with a nonzero count.
- Each malformed shape, per read, has a test proving a load error rather than a panic.

## Base

Stacked on `feature/character-clip-library`, which holds the helpers. Rebase onto `main` once that branch merges.
