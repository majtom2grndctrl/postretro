// Which bake a route or walk run read: the PRL file and its id-51 hash.
// See: context/lib/rendering_pipeline.md §12

use postretro_level_loader::LevelWorld;

/// The PRL and id 51 a run read (O14), as `key=value` tokens:
/// `id51=<hex|none> walk_epoch=<u32> size=<u64> mtime=<u64> prl=<path as typed>`.
/// The walk epoch is this build's; the runtime never reads a bake's. Two
/// runs share a bake when `id51` and `walk_epoch` match; size, mtime and the
/// path only say which file was read.
pub fn bake_identity(path: &str, world: &LevelWorld) -> String {
    let (size, mtime) = std::fs::metadata(path).map_or((0, 0), |meta| {
        let mtime = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_secs());
        (meta.len(), mtime)
    });
    let id51 = world.cell_residency_set.as_ref().map_or_else(
        || "none".to_string(),
        |set| {
            blake3::hash(&set.to_bytes(postretro_level_format::coded_plane::BlockCodec::Raw))
                .to_hex()[..16]
                .to_string()
        },
    );
    format!(
        "id51={id51} walk_epoch={} size={size} mtime={mtime} prl={path}",
        postretro_visibility::PORTAL_WALK_EPOCH,
    )
}

/// The value of `key` in a bake identity string.
pub fn bake_token<'a>(bake: &'a str, key: &str) -> Option<&'a str> {
    bake.split_ascii_whitespace()
        .find_map(|token| token.strip_prefix(key)?.strip_prefix('='))
}
