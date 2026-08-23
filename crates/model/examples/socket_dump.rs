// Throwaway diagnostic: print a socket joint's world matrix EXACTLY as the
// engine computes it (same loader + world-pose sampler the attachment path uses),
// so weapon-mount tuning can target the engine's real glTF-node frame instead of
// Blender's reoriented pose-bone frame.
//
//   cargo run -p postretro-model --example socket_dump -- <model.gltf> [clip] [socket] [time]

use std::path::Path;

use glam::Mat4;
use postretro_model::anim::{Loop, sample_clip_looped_world_modified};
use postretro_model::gltf_loader::{SocketBinding, load_model};

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
    let path = argv.first().cloned().expect(
        "usage: socket_dump <model.gltf> [clip] [socket] [time] [weapon.gltf] [ex ey ez]",
    );
    let clip_name = argv.get(1).cloned().unwrap_or_else(|| "idle_aiming".to_string());
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

    let model = load_model(Path::new(&path)).expect("load_model failed");
    let joint = match model.sockets.get(&socket) {
        Some(SocketBinding::SkinnedJoint(j)) => *j,
        other => panic!("socket {socket:?} is not a skinned joint: {other:?}"),
    };
    let clip = model
        .clips
        .iter()
        .find(|c| c.name == clip_name)
        .unwrap_or_else(|| {
            panic!(
                "clip {clip_name:?} not found; have: {:?}",
                model.clips.iter().map(|c| c.name.as_str()).collect::<Vec<_>>()
            )
        });

    let mut out: Vec<Mat4> = Vec::new();
    sample_clip_looped_world_modified(
        clip,
        &model.skeleton,
        time,
        Loop::Clamp,
        &model.pose_stack,
        None,
        &mut out,
    );
    let m = out[joint];
    eprintln!("socket {socket} -> joint {joint}; clip {clip_name} @ t={time}");
    eprintln!("  hand local +X -> world {:?}", m.x_axis.truncate().to_array());
    eprintln!("  hand local +Y -> world {:?}", m.y_axis.truncate().to_array());
    eprintln!("  hand local +Z -> world {:?}", m.z_axis.truncate().to_array());
    eprintln!("  position -> world {:?}", m.w_axis.truncate().to_array());
    println!(
        "MAT {}",
        m.to_cols_array().iter().map(|v| format!("{v:.6}")).collect::<Vec<_>>().join(" ")
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
        use glam::{Mat3, Vec3};
        let weapon = load_model(Path::new(&weapon_path)).expect("load weapon failed");
        let verts: Vec<Vec3> =
            weapon.mesh.vertices.iter().map(|v| Vec3::from_array(v.position)).collect();
        let mut mn = Vec3::splat(f32::INFINITY);
        let mut mx = Vec3::splat(f32::NEG_INFINITY);
        for p in &verts {
            mn = mn.min(*p);
            mx = mx.max(*p);
        }
        // Long axis: extreme pair (subsampled), then refine via end centroids.
        let stride = (verts.len() / 3000).max(1);
        let sample: Vec<Vec3> = verts.iter().step_by(stride).copied().collect();
        let (mut pa, mut pb, mut best2) = (Vec3::ZERO, Vec3::ZERO, -1.0f32);
        for i in 0..sample.len() {
            for j in (i + 1)..sample.len() {
                let d = sample[i].distance_squared(sample[j]);
                if d > best2 {
                    best2 = d;
                    pa = sample[i];
                    pb = sample[j];
                }
            }
        }
        let mut axis = (pa - pb).normalize();
        let (mut ca, mut cb) = (Vec3::ZERO, Vec3::ZERO);
        for _ in 0..3 {
            let ts: Vec<f32> = verts.iter().map(|v| v.dot(axis)).collect();
            let tmin = ts.iter().cloned().fold(f32::INFINITY, f32::min);
            let tmax = ts.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let len = tmax - tmin;
            let (mut sa, mut na, mut sb, mut nb) = (Vec3::ZERO, 0.0f32, Vec3::ZERO, 0.0f32);
            for (v, t) in verts.iter().zip(&ts) {
                if *t > tmax - 0.10 * len {
                    sa += *v;
                    na += 1.0;
                }
                if *t < tmin + 0.10 * len {
                    sb += *v;
                    nb += 1.0;
                }
            }
            ca = sa / na;
            cb = sb / nb;
            axis = (ca - cb).normalize();
        }
        // Cross-section radius at each end (15% end regions), measured from the
        // long-axis line through the overall centroid.
        let c = verts.iter().copied().sum::<Vec3>() / verts.len() as f32;
        let ts: Vec<f32> = verts.iter().map(|v| v.dot(axis)).collect();
        let tmin = ts.iter().cloned().fold(f32::INFINITY, f32::min);
        let tmax = ts.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let len = tmax - tmin;
        let radial = |v: Vec3| -> f32 {
            let d = v - c;
            (d - d.dot(axis) * axis).length()
        };
        let (mut ra, mut rb) = (0.0f32, 0.0f32);
        for (v, t) in verts.iter().zip(&ts) {
            if *t > tmax - 0.15 * len {
                ra = ra.max(radial(*v));
            }
            if *t < tmin + 0.15 * len {
                rb = rb.max(radial(*v));
            }
        }
        // Muzzle = thin end.
        let (barrel_l, muzzle_c, stock_c, r_muzzle, r_stock) =
            if ra < rb { (axis, ca, cb, ra, rb) } else { (-axis, cb, ca, rb, ra) };
        // Up: mean mass offset from the bore line (through the muzzle centroid)
        // points DOWN (grip/mag/stock hang under the barrel).
        let mut mean_off = Vec3::ZERO;
        for v in &verts {
            let d = *v - muzzle_c;
            mean_off += d - d.dot(barrel_l) * barrel_l;
        }
        mean_off /= verts.len() as f32;
        let up_l = {
            let u = -mean_off;
            (u - u.dot(barrel_l) * barrel_l).normalize()
        };
        let side_l = up_l.cross(barrel_l); // X = Y cross Z (right-handed)

        let rot = Mat3::from_mat4(m);
        let barrel_w = (rot * barrel_l).normalize();
        let up_w = (rot * up_l).normalize();
        eprintln!("--- weapon {weapon_path} mounted ---");
        eprintln!("  weapon local bbox min {:?} max {:?}", mn.to_array(), mx.to_array());
        eprintln!(
            "  end A (t max) centroid {:?} max cross-radius {:.3}",
            ca.to_array(),
            ra
        );
        eprintln!(
            "  end B (t min) centroid {:?} max cross-radius {:.3}",
            cb.to_array(),
            rb
        );
        eprintln!(
            "  MUZZLE = thin end: centroid {:?} (r {:.3}); stock end {:?} (r {:.3})",
            muzzle_c.to_array(),
            r_muzzle,
            stock_c.to_array(),
            r_stock
        );
        eprintln!("  barrel local {:?}  up local {:?}", barrel_l.to_array(), up_l.to_array());
        eprintln!("  BARREL -> world {:?}   (target forward = [0,0,1])", barrel_w.to_array());
        eprintln!("  UP     -> world {:?}   (target up      = [0,1,0])", up_w.to_array());
        eprintln!(
            "  barrel·+Z = {:.3}  (1.0 = forward)   barrel·+Y = {:+.3}  (0 = level, + = muzzle up)",
            barrel_w.dot(Vec3::Z),
            barrel_w.dot(Vec3::Y)
        );
        eprintln!("  up·+Y     = {:.3}  (1.0 = not rolled)", up_w.dot(Vec3::Y));

        // Corrective delta D (glTF space, about the grip origin) so that
        // S·D maps barrel->+Z, up->+Y: D = S^T · G^T with G = [side up barrel].
        let g_l = Mat3::from_cols(side_l, up_l, barrel_l);
        let s3 = Mat3::from_cols(
            rot.x_axis.normalize(),
            rot.y_axis.normalize(),
            rot.z_axis.normalize(),
        );
        let d_gltf = s3.transpose() * g_l.transpose();
        // Blender frame: b = C·g, C: (x,y,z) -> (x,-z,y).
        let c_map = Mat3::from_cols(Vec3::X, Vec3::Z, -Vec3::Y);
        let d_b = c_map * d_gltf * c_map.transpose();
        // Blender 'XYZ' euler means R = Rz·Ry·Rx.
        let blender_euler_deg = |r: Mat3| -> [f32; 3] {
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
                let rz = Mat3::from_rotation_z(old[2].to_radians());
                let ry = Mat3::from_rotation_y(old[1].to_radians());
                let rx = Mat3::from_rotation_x(old[0].to_radians());
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
