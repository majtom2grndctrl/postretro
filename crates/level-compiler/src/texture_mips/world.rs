// World material sidecar orchestration.
// See: context/lib/build_pipeline.md §Baked texture mips

use super::*;

/// Bake the world's material bundles — per-texture mip pyramids in `.prm`
/// sidecars under `cache_root` — and report what they cost.
///
/// Returns a map from texture name → 32-byte cache key (the `.prm` filename
/// stem in hex). Names whose slots are all missing get a `[0u8; 32]` key and
/// no `.prm` is written; callers flag the all-zero key in
/// `TextureCacheKeysSection`. The runtime treats zero keys as 'no source PNG'
/// and substitutes placeholders silently by design — missing PNGs are not an
/// error in maps that don't use every named texture slot.
///
/// The byte accounting is handed back rather than logged, because the
/// compiler's TextureMips stage bakes model sidecars and sprite collections
/// too and the report has to cover all three. Callers fold those in with
/// [`StageTextureBytes::account_baked_sidecar`] and flush the whole report
/// once with [`log_texture_byte_summary`].
pub fn bake_world_texture_mips(
    texture_names: &[String],
    texture_root: &Path,
    cache_root: &Path,
) -> anyhow::Result<(HashMap<String, [u8; 32]>, StageTextureBytes)> {
    let name_to_path = build_name_to_path_map(texture_root);
    let lut = build_srgb_to_linear_lut();

    let mut out: HashMap<String, [u8; 32]> = HashMap::with_capacity(texture_names.len());
    // Per-material, per-slot, per-mip byte accounting for the stage report.
    // `out` is a HashMap, so the summary keeps its own name-sorted ordering
    // rather than borrowing hash iteration order — the report has to be
    // identical across runs for identical inputs, like everything else
    // `prl-build` emits.
    let mut byte_summary = StageTextureBytes::new();

    for name in texture_names {
        // Normalize the incoming map name: lowercase, backslashes → forward
        // slashes, and strip a leading `textures/` (a no-op when absent) so a
        // root-inclusive TrenchBroom name (`textures/collection/stem`) matches
        // the relative keys (`collection/stem`).
        let normalized = normalize_map_texture_name(name);

        // Preserve a requested qualified base when any of its slots exists.
        // Only an entirely missing qualified bundle may fall back to the
        // unique bare-stem aliases.
        let resolved = resolve_texture_bundle_paths(&name_to_path, &normalized);
        let diff_path = resolved.diffuse;
        let spec_path = resolved.specular;
        let norm_path = resolved.normal;
        let emissive_path = resolved.emissive;
        let height_path = resolved.height;

        // Read raw bytes (needed for both filename key and bundle hash).
        let diff_bytes = match diff_path.as_ref() {
            Some(p) => Some(std::fs::read(p).map_err(|e| {
                anyhow::anyhow!("failed to read diffuse {} for '{name}': {e}", p.display())
            })?),
            None => None,
        };
        let spec_bytes = match spec_path.as_ref() {
            Some(p) => Some(std::fs::read(p).map_err(|e| {
                anyhow::anyhow!("failed to read specular {} for '{name}': {e}", p.display())
            })?),
            None => None,
        };
        let norm_bytes = match norm_path.as_ref() {
            Some(p) => Some(std::fs::read(p).map_err(|e| {
                anyhow::anyhow!("failed to read normal {} for '{name}': {e}", p.display())
            })?),
            None => None,
        };
        let emissive_bytes = match emissive_path.as_ref() {
            Some(p) => Some(std::fs::read(p).map_err(|e| {
                anyhow::anyhow!("failed to read emissive {} for '{name}': {e}", p.display())
            })?),
            None => None,
        };
        let height_bytes = match height_path.as_ref() {
            Some(p) => Some(std::fs::read(p).map_err(|e| {
                anyhow::anyhow!("failed to read height {} for '{name}': {e}", p.display())
            })?),
            None => None,
        };

        let filename_key = filename_key_for(
            diff_bytes.as_deref(),
            spec_bytes.as_deref(),
            norm_bytes.as_deref(),
            emissive_bytes.as_deref(),
            height_bytes.as_deref(),
        );

        // All-absent: nothing to bake.
        if diff_bytes.is_none()
            && spec_bytes.is_none()
            && norm_bytes.is_none()
            && emissive_bytes.is_none()
            && height_bytes.is_none()
        {
            out.insert(name.clone(), [0u8; 32]);
            continue;
        }

        if let (Some(diffuse), Some(emissive), Some(diffuse_path), Some(emissive_path)) = (
            diff_bytes.as_deref(),
            emissive_bytes.as_deref(),
            diff_path.as_ref(),
            emissive_path.as_ref(),
        ) {
            let (diffuse_width, diffuse_height) = png_dimensions(diffuse, diffuse_path)?;
            let (emissive_width, emissive_height) = png_dimensions(emissive, emissive_path)?;
            if (emissive_width, emissive_height) != (diffuse_width, diffuse_height) {
                anyhow::bail!(
                    "emissive texture {} is {emissive_width}x{emissive_height}, but diffuse texture {} for '{name}' is {diffuse_width}x{diffuse_height}; _e.png dimensions must match diffuse",
                    emissive_path.display(),
                    diffuse_path.display(),
                );
            }
        }

        // `_h.png` is packed into the specular slot's G channel, so its
        // dimensions are load-bearing twice over: they must match the diffuse
        // the shader marches against, and they must match `_s.png` or the two
        // channels cannot be interleaved at all. Both are hard bails, the way
        // `_e` already is. (`_s`/`_n` versus diffuse remains documented-only
        // and unenforced — out of scope here.)
        if let (Some(height), Some(height_path)) = (height_bytes.as_deref(), height_path.as_ref()) {
            let (height_width, height_height) = png_dimensions(height, height_path)?;
            if let (Some(diffuse), Some(diffuse_path)) = (diff_bytes.as_deref(), diff_path.as_ref())
            {
                let (diffuse_width, diffuse_height) = png_dimensions(diffuse, diffuse_path)?;
                if (height_width, height_height) != (diffuse_width, diffuse_height) {
                    anyhow::bail!(
                        "height texture {} is {height_width}x{height_height}, but diffuse texture {} for '{name}' is {diffuse_width}x{diffuse_height}; _h.png dimensions must match diffuse",
                        height_path.display(),
                        diffuse_path.display(),
                    );
                }
            }
            if let (Some(specular), Some(spec_path)) = (spec_bytes.as_deref(), spec_path.as_ref()) {
                let (spec_width, spec_height) = png_dimensions(specular, spec_path)?;
                if (height_width, height_height) != (spec_width, spec_height) {
                    anyhow::bail!(
                        "height texture {} is {height_width}x{height_height}, but specular texture {} for '{name}' is {spec_width}x{spec_height}; _h.png and _s.png share one Rg8Unorm slot and must have identical dimensions",
                        height_path.display(),
                        spec_path.display(),
                    );
                }
            }
        }

        let bundle_hash = bundle_hash_for(
            diff_bytes.as_deref(),
            spec_bytes.as_deref(),
            norm_bytes.as_deref(),
            emissive_bytes.as_deref(),
            height_bytes.as_deref(),
        );

        let prm_path = cache_root.join(format!("{}.prm", cache_filename_for_key(&filename_key)));

        // Cache hit: header and every declared slot parse, and bundle_hash matches.
        if prm_path.exists()
            && let Ok(bytes) = std::fs::read(&prm_path)
        {
            let (hdr_result, slots) = PrmFile::from_bytes_partial(&bytes);
            if let Ok(hdr) = hdr_result
                && hdr.layer_count == 1
                && hdr.bundle_hash == bundle_hash
                && cache_entry_has_valid_declared_slots(&hdr, &slots)
            {
                // Account the reused sidecar too: the report describes
                // what this level costs, not what this run rebaked.
                byte_summary.record_bundle(
                    filename_key,
                    name.clone(),
                    MaterialBytes::from_parsed_slots(&slots, hdr.layer_count),
                );
                out.insert(name.clone(), filename_key);
                continue;
            }
        }

        // Build only the slots whose source PNGs exist. Emissive dimensions
        // are checked against diffuse before the bundle is encoded.
        let mut slots_arr: [Option<PrmSlot>; 4] = [None, None, None, None];
        let mut slot_mask = PrmSlots::empty();

        if let (Some(b), Some(p)) = (diff_bytes.as_deref(), diff_path.as_ref()) {
            let (rgba, w, h) = decode_png_rgba(b, p)?;
            let payload = build_diffuse_chain(&rgba, w, h, &lut);
            slots_arr[0] = Some(PrmSlot {
                format: PrmFormat::Rgba8UnormSrgb,
                width: w as u16,
                height: h as u16,
                level_count: expected_level_count(w as u16, h as u16),
                payload,
            });
            slot_mask |= PrmSlots::DIFFUSE;
        }
        // Slot 1 is the specular slot in both of its forms. Without a height
        // sibling it stays single-channel `R8Unorm`, byte-identical to what it
        // has always baked. With one it becomes the two-channel surface map:
        // R specular (or 0 when `_s.png` is absent), G the inverted height
        // byte (`255 - height`). The SPECULAR
        // slot-mask bit is set if EITHER sibling is present.
        match (height_bytes.as_deref(), height_path.as_ref()) {
            (Some(hb), Some(hp)) => {
                let (height_rgba, w, h) = decode_png_rgba(hb, hp)?;
                // Stored as `255 - height`. The runtime re-centers on mid-gray
                // (128 = the surface plane); the bake applies no zero point,
                // because the bundle hash covers source PNG bytes only and a
                // changed transform would silently reuse stale sidecars.
                let inverted_height: Vec<u8> = height_rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| 255 - c[0])
                    .collect();

                let specular: Vec<u8> = match spec_bytes.as_deref().zip(spec_path.as_ref()) {
                    Some((sb, sp)) => {
                        let (spec_rgba, _, _) = decode_png_rgba(sb, sp)?;
                        spec_rgba.as_chunks::<4>().0.iter().map(|c| c[0]).collect()
                    }
                    None => vec![0u8; inverted_height.len()],
                };

                let mut rg: Vec<u8> = Vec::with_capacity(inverted_height.len() * 2);
                for (s, g) in specular.iter().zip(inverted_height.iter()) {
                    rg.push(*s);
                    rg.push(*g);
                }

                let payload = build_surface_chain(&rg, w, h);
                slots_arr[1] = Some(PrmSlot {
                    format: PrmFormat::Rg8Unorm,
                    width: w as u16,
                    height: h as u16,
                    level_count: expected_level_count(w as u16, h as u16),
                    payload,
                });
                slot_mask |= PrmSlots::SPECULAR;
            }
            _ => {
                if let (Some(b), Some(p)) = (spec_bytes.as_deref(), spec_path.as_ref()) {
                    // Decode as RGBA; flatten to R8 (PNG authoring is typically L8 or
                    // RGBA8 with the spec data in R). We accept either.
                    let (rgba, w, h) = decode_png_rgba(b, p)?;
                    let r8: Vec<u8> = rgba.as_chunks::<4>().0.iter().map(|c| c[0]).collect();
                    let payload = build_specular_chain(&r8, w, h);
                    slots_arr[1] = Some(PrmSlot {
                        format: PrmFormat::R8Unorm,
                        width: w as u16,
                        height: h as u16,
                        level_count: expected_level_count(w as u16, h as u16),
                        payload,
                    });
                    slot_mask |= PrmSlots::SPECULAR;
                }
            }
        }
        if let (Some(b), Some(p)) = (norm_bytes.as_deref(), norm_path.as_ref()) {
            let (rgba, w, h) = decode_png_rgba(b, p)?;
            // BC5 needs both dims ≥ 4. A normal map smaller than 4×4 has no
            // valid BC5 level, so emitting the slot would write level_count = 0
            // with an empty payload — which the runtime cannot upload. Drop the
            // slot instead; the runtime substitutes its neutral-normal
            // placeholder for an absent NORMAL slot.
            let level_count = bc5_level_count(w as u16, h as u16);
            if level_count == 0 {
                log::warn!(
                    "[prl-build] normal map for '{name}' is {w}x{h}, below the BC5 4x4 minimum — \
                     dropping the normal slot; the runtime neutral-normal placeholder will be used"
                );
            } else {
                let payload = build_normal_bc5_chain(&rgba, w, h);
                slots_arr[2] = Some(PrmSlot {
                    format: PrmFormat::Bc5RgUnorm,
                    width: w as u16,
                    height: h as u16,
                    level_count,
                    payload,
                });
                slot_mask |= PrmSlots::NORMAL;
            }
        }
        if let (Some(b), Some(p)) = (emissive_bytes.as_deref(), emissive_path.as_ref()) {
            let (rgba, w, h) = decode_png_rgba(b, p)?;
            let payload = build_diffuse_chain(&rgba, w, h, &lut);
            slots_arr[3] = Some(PrmSlot {
                format: PrmFormat::Rgba8UnormSrgb,
                width: w as u16,
                height: h as u16,
                level_count: expected_level_count(w as u16, h as u16),
                payload,
            });
            slot_mask |= PrmSlots::EMISSIVE;
        }

        let prm = PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask,
                bundle_hash,
                total_body_bytes: 0, // recomputed by to_bytes
                layer_count: 1,
            },
            slots: slots_arr,
        };

        let encoded = prm
            .to_bytes()
            .map_err(|e| anyhow::anyhow!("encoding .prm for texture {name:?}: {e}"))?;
        atomic_write(&prm_path, &encoded)?;

        byte_summary.record_bundle(
            filename_key,
            name.clone(),
            MaterialBytes::from_slots(&prm.slots, prm.header.layer_count),
        );
        out.insert(name.clone(), filename_key);
    }

    Ok((out, byte_summary))
}
