// CPU half of baked `.prm` texture loading: sidecar read, parse, slot plan.
// See: context/lib/boot_sequence.md §2 · context/lib/rendering_pipeline.md §3

use std::path::Path;
use std::time::{Duration, Instant};

use postretro_level_format::prm::{
    PrmFile, PrmHeader, PrmReadError, PrmSlot, cache_filename_for_key,
};
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;

use crate::loaded_texture::{TextureSlotPlan, TextureSlotPolicy, texture_slot_plan};

/// One material's parsed sidecar, ready for the renderer's GPU pass. Owns its
/// bytes, so it can cross threads.
#[derive(Debug)]
pub enum PreparedTexture {
    /// Every slot takes its placeholder: no key, a zero key, or a sidecar that
    /// could not be read or planned. The reason was already logged.
    Placeholder,
    /// A single-layer sidecar. Slot errors stay per slot: the GPU pass warns
    /// and binds that slot's placeholder.
    Bundle {
        name: String,
        slots: [Result<PrmSlot, PrmReadError>; 4],
        plan: TextureSlotPlan,
    },
}

/// Where [`prepare_world_textures`] spent its time. Log-only.
#[derive(Debug, Default, Clone, Copy)]
pub struct TexturePrepTiming {
    /// `std::fs::read` of every `.prm` sidecar.
    pub read: Duration,
    /// `PrmFile::from_bytes_partial`: header decode and the per-slot payload copy.
    pub parse: Duration,
    /// Sidecar files read (a key shared by several names reads once per name).
    pub files_read: usize,
    /// Total bytes those reads returned.
    pub bytes_read: usize,
}

/// The CPU pass over one level's world materials, parallel to its texture names.
#[derive(Debug)]
pub struct PreparedWorldTextures {
    pub entries: Vec<PreparedTexture>,
    pub timing: TexturePrepTiming,
}

// Compile-time guard: prepared products cross from load prep to the
// renderer's upload thread.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<PreparedWorldTextures>();
    assert_send::<PreparedTexture>();
};

/// Plans a `texture_2d` upload, or `None` when the sidecar is layered: the
/// one-chain mip slicer must never see a layer-major payload.
pub fn d2_texture_slot_plan(
    header: &PrmHeader,
    slot_results: &[Result<PrmSlot, PrmReadError>; 4],
    policy: TextureSlotPolicy,
) -> Option<TextureSlotPlan> {
    (header.layer_count == 1).then(|| texture_slot_plan(header.slot_mask, slot_results, policy))
}

/// Read, parse and plan every world-material sidecar the PRL references.
/// `texture_names[i]` pairs with `keys.keys[i]`; an all-zero key is a silent
/// placeholder. A missing key, an unreadable file, a header error or a layered
/// sidecar warns once and degrades that entry to a placeholder.
pub fn prepare_world_textures(
    texture_names: &[String],
    keys: &TextureCacheKeysSection,
    prm_cache_root: &Path,
) -> PreparedWorldTextures {
    let mut entries = Vec::with_capacity(texture_names.len());
    let mut timing = TexturePrepTiming::default();

    for (i, name) in texture_names.iter().enumerate() {
        let key = match keys.keys.get(i) {
            Some(k) => *k,
            None => {
                // PRL out of sync with TextureCacheKeys — log once per entry.
                log::warn!(
                    "[Loader] texture '{name}' index {i} has no cache key — using placeholders"
                );
                entries.push(PreparedTexture::Placeholder);
                continue;
            }
        };

        if key == [0u8; 32] {
            // Zero key signals "no source PNG" (e.g., compiler couldn't resolve
            // the name). Silent placeholder by design.
            entries.push(PreparedTexture::Placeholder);
            continue;
        }

        let prm_path = prm_cache_root.join(format!("{}.prm", cache_filename_for_key(&key)));
        let read_started = Instant::now();
        let read_result = std::fs::read(&prm_path);
        timing.read += read_started.elapsed();
        let bytes = match read_result {
            Ok(b) => {
                timing.files_read += 1;
                timing.bytes_read += b.len();
                b
            }
            Err(err) => {
                log::warn!(
                    "[Loader] texture '{name}': cannot read {} : {err} — using placeholders",
                    prm_path.display(),
                );
                entries.push(PreparedTexture::Placeholder);
                continue;
            }
        };

        let parse_started = Instant::now();
        let (header_result, slots) = PrmFile::from_bytes_partial(&bytes);
        timing.parse += parse_started.elapsed();
        let header = match header_result {
            Ok(header) => header,
            Err(e) => {
                log::warn!(
                    "[Loader] texture '{name}': .prm header error: {e:?} — using placeholders"
                );
                entries.push(PreparedTexture::Placeholder);
                continue;
            }
        };

        let Some(plan) = d2_texture_slot_plan(&header, &slots, TextureSlotPolicy::WorldBundle)
        else {
            log::warn!(
                "[Loader] texture '{name}': .prm declares {} layers, but world textures require exactly 1 — using placeholders",
                header.layer_count,
            );
            entries.push(PreparedTexture::Placeholder);
            continue;
        };

        entries.push(PreparedTexture::Bundle {
            name: name.clone(),
            slots,
            plan,
        });
    }

    PreparedWorldTextures { entries, timing }
}

/// Read, parse and plan one model material from the shared diffuse-addressed
/// `.prm` cache. Models consume only diffuse, even from a richer world bundle.
pub fn prepare_model_diffuse_texture(
    name: &str,
    key: [u8; 32],
    prm_cache_root: &Path,
) -> PreparedTexture {
    if key == [0u8; 32] {
        return PreparedTexture::Placeholder;
    }

    let prm_path = prm_cache_root.join(format!("{}.prm", cache_filename_for_key(&key)));
    let bytes = match std::fs::read(&prm_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            log::warn!(
                "[Loader] model texture '{name}': cannot read {} : {err} — using placeholders",
                prm_path.display(),
            );
            return PreparedTexture::Placeholder;
        }
    };

    let (header_result, slots) = PrmFile::from_bytes_partial(&bytes);
    let header = match header_result {
        Ok(header) => header,
        Err(err) => {
            log::warn!(
                "[Loader] model texture '{name}': .prm header error: {err:?} — using placeholders"
            );
            return PreparedTexture::Placeholder;
        }
    };
    let Some(plan) = d2_texture_slot_plan(&header, &slots, TextureSlotPolicy::ModelDiffuseOnly)
    else {
        log::warn!(
            "[Loader] model texture '{name}': .prm declares {} layers, but model textures require exactly 1 — using placeholders",
            header.layer_count,
        );
        return PreparedTexture::Placeholder;
    };
    PreparedTexture::Bundle {
        name: name.to_string(),
        slots,
        plan,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::prm::{PrmFormat, PrmSlots, STAGE_VERSION};

    fn diffuse_prm(layer_count: u16) -> Vec<u8> {
        let slot = PrmSlot {
            format: PrmFormat::Rgba8UnormSrgb,
            width: 1,
            height: 1,
            level_count: 1,
            payload: [0x11, 0x22, 0x33, 0xFF].repeat(usize::from(layer_count)),
        };
        PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask: PrmSlots::DIFFUSE,
                bundle_hash: [0xA5; 32],
                total_body_bytes: 0,
                layer_count,
            },
            slots: [Some(slot), None, None, None],
        }
        .to_bytes()
        .expect("fixture serializes")
    }

    fn write_prm(root: &Path, key: [u8; 32], bytes: &[u8]) {
        std::fs::write(
            root.join(format!("{}.prm", cache_filename_for_key(&key))),
            bytes,
        )
        .unwrap();
    }

    #[test]
    fn prepare_world_textures_degrades_each_unusable_sidecar_to_a_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        let good = [1u8; 32];
        let bad_header = [2u8; 32];
        let layered = [3u8; 32];
        let unreadable = [4u8; 32];
        write_prm(dir.path(), good, &diffuse_prm(1));
        write_prm(dir.path(), bad_header, b"not a prm file at all");
        write_prm(dir.path(), layered, &diffuse_prm(2));

        let names: Vec<String> = [
            "good",
            "zero",
            "unreadable",
            "bad_header",
            "layered",
            "keyless",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        // Five keys for six names: the last name has no cache key.
        let keys = TextureCacheKeysSection {
            keys: vec![good, [0u8; 32], unreadable, bad_header, layered],
        };

        let prepared = prepare_world_textures(&names, &keys, dir.path());

        assert_eq!(prepared.entries.len(), names.len());
        match &prepared.entries[0] {
            PreparedTexture::Bundle { name, slots, plan } => {
                assert_eq!(name, "good");
                assert!(slots[0].is_ok());
                assert!(matches!(slots[1], Err(PrmReadError::NotPresent)));
                assert_eq!(plan.consume, [true; 4]);
                assert_eq!(plan.mip_count, 1);
            }
            PreparedTexture::Placeholder => panic!("a valid sidecar must prepare a bundle"),
        }
        for (i, entry) in prepared.entries.iter().enumerate().skip(1) {
            assert!(
                matches!(entry, PreparedTexture::Placeholder),
                "entry {i} ({}) must degrade to a placeholder",
                names[i],
            );
        }
        // Good, bad-header and layered files were read; the zero key, the
        // missing file and the keyless name read nothing.
        assert_eq!(prepared.timing.files_read, 3);
    }

    #[test]
    fn prepare_model_diffuse_texture_consumes_only_the_diffuse_slot() {
        let dir = tempfile::tempdir().unwrap();
        let key = [7u8; 32];
        write_prm(dir.path(), key, &diffuse_prm(1));

        let PreparedTexture::Bundle { plan, .. } =
            prepare_model_diffuse_texture("model", key, dir.path())
        else {
            panic!("a valid sidecar must prepare a bundle");
        };
        assert_eq!(plan.consume, [true, false, false, false]);
        assert!(matches!(
            prepare_model_diffuse_texture("model", [0u8; 32], dir.path()),
            PreparedTexture::Placeholder
        ));
        assert!(matches!(
            prepare_model_diffuse_texture("model", [8u8; 32], dir.path()),
            PreparedTexture::Placeholder
        ));
    }

    // Regression: parsed multi-layer PRMs reached the legacy one-chain mip
    // slicer, panicking in debug and uploading only layer 0 in release.
    #[test]
    fn d2_world_and_model_planning_rejects_multi_layer_prm() {
        let bytes = diffuse_prm(2);
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        let header = header.expect("multi-layer header parses");

        assert!(
            slots[0].is_ok(),
            "fixture must contain valid layer payloads"
        );
        assert!(
            d2_texture_slot_plan(&header, &slots, TextureSlotPolicy::WorldBundle).is_none(),
            "world D2 upload must fall back before mip planning"
        );
        assert!(
            d2_texture_slot_plan(&header, &slots, TextureSlotPolicy::ModelDiffuseOnly).is_none(),
            "model D2 upload must fall back before mip planning"
        );
    }

    // Regression: a hand-authored 2x2 BC5 normal declared zero mips, then the
    // D2 upload path indexed an empty level list as if mip 0 existed.
    #[test]
    fn d2_world_planning_degrades_zero_mip_bc5_normal_to_placeholder() {
        const HEADER_SIZE: usize = 45;
        const SLOT_HEADER_SIZE: usize = 12;
        let mut bytes = vec![0u8; HEADER_SIZE + SLOT_HEADER_SIZE];
        bytes[0..4].copy_from_slice(b"PRM\x02");
        bytes[4] = STAGE_VERSION;
        bytes[5] = PrmSlots::NORMAL.bits();
        bytes[39..43].copy_from_slice(&(SLOT_HEADER_SIZE as u32).to_le_bytes());
        bytes[43..45].copy_from_slice(&1u16.to_le_bytes());
        bytes[HEADER_SIZE] = PrmFormat::Bc5RgUnorm as u8;
        bytes[HEADER_SIZE + 2..HEADER_SIZE + 4].copy_from_slice(&2u16.to_le_bytes());
        bytes[HEADER_SIZE + 4..HEADER_SIZE + 6].copy_from_slice(&2u16.to_le_bytes());
        // level_count and payload_bytes remain zero.

        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        let header = header.expect("slot failure must not poison the header");
        assert!(matches!(
            &slots[2],
            Err(PrmReadError::EmptyMipChain {
                slot: 2,
                format: PrmFormat::Bc5RgUnorm,
                width: 2,
                height: 2,
            })
        ));

        let plan = d2_texture_slot_plan(&header, &slots, TextureSlotPolicy::WorldBundle)
            .expect("single-layer world bundle still produces a placeholder plan");
        assert!(
            plan.consume[2],
            "the slot error routes through placeholder upload"
        );
        assert_eq!(
            plan.mip_count, 1,
            "no valid mip chain disables mip filtering"
        );
    }
}
