// TEMPORARY dev-only diagnostics for B7 (mover-light-flow--probe-channel task
// 2): a patch-seed salt for seed-spread runs, per-ray and per-patch dumps for
// the data-path trace, and the posed gather's τ experiment. Every knob is a
// no-op unless its env var is set. Plan task 12 deletes this file and its call
// sites.
// See: context/plans/in-progress/mover-light-flow--probe-channel/research.md (B7 investigation)

use std::io::Write;

/// A `u64`: XORed into every surface-radiance patch's seed, static and
/// posed alike, so a build's patch shading and later-bounce noise draws a
/// fresh realisation. B7's seed-spread experiment. Unset, seeds are today's.
pub(crate) const PATCH_SEED_SALT_VAR: &str = "POSTRETRO_DEV_PATCH_SEED_SALT";

/// [`PATCH_SEED_SALT_VAR`], read once; 0 when unset. Layer and volume
/// caches key nothing of it, so a salted build runs with `--no-cache`.
pub(crate) fn patch_seed_salt() -> u64 {
    static SALT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *SALT.get_or_init(|| {
        std::env::var(PATCH_SEED_SALT_VAR)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    })
}

/// `<path>:<probe>,<probe>,…`: append every gather ray of the listed dense
/// probes to `<path>`, one line per ray: gather kind (`base`, `posed`), probe,
/// ray, direction, hit distance and radiance. B7's per-ray trace.
pub(crate) const RAY_DUMP_VAR: &str = "POSTRETRO_DEV_PROBE_RAY_DUMP";

type RayDump = Option<(std::sync::Mutex<std::fs::File>, Vec<u64>)>;

fn ray_dump_target() -> &'static RayDump {
    static DUMP: std::sync::OnceLock<RayDump> = std::sync::OnceLock::new();
    DUMP.get_or_init(|| {
        let value = std::env::var(RAY_DUMP_VAR).ok()?;
        let (path, probes) = value.rsplit_once(':')?;
        let probes = probes.split(',').filter_map(|p| p.parse().ok()).collect();
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()?;
        Some((std::sync::Mutex::new(file), probes))
    })
}

/// Record one gather ray when [`RAY_DUMP_VAR`] lists `probe`.
pub(crate) fn dump_ray(
    kind: &str,
    probe: u64,
    ray: usize,
    dir: glam::Vec3,
    distance: f32,
    radiance: glam::Vec3,
) {
    let Some((file, probes)) = ray_dump_target() else {
        return;
    };
    if !probes.contains(&probe) {
        return;
    }
    let line = format!(
        "{kind} {probe} {ray} {} {} {} {distance} {} {} {}\n",
        dir.x, dir.y, dir.z, radiance.x, radiance.y, radiance.z
    );
    if let Ok(mut file) = file.lock() {
        let _ = file.write_all(line.as_bytes());
    }
}

/// `<path>:<minx>,<miny>,<minz>,<maxx>,<maxy>,<maxz>` (engine metres):
/// append every patch whose centroid lies in the box to `<path>` at each
/// stage the caller names: face, cell, centroid, area, front and back
/// radiance. B7's patch trace.
pub(crate) const PATCH_DUMP_VAR: &str = "POSTRETRO_DEV_PATCH_DUMP";

type PatchDump = Option<(std::sync::Mutex<std::fs::File>, [f32; 6])>;

fn patch_dump_target() -> &'static PatchDump {
    static DUMP: std::sync::OnceLock<PatchDump> = std::sync::OnceLock::new();
    DUMP.get_or_init(|| {
        let value = std::env::var(PATCH_DUMP_VAR).ok()?;
        let (path, bounds) = value.rsplit_once(':')?;
        let bounds: Vec<f32> = bounds.split(',').filter_map(|v| v.parse().ok()).collect();
        let bounds: [f32; 6] = bounds.try_into().ok()?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()?;
        Some((std::sync::Mutex::new(file), bounds))
    })
}

/// Record `patches`' radiance at stage `kind` when [`PATCH_DUMP_VAR`] is set.
pub(crate) fn dump_patches(
    kind: &str,
    patches: &[crate::surface_radiance::Patch],
    radiance: &crate::surface_radiance::PatchRadiance,
) {
    use crate::surface_radiance::Side;
    let Some((file, b)) = patch_dump_target() else {
        return;
    };
    let mut out = String::new();
    for (index, patch) in patches.iter().enumerate() {
        let c = patch.centroid;
        if c.x < b[0] || c.y < b[1] || c.z < b[2] || c.x > b[3] || c.y > b[4] || c.z > b[5] {
            continue;
        }
        let front = radiance.get(index as u32, Side::Front);
        let back = radiance.get(index as u32, Side::Back);
        out += &format!(
            "{kind} {} {} {} {} {} {} {} {} {} {} {} {} {} {}\n",
            patch.face,
            patch.cell[0],
            patch.cell[1],
            c.x,
            c.y,
            c.z,
            patch.area,
            front.x,
            front.y,
            front.z,
            back.x,
            back.y,
            back.z,
            patch.normal.y,
        );
    }
    if let Ok(mut file) = file.lock() {
        let _ = file.write_all(out.as_bytes());
    }
}

/// Set (any value): the posed probe gather grows each posed brush by the
/// tracer's τ, as the static tracer grows each triangle. B7's τ experiment.
pub(crate) const POSED_TAU_VAR: &str = "POSTRETRO_DEV_POSED_TAU";

/// τ in metres under [`POSED_TAU_VAR`], else 0.
pub(crate) fn posed_tau() -> f64 {
    static TAU: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *TAU.get_or_init(|| {
        if std::env::var_os(POSED_TAU_VAR).is_some() {
            f64::from(crate::ray_traversal::TRACER_TOLERANCE_METERS)
        } else {
            0.0
        }
    })
}
