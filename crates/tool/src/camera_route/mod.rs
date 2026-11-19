//! `generate-camera-route`: write a level's camera route as a pose file.
//!
//! The route comes from the generator the residency walk harness measures
//! (`postretro-camera-route`), so a route generated from a bundle is the
//! route the walk measures. It reads only the level's cells and portals: no
//! GPU, engine or feature.
//!
//! See: context/lib/rendering_pipeline.md §12

use std::ffi::OsString;

use postretro_camera_route::{
    CADENCE_HZ, DEFAULT_ASPECT, GeneratedKind, PoseFile, Projection, RouteHeader, RouteOrigin,
    bake_identity, exact_route_seeded, format_pose_file,
};
use postretro_level_loader::LevelWorld;

mod args;
#[cfg(test)]
mod tests;

use args::{GenerateArgs, parse_args};

pub(crate) fn run(args: Vec<OsString>) -> Result<i32, String> {
    let args = parse_args(args)?;
    let world = postretro_level_loader::load_prl(&args.level)
        .map_err(|error| format!("cannot read level {}: {error}", args.level))?;
    let file = generate(&world, &args)?;
    std::fs::write(&args.out, format_pose_file(&file))
        .map_err(|error| format!("write {}: {error}", args.out.display()))?;
    println!(
        "Wrote {}: {} poses, {} route, seed {:#x}, {} steps, from {}",
        args.out.display(),
        file.poses.len(),
        args.kind.key(),
        args.seed,
        args.steps,
        args.level,
    );
    Ok(0)
}

/// The pose file for `args` over `world`, nothing written. The header
/// records every value the poses were made with.
fn generate(world: &LevelWorld, args: &GenerateArgs) -> Result<PoseFile, String> {
    // Degrees to radians exactly as the walk converts its pitch, so a pose
    // fingerprints as the walk's frame at any pitch.
    let pitch = args.pitch_degrees.to_radians();
    let route =
        exact_route_seeded(world, args.kind, args.steps, pitch, args.seed).map_err(|error| {
            format!(
                "level {}: {error}; a route needs camera cells with a portal between them",
                args.level
            )
        })?;
    let params = [
        ("seed", format!("{:#x}", args.seed)),
        ("steps", args.steps.to_string()),
        ("pitch", args.pitch_degrees.to_string()),
        ("hfov", args.hfov_degrees.to_string()),
    ];
    Ok(PoseFile {
        header: RouteHeader {
            origin: RouteOrigin::Generated(GeneratedKind::exact(args.kind)),
            params: params
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
            projection: Projection {
                hfov_degrees: args.hfov_degrees,
                aspect: DEFAULT_ASPECT,
            },
            bake: bake_identity(&args.level, world),
            cadence_hz: CADENCE_HZ,
            spawn: Some(route.spawn),
        },
        poses: route.poses,
    })
}
