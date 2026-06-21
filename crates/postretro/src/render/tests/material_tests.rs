// Renderer unit tests (split from the original `mod tests`).
// See: context/lib/testing_guide.md

use super::super::*;

/// Valid 64-char hex string round-trips to the expected 32 bytes.
#[test]
fn parse_blake3_key_parses_valid_hex_to_expected_bytes() {
    // 32 bytes: 00 01 02 … 1e 1f
    let hex = (0u8..32).map(|b| format!("{b:02x}")).collect::<String>();
    let result = parse_blake3_key(&hex);
    let expected: [u8; 32] = std::array::from_fn(|i| i as u8);
    assert_eq!(result, expected);
}

/// A hex string that is too short yields the zero sentinel key.
#[test]
fn parse_blake3_key_wrong_length_returns_zero_sentinel() {
    // 63 chars — one short of the required 64.
    let short = "a".repeat(63);
    assert_eq!(parse_blake3_key(&short), [0u8; 32]);
}

/// A non-hex character anywhere in the string yields the zero sentinel key.
#[test]
fn parse_blake3_key_non_hex_chars_return_zero_sentinel() {
    // 64 chars but contains 'zz' at the start — not valid hex.
    let bad = format!("zz{}", "00".repeat(31));
    assert_eq!(parse_blake3_key(&bad), [0u8; 32]);
}

// Regression: a 64-byte non-ASCII key panicked on a UTF-8 boundary slice.
#[test]
fn parse_blake3_key_non_ascii_input_does_not_panic_and_returns_zero_sentinel() {
    let non_ascii = "é".repeat(32);
    assert_eq!(non_ascii.len(), 64);

    let result = std::panic::catch_unwind(|| parse_blake3_key(&non_ascii));

    assert!(result.is_ok());
    assert_eq!(result.expect("parser must not panic"), [0u8; 32]);
}

/// The all-zero 64-char sentinel string maps to the zero key. This is the
/// same string `zero_material_key()` in the loader produces ("0".repeat(64)),
/// pinning the cross-module contract without importing that function here.
#[test]
fn parse_blake3_key_maps_zero_sentinel_to_zero_key() {
    assert_eq!(parse_blake3_key(&"0".repeat(64)), [0u8; 32]);
}

// --- Model open-path vs. cache-key split (finding: content_root join) ---
//
// `load_skinned_model` needs a live `wgpu::Device`, so the path/key
// derivation is factored into the pure `resolve_model_open_path_and_handle`.
// These pin the contract: the glTF opens content-root-JOINED while the cache
// key stays the VERBATIM handle, so it equals what `mesh_render.rs` produces
// from `mesh.model` (`ModelHandle::from(mesh.model.clone())`) and the
// planner's `models.get(&group.model)` lookup hits.

#[test]
fn model_cache_key_is_the_verbatim_handle_while_open_path_is_joined() {
    let content_root = Path::new("/content/root");
    let model_rel = "models/x/scene.gltf";
    let (open_path, handle) = resolve_model_open_path_and_handle(model_rel, content_root);

    // Open path is joined under the content root.
    assert_eq!(open_path, content_root.join(model_rel));
    // Cache key is the raw handle, NOT the joined path — must match the
    // per-frame collector's `ModelHandle::from(mesh.model.clone())`.
    assert_eq!(handle, crate::model::ModelHandle::from(model_rel));
    assert_eq!(handle.as_str(), model_rel);
    // And the key is explicitly not the joined string.
    assert_ne!(handle.as_str(), open_path.to_string_lossy());
}

// --- Submesh material plan (GPU-free dedup + draw bookkeeping) ---------
//
// `resolve_skinned_model_material` needs a live `wgpu::Device` to build bind
// groups, so the dedup + range bookkeeping is factored into the pure
// `plan_submesh_materials`. These tests pin the contract the GPU layer
// builds on: one distinct key per distinct material (deduped), one draw per
// submesh covering its range, in submesh order.

use crate::model::gltf_loader::Submesh;

fn submesh(key: &str, start: u32, end: u32) -> Submesh {
    Submesh {
        material_key: key.to_string(),
        indices: start..end,
    }
}

#[test]
fn plan_records_one_draw_per_submesh_covering_every_range() {
    // Three distinct materials → three submeshes; every range must be
    // recorded (not just the first), in submesh order, each pointing at its
    // own distinct key.
    let a = "a".repeat(64);
    let b = "b".repeat(64);
    let c = "c".repeat(64);
    let submeshes = vec![submesh(&a, 0, 6), submesh(&b, 6, 12), submesh(&c, 12, 15)];

    let plan = plan_submesh_materials(&submeshes);

    // Three distinct keys, in first-seen order.
    assert_eq!(plan.distinct_keys, vec![a, b, c]);
    // One draw per submesh, ranges preserved in submesh order, each to its
    // own distinct material (0, 1, 2) — every range covered, not just #0.
    assert_eq!(plan.draws.len(), 3, "one draw entry per submesh");
    assert_eq!(plan.draws[0].indices, 0..6);
    assert_eq!(plan.draws[1].indices, 6..12);
    assert_eq!(plan.draws[2].indices, 12..15);
    assert_eq!(
        plan.draws.iter().map(|d| d.distinct).collect::<Vec<_>>(),
        vec![0, 1, 2],
        "distinct materials map to distinct plan entries",
    );
}

#[test]
fn plan_dedups_repeated_material_key_to_one_build() {
    // A model reusing one material across three primitives must build that
    // material ONCE (one distinct key) while still recording three draws —
    // each submesh range paired with the shared (deduped) material.
    let shared = "f".repeat(64);
    let submeshes = vec![
        submesh(&shared, 0, 3),
        submesh(&shared, 3, 6),
        submesh(&shared, 6, 9),
    ];

    let plan = plan_submesh_materials(&submeshes);

    assert_eq!(
        plan.distinct_keys.len(),
        1,
        "reused material key dedups to a single bind-group build",
    );
    assert_eq!(plan.distinct_keys[0], shared);
    assert_eq!(plan.draws.len(), 3, "still one draw per submesh");
    assert!(
        plan.draws.iter().all(|d| d.distinct == 0),
        "every submesh shares the one distinct material",
    );
    // Ranges still cover each submesh independently.
    assert_eq!(
        plan.draws
            .iter()
            .map(|d| d.indices.clone())
            .collect::<Vec<_>>(),
        vec![0..3, 3..6, 6..9],
    );
}

#[test]
fn plan_mixes_shared_and_distinct_keys_with_first_seen_order() {
    // Interleaved reuse: keys [x, y, x, z]. Distinct keys are first-seen
    // [x, y, z] (3 builds, not 4), and the third submesh reuses x's entry.
    let x = "1".repeat(64);
    let y = "2".repeat(64);
    let z = "3".repeat(64);
    let submeshes = vec![
        submesh(&x, 0, 3),
        submesh(&y, 3, 6),
        submesh(&x, 6, 9),
        submesh(&z, 9, 12),
    ];

    let plan = plan_submesh_materials(&submeshes);

    assert_eq!(
        plan.distinct_keys,
        vec![x, y, z],
        "distinct keys in first-seen order, deduped",
    );
    assert_eq!(
        plan.draws.iter().map(|d| d.distinct).collect::<Vec<_>>(),
        vec![0, 1, 0, 2],
        "third submesh reuses the first distinct material",
    );
    assert_eq!(plan.draws.len(), 4, "one draw per submesh, none dropped");
}
