// Throwaway diagnostic: print a socket joint's world matrix EXACTLY as the
// engine computes it (same loader + world-pose sampler the attachment path uses),
// so weapon-mount tuning can target the engine's real glTF-node frame instead of
// Blender's reoriented pose-bone frame.
//
//   cargo run -p postretro-model --example socket_dump -- <model.gltf> [clip] [socket] [time]

use std::path::Path;

use postretro_model::gltf_loader::load_model;
use postretro_model::mount::{
    corrective_delta, detect_weapon_mount, resolve_socket_frame, verify_mount,
};

/// A path that looks like a model file — used to catch the common mistake of
/// `socket_dump <model> <weapon>`, which parses the weapon into the clip slot.
fn looks_like_model_path(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.ends_with(".gltf") || lower.ends_with(".glb") || lower.ends_with(".bin")
}

fn main() {
    // Positional args, collected once (indices stay stable for the weapon and
    // prior-euler slots below): <model> [clip] [socket] [time] [weapon] [ex ey ez].
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let path = argv
        .first()
        .cloned()
        .expect("usage: socket_dump <model.gltf> [clip] [socket] [time] [weapon.gltf] [ex ey ez]");
    let clip_name = argv
        .get(1)
        .cloned()
        .unwrap_or_else(|| "idle_aiming".to_string());
    // Args are strictly positional: a weapon must sit in slot 5 with clip,
    // socket, and time supplied first. A .gltf/.glb/.bin in the clip slot almost
    // always means someone wrote `socket_dump <model> <weapon>` and it is about
    // to panic with "clip not found".
    if looks_like_model_path(&clip_name) {
        eprintln!(
            "WARNING: clip arg {clip_name:?} looks like a model path. Args are positional — \
             to mount a weapon pass clip/socket/time first: \
             socket_dump <model> <clip> <socket> <time> <weapon>"
        );
    }
    let socket = argv.get(2).cloned().unwrap_or_else(|| "hand_r".to_string());
    let time: f32 = match argv.get(3) {
        Some(s) => s.parse().unwrap_or_else(|_| {
            eprintln!("WARNING: time arg {s:?} is not a number; using 0.0");
            0.0
        }),
        None => 0.0,
    };

    let resolved = resolve_socket_frame(Path::new(&path), &clip_name, &socket, time)
        .expect("socket frame resolution failed");
    let joint = resolved.joint_index;
    let m = resolved.matrix;
    eprintln!("socket {socket} -> joint {joint}; clip {clip_name} @ t={time}");
    eprintln!(
        "  hand local +X -> world {:?}",
        m.x_axis.truncate().to_array()
    );
    eprintln!(
        "  hand local +Y -> world {:?}",
        m.y_axis.truncate().to_array()
    );
    eprintln!(
        "  hand local +Z -> world {:?}",
        m.z_axis.truncate().to_array()
    );
    eprintln!("  position -> world {:?}", m.w_axis.truncate().to_array());
    println!(
        "MAT {}",
        m.to_cols_array()
            .iter()
            .map(|v| format!("{v:.6}"))
            .collect::<Vec<_>>()
            .join(" ")
    );

    // Optional 5th arg: a weapon model to mount at this socket (holder = identity,
    // exactly like the engine's `holder_transform * socket_matrix`). Reports the
    // barrel direction in ENGINE world space so tuning can target +Z (forward).
    //
    // The barrel is identified GEOMETRICALLY (not "farthest vertex from origin",
    // which can pick the stock butt): long axis from the extreme vertex pair,
    // refined by end-region centroids; the MUZZLE is the END WITH THE SMALLER
    // CROSS-SECTION (a barrel is thin; a stock/receiver is tall). "Up" comes from
    // the mass hanging below the bore line (grip/mag/stock are under the barrel).
    //
    // Optional args 6-8: the CURRENT bake's Blender-XYZ --rotate-euler degrees;
    // when given, prints the composed TOTAL euler to re-bake from the raw source.
    if let Some(weapon_path) = argv.get(4).cloned() {
        let weapon = load_model(Path::new(&weapon_path)).expect("load weapon failed");
        let detection = detect_weapon_mount(&weapon).expect("weapon mount detection failed");
        let frame = detection.frame;
        let verification = verify_mount(m, frame).expect("weapon mount verification failed");
        eprintln!("--- weapon {weapon_path} mounted ---");
        eprintln!(
            "  weapon local bbox min {:?} max {:?}",
            detection.bbox_min.to_array(),
            detection.bbox_max.to_array()
        );
        eprintln!(
            "  end A (t max) centroid {:?} max cross-radius {:.3}",
            detection.end_a.centroid.to_array(),
            detection.end_a.max_cross_radius
        );
        eprintln!(
            "  end B (t min) centroid {:?} max cross-radius {:.3}",
            detection.end_b.centroid.to_array(),
            detection.end_b.max_cross_radius
        );
        eprintln!(
            "  MUZZLE = thin end: centroid {:?} (r {:.3}); stock end {:?} (r {:.3})",
            detection.muzzle.centroid.to_array(),
            detection.muzzle.max_cross_radius,
            detection.stock.centroid.to_array(),
            detection.stock.max_cross_radius
        );
        eprintln!(
            "  barrel local {:?}  up local {:?}",
            frame.barrel.to_array(),
            frame.up.to_array()
        );
        eprintln!(
            "  BARREL -> world {:?}   (target forward = [0,0,1])",
            verification.barrel_world.to_array()
        );
        eprintln!(
            "  UP     -> world {:?}   (target up      = [0,1,0])",
            verification.up_world.to_array()
        );
        eprintln!(
            "  barrel·+Z = {:.3}  (1.0 = forward)   barrel·+Y = {:+.3}  (0 = level, + = muzzle up)",
            verification.barrel_dot_forward, verification.barrel_dot_up
        );
        eprintln!(
            "  up·+Y     = {:.3}  (1.0 = not rolled)",
            verification.up_dot_up
        );

        let d_gltf = corrective_delta(m, frame).expect("corrective delta failed");
        // Blender frame: b = C·g, C: (x,y,z) -> (x,-z,y).
        let c_map = glam::Mat3::from_cols(glam::Vec3::X, glam::Vec3::Z, -glam::Vec3::Y);
        let d_b = c_map * d_gltf * c_map.transpose();
        // Blender 'XYZ' euler means R = Rz·Ry·Rx.
        let blender_euler_deg = |r: glam::Mat3| -> [f32; 3] {
            let y = (-r.x_axis.z).asin();
            let x = r.y_axis.z.atan2(r.z_axis.z);
            let z = r.x_axis.y.atan2(r.x_axis.x);
            [x.to_degrees(), y.to_degrees(), z.to_degrees()]
        };
        let de = blender_euler_deg(d_b);
        eprintln!(
            "  DELTA rotate-euler (Blender XYZ deg, apply to THIS baked model): {:.3} {:.3} {:.3}",
            de[0], de[1], de[2]
        );
        // Prior --rotate-euler (ex ey ez) in slots 6-8. All three are required
        // for the TOTAL line; a partial or non-numeric set is a mistake, not a
        // silent skip.
        let euler_args: Vec<&String> = (5..8).filter_map(|i| argv.get(i)).collect();
        if !euler_args.is_empty() {
            let old: Vec<f32> = euler_args.iter().filter_map(|s| s.parse().ok()).collect();
            if old.len() == 3 {
                let rz = glam::Mat3::from_rotation_z(old[2].to_radians());
                let ry = glam::Mat3::from_rotation_y(old[1].to_radians());
                let rx = glam::Mat3::from_rotation_x(old[0].to_radians());
                let r_cur_b = rz * ry * rx;
                let te = blender_euler_deg(d_b * r_cur_b);
                eprintln!(
                    "  TOTAL rotate-euler (Blender XYZ deg, re-bake from raw source): {:.3} {:.3} {:.3}",
                    te[0], te[1], te[2]
                );
            } else {
                eprintln!(
                    "  WARNING: prior --rotate-euler needs exactly 3 numbers (ex ey ez); got {euler_args:?} — skipping the TOTAL re-bake line"
                );
            }
        }
    }
}
