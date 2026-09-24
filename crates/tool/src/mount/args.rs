//! `solve-weapon-mount` argument parsing.
//!
//! The parser is where the command's refusals live: axes are declared in pairs
//! or not at all, a solve names both ends of the bake, a check needs neither,
//! and every numeric option is finite and in range before any model is loaded.
//!
//! See: context/lib/resource_management.md §7

use std::ffi::OsString;
use std::path::PathBuf;

use glam::Vec3;
use postretro_model::mount::MountAxes;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct MountCheckThresholds {
    pub(super) min_barrel_dot: f32,
    pub(super) max_barrel_y: f32,
    pub(super) min_up_dot: f32,
}

impl Default for MountCheckThresholds {
    fn default() -> Self {
        Self {
            min_barrel_dot: 0.999,
            max_barrel_y: 0.02,
            min_up_dot: 0.999,
        }
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct SolveWeaponMountArgs {
    pub(super) holder_path: PathBuf,
    pub(super) mount_joint: String,
    pub(super) clip: String,
    pub(super) time: f32,
    pub(super) weapon_path: PathBuf,
    pub(super) barrel: Option<Vec3>,
    pub(super) up: Option<Vec3>,
    pub(super) check: bool,
    pub(super) raw_source: Option<String>,
    pub(super) out: Option<String>,
    pub(super) grip: Option<[String; 3]>,
    pub(super) scale: Option<String>,
    pub(super) sockets: Vec<String>,
    pub(super) current_euler: Option<[f32; 3]>,
    pub(super) thresholds: MountCheckThresholds,
}

impl SolveWeaponMountArgs {
    pub(super) fn cli_axes(&self) -> Result<Option<MountAxes>, String> {
        match (self.barrel, self.up) {
            (Some(barrel), Some(up)) => Ok(Some(MountAxes {
                barrel,
                up,
                euler: None,
            })),
            (None, None) => Ok(None),
            _ => Err(usage("--barrel and --up must be supplied together")),
        }
    }
}

/// Recognize the distinct viewmodel-only read before parsing the regular solver.
pub(super) fn parse_read_muzzle_offset_args(args: &[OsString]) -> Result<Option<PathBuf>, String> {
    if args
        .first()
        .map(|argument| argument != "--read-muzzle-offset")
        .unwrap_or(true)
    {
        return Ok(None);
    }

    match args {
        [_, viewmodel_path] => Ok(Some(PathBuf::from(argument_string(
            viewmodel_path,
            "--read-muzzle-offset",
        )?))),
        _ => Err(usage(
            "--read-muzzle-offset requires exactly one weapon viewmodel glTF path",
        )),
    }
}

pub(super) fn parse_solve_weapon_mount_args(
    args: Vec<OsString>,
) -> Result<SolveWeaponMountArgs, String> {
    let mut holder_path = None;
    let mut mount_joint = None;
    let mut clip = None;
    let mut time = None;
    let mut weapon_path = None;
    let mut barrel = None;
    let mut up = None;
    let mut check = false;
    let mut raw_source = None;
    let mut out = None;
    let mut grip = None;
    let mut scale = None;
    let mut sockets = Vec::new();
    let mut current_euler = None;
    let mut min_barrel_dot = None;
    let mut max_barrel_y = None;
    let mut min_up_dot = None;
    let mut index = 0;

    while index < args.len() {
        let argument = argument_string(&args[index], "solve-weapon-mount argument")?;
        match argument.as_str() {
            "--mount-joint" => {
                set_once(
                    &mut mount_joint,
                    next_argument(&args, &mut index, "--mount-joint")?,
                    "--mount-joint",
                )?;
            }
            "--clip" => {
                set_once(
                    &mut clip,
                    next_argument(&args, &mut index, "--clip")?,
                    "--clip",
                )?;
            }
            "--time" => {
                let value = next_argument(&args, &mut index, "--time")?;
                set_once(&mut time, parse_finite_number(&value, "--time")?, "--time")?;
            }
            "--weapon" => {
                let value = next_argument(&args, &mut index, "--weapon")?;
                set_once(&mut weapon_path, PathBuf::from(value), "--weapon")?;
            }
            "--barrel" => {
                set_once(
                    &mut barrel,
                    parse_vec3(&args, &mut index, "--barrel")?,
                    "--barrel",
                )?;
            }
            "--up" => {
                set_once(&mut up, parse_vec3(&args, &mut index, "--up")?, "--up")?;
            }
            "--check" => {
                if check {
                    return Err(usage("solve-weapon-mount accepts only one --check"));
                }
                check = true;
                index += 1;
            }
            "--raw-source" => {
                set_once(
                    &mut raw_source,
                    next_argument(&args, &mut index, "--raw-source")?,
                    "--raw-source",
                )?;
            }
            "--out" => {
                set_once(
                    &mut out,
                    next_argument(&args, &mut index, "--out")?,
                    "--out",
                )?;
            }
            "--grip" => {
                set_once(
                    &mut grip,
                    parse_raw_vec3(&args, &mut index, "--grip")?,
                    "--grip",
                )?;
            }
            "--scale" => {
                let value = next_argument(&args, &mut index, "--scale")?;
                let parsed = parse_finite_number(&value, "--scale")?;
                if parsed <= 0.0 {
                    return Err(usage(&format!(
                        "--scale must be greater than zero, got {value:?}"
                    )));
                }
                set_once(&mut scale, value, "--scale")?;
            }
            "--socket" => {
                let socket = next_argument(&args, &mut index, "--socket")?;
                let Some((name, node)) = socket.split_once('=') else {
                    return Err(usage(
                        "--socket requires NAME=NODE (this is a prop metadata tag, not --mount-joint)",
                    ));
                };
                if name.is_empty() || node.is_empty() {
                    return Err(usage(
                        "--socket requires non-empty NAME and NODE in NAME=NODE",
                    ));
                }
                sockets.push(socket);
            }
            "--current-euler" => {
                set_once(
                    &mut current_euler,
                    parse_array3(&args, &mut index, "--current-euler")?,
                    "--current-euler",
                )?;
            }
            "--min-barrel-dot" => {
                set_once(
                    &mut min_barrel_dot,
                    parse_threshold(&args, &mut index, "--min-barrel-dot", -1.0, 1.0)?,
                    "--min-barrel-dot",
                )?;
            }
            "--max-barrel-y" => {
                set_once(
                    &mut max_barrel_y,
                    parse_threshold(&args, &mut index, "--max-barrel-y", 0.0, 1.0)?,
                    "--max-barrel-y",
                )?;
            }
            "--min-up-dot" => {
                set_once(
                    &mut min_up_dot,
                    parse_threshold(&args, &mut index, "--min-up-dot", -1.0, 1.0)?,
                    "--min-up-dot",
                )?;
            }
            option if option.starts_with('-') => {
                return Err(usage(&format!(
                    "unknown solve-weapon-mount option {option:?}"
                )));
            }
            path => {
                if holder_path.replace(PathBuf::from(path)).is_some() {
                    return Err(usage(
                        "solve-weapon-mount accepts exactly one skeleton model path",
                    ));
                }
                index += 1;
            }
        }
    }

    let holder_path =
        holder_path.ok_or_else(|| usage("solve-weapon-mount requires a skeleton model path"))?;
    let weapon_path =
        weapon_path.ok_or_else(|| usage("solve-weapon-mount requires --weapon <path>"))?;
    if !check && raw_source.is_none() {
        return Err(usage(
            "solve-weapon-mount requires --raw-source <path> in solve mode",
        ));
    }
    if !check && out.is_none() {
        return Err(usage(
            "solve-weapon-mount requires --out <path> in solve mode",
        ));
    }

    if barrel.is_some() != up.is_some() {
        return Err(usage("--barrel and --up must be supplied together"));
    }

    Ok(SolveWeaponMountArgs {
        holder_path,
        mount_joint: mount_joint.unwrap_or_else(|| "hand_r".to_string()),
        clip: clip.unwrap_or_else(|| "idle_aiming".to_string()),
        time: time.unwrap_or(0.0),
        weapon_path,
        barrel,
        up,
        check,
        raw_source,
        out,
        grip,
        scale,
        sockets,
        current_euler,
        thresholds: MountCheckThresholds {
            min_barrel_dot: min_barrel_dot.unwrap_or(0.999),
            max_barrel_y: max_barrel_y.unwrap_or(0.02),
            min_up_dot: min_up_dot.unwrap_or(0.999),
        },
    })
}

fn next_argument(args: &[OsString], index: &mut usize, option: &str) -> Result<String, String> {
    let Some(value) = args.get(*index + 1) else {
        return Err(usage(&format!("{option} requires a value")));
    };
    *index += 2;
    argument_string(value, option)
}

fn parse_vec3(args: &[OsString], index: &mut usize, option: &str) -> Result<Vec3, String> {
    Ok(Vec3::from_array(parse_array3(args, index, option)?))
}

fn parse_array3(args: &[OsString], index: &mut usize, option: &str) -> Result<[f32; 3], String> {
    let values = parse_raw_vec3(args, index, option)?;
    Ok([
        parse_finite_number(&values[0], option)?,
        parse_finite_number(&values[1], option)?,
        parse_finite_number(&values[2], option)?,
    ])
}

fn parse_raw_vec3(
    args: &[OsString],
    index: &mut usize,
    option: &str,
) -> Result<[String; 3], String> {
    let first = next_argument(args, index, option)?;
    let second = next_vector_component(args, index, option)?;
    let third = next_vector_component(args, index, option)?;
    for value in [&first, &second, &third] {
        let _ = parse_finite_number(value, option)?;
    }
    Ok([first, second, third])
}

fn next_vector_component(
    args: &[OsString],
    index: &mut usize,
    option: &str,
) -> Result<String, String> {
    let Some(value) = args.get(*index) else {
        return Err(usage(&format!(
            "{option} requires exactly three finite numbers"
        )));
    };
    *index += 1;
    argument_string(value, option)
}

fn argument_string(value: &OsString, label: &str) -> Result<String, String> {
    value
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| usage(&format!("{label} must be valid UTF-8")))
}

fn parse_finite_number(value: &str, option: &str) -> Result<f32, String> {
    let parsed = value
        .parse::<f32>()
        .map_err(|_| usage(&format!("{option} expects a finite number, got {value:?}")))?;
    if !parsed.is_finite() {
        return Err(usage(&format!(
            "{option} expects a finite number, got {value:?}"
        )));
    }
    Ok(parsed)
}

fn parse_threshold(
    args: &[OsString],
    index: &mut usize,
    option: &str,
    minimum: f32,
    maximum: f32,
) -> Result<f32, String> {
    let value = next_argument(args, index, option)?;
    let parsed = parse_finite_number(&value, option)?;
    if !(minimum..=maximum).contains(&parsed) {
        return Err(usage(&format!(
            "{option} must be between {minimum} and {maximum}, got {value:?}"
        )));
    }
    Ok(parsed)
}

fn set_once<T>(slot: &mut Option<T>, value: T, option: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(usage(&format!(
            "solve-weapon-mount accepts only one {option}"
        )));
    }
    Ok(())
}

pub(super) fn usage(message: &str) -> String {
    format!(
        "{message}\n\nUsage: postretro-tool solve-weapon-mount <skeleton.gltf> \\
         --weapon <baked-weapon.gltf> [--check] [--raw-source <raw-source> --out <output.gltf>] \\
         [--mount-joint NAME] [--clip NAME] [--time SECONDS] \\
         [--barrel X Y Z --up X Y Z] [--current-euler X Y Z] \\
         [--min-barrel-dot VALUE] [--max-barrel-y VALUE] [--min-up-dot VALUE] \\
         [--grip X Y Z] [--scale FACTOR] [--socket NAME=NODE]..."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn parse_solve_weapon_mount_args_defaults_and_preserves_bake_passthrough() {
        let parsed = parse_solve_weapon_mount_args(os_args(&[
            "content/dev/models/limitator/model.gltf",
            "--weapon",
            "content/dev/models/ar_4/model.gltf",
            "--barrel",
            "0",
            "1",
            "0",
            "--up",
            "0",
            "0",
            "1",
            "--raw-source",
            "raw/ar 4.glb",
            "--out",
            "content/dev/models/ar_4/model.gltf",
            "--grip",
            "0.0",
            "-0.05",
            "0.120",
            "--scale",
            "0.68",
            "--socket",
            "muzzle=BarrelTip",
            "--socket",
            "optic_rail=ScopeMount",
        ]))
        .expect("complete solve command should parse");

        assert_eq!(
            parsed.holder_path,
            PathBuf::from("content/dev/models/limitator/model.gltf")
        );
        assert_eq!(parsed.mount_joint, "hand_r");
        assert_eq!(parsed.clip, "idle_aiming");
        assert_eq!(parsed.time, 0.0);
        assert_eq!(
            parsed.weapon_path,
            PathBuf::from("content/dev/models/ar_4/model.gltf")
        );
        assert_eq!(parsed.barrel, Some(Vec3::Y));
        assert_eq!(parsed.up, Some(Vec3::Z));
        assert!(!parsed.check);
        assert_eq!(parsed.raw_source.as_deref(), Some("raw/ar 4.glb"));
        assert_eq!(
            parsed.out.as_deref(),
            Some("content/dev/models/ar_4/model.gltf")
        );
        assert_eq!(parsed.thresholds, MountCheckThresholds::default());
        assert_eq!(
            parsed.grip,
            Some(["0.0".into(), "-0.05".into(), "0.120".into()])
        );
        assert_eq!(parsed.scale.as_deref(), Some("0.68"));
        assert_eq!(
            parsed.sockets,
            ["muzzle=BarrelTip", "optic_rail=ScopeMount"]
        );
    }

    #[test]
    fn parse_solve_weapon_mount_args_rejects_incomplete_axes_and_missing_bake_endpoints() {
        let common = [
            "holder.gltf",
            "--weapon",
            "weapon.gltf",
            "--raw-source",
            "raw.glb",
            "--out",
            "out.gltf",
        ];
        let mut incomplete_axes = common.to_vec();
        incomplete_axes.extend(["--barrel", "0", "0", "1"]);
        assert!(parse_solve_weapon_mount_args(os_args(&incomplete_axes)).is_err());

        assert!(
            parse_solve_weapon_mount_args(os_args(&[
                "holder.gltf",
                "--weapon",
                "weapon.gltf",
                "--raw-source",
                "raw.glb",
            ]))
            .is_err()
        );
    }

    #[test]
    fn parse_solve_weapon_mount_rejects_non_positive_scale() {
        // Regression: zero scale was silently omitted from the emitted bake,
        // while negative scale could bake a reflected weapon frame.
        for scale in ["0", "-0.68"] {
            let error = parse_solve_weapon_mount_args(os_args(&[
                "holder.gltf",
                "--weapon",
                "weapon.gltf",
                "--raw-source",
                "raw.glb",
                "--out",
                "out.gltf",
                "--scale",
                scale,
            ]))
            .expect_err("mount workflow scale must be positive");
            assert!(error.contains("--scale must be greater than zero"));
        }
    }

    #[test]
    fn parse_solve_weapon_mount_check_mode_does_not_require_bake_endpoints() {
        let parsed = parse_solve_weapon_mount_args(os_args(&[
            "holder.gltf",
            "--weapon",
            "baked-weapon.gltf",
            "--check",
            "--min-barrel-dot",
            "0.95",
            "--max-barrel-y",
            "0.1",
            "--min-up-dot",
            "0.9",
        ]))
        .expect("check mode only needs the baked weapon");

        assert!(parsed.check);
        assert_eq!(parsed.raw_source, None);
        assert_eq!(parsed.out, None);
        assert_eq!(
            parsed.thresholds,
            MountCheckThresholds {
                min_barrel_dot: 0.95,
                max_barrel_y: 0.1,
                min_up_dot: 0.9,
            }
        );
    }

    #[test]
    fn parse_solve_weapon_mount_rejects_out_of_range_check_thresholds() {
        for args in [
            [
                "holder.gltf",
                "--weapon",
                "weapon.gltf",
                "--check",
                "--min-barrel-dot",
                "1.01",
            ]
            .as_slice(),
            [
                "holder.gltf",
                "--weapon",
                "weapon.gltf",
                "--check",
                "--max-barrel-y",
                "-0.01",
            ]
            .as_slice(),
            [
                "holder.gltf",
                "--weapon",
                "weapon.gltf",
                "--check",
                "--min-up-dot",
                "nan",
            ]
            .as_slice(),
        ] {
            assert!(parse_solve_weapon_mount_args(os_args(args)).is_err());
        }
    }

    #[test]
    fn parse_read_muzzle_offset_args_accepts_only_one_viewmodel_path() {
        assert_eq!(
            parse_read_muzzle_offset_args(&os_args(&[
                "--read-muzzle-offset",
                "content/dev/models/ar_4/model.gltf",
            ])),
            Ok(Some(PathBuf::from("content/dev/models/ar_4/model.gltf"))),
        );
        assert_eq!(
            parse_read_muzzle_offset_args(&os_args(&["holder.gltf"])),
            Ok(None),
            "ordinary solver arguments must continue to select the mount path",
        );
        assert!(parse_read_muzzle_offset_args(&os_args(&["--read-muzzle-offset"])).is_err());
        assert!(
            parse_read_muzzle_offset_args(&os_args(&[
                "--read-muzzle-offset",
                "one.gltf",
                "two.gltf",
            ]))
            .is_err()
        );
    }
}
