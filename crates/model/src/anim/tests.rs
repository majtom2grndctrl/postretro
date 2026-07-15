use super::*;
use crate::pose_modifier::{JointMask, ModifierEntry, PoseModifier, PoseModifierStack};
use crate::skeleton::Joint;
use postretro_foundation::PoseInputs;

const EPS: f32 = 1.0e-5;

fn assert_vec3_eq(a: Vec3, b: Vec3, ctx: &str) {
    assert!(
        (a - b).length() < EPS,
        "{ctx}: expected {b:?}, got {a:?} (|d|={})",
        (a - b).length()
    );
}

fn assert_quat_eq(a: Quat, b: Quat, ctx: &str) {
    // Quats q and -q are the same rotation; compare via the angle between.
    let dot = a.normalize().dot(b.normalize()).abs().min(1.0);
    let angle = 2.0 * dot.acos();
    assert!(
        angle < 1.0e-3,
        "{ctx}: expected {b:?}, got {a:?} (angle={angle})"
    );
}

fn assert_mat4_eq(a: Mat4, b: Mat4, ctx: &str) {
    let (ca, cb) = (a.to_cols_array(), b.to_cols_array());
    for i in 0..16 {
        assert!(
            (ca[i] - cb[i]).abs() < 1.0e-4,
            "{ctx}: element {i} expected {}, got {}",
            cb[i],
            ca[i]
        );
    }
}

fn joint(parent: Option<usize>, inverse_bind: Mat4, rest: RestLocal) -> Joint {
    Joint {
        parent,
        inverse_bind: inverse_bind.to_cols_array_2d(),
        rest_local: rest,
    }
}

fn translation_clip(name: &str, duration: f32, joints: Vec<JointTracks>) -> AnimationClip {
    AnimationClip {
        name: name.to_string(),
        duration,
        joints,
    }
}

/// Child world/skinning matrix equals parentWorld * childLocal * inverseBind.
#[test]
fn hierarchy_composes_child_world_through_parent() {
    // Root at +X(2); child rest-translated +Y(3) relative to root, animated
    // to hold that rest (empty tracks). Inverse-binds chosen non-identity so
    // the multiply order is actually exercised.
    let root_ib = Mat4::from_translation(Vec3::new(-2.0, 0.0, 0.0));
    let child_ib = Mat4::from_translation(Vec3::new(0.0, -5.0, 0.0));
    let skeleton = Skeleton {
        joints: vec![
            joint(
                None,
                root_ib,
                RestLocal {
                    translation: Vec3::new(2.0, 0.0, 0.0),
                    ..Default::default()
                },
            ),
            joint(
                Some(0),
                child_ib,
                RestLocal {
                    translation: Vec3::new(0.0, 3.0, 0.0),
                    ..Default::default()
                },
            ),
        ],
    };
    // Empty tracks → rest pose held.
    let clip = translation_clip("rest", 1.0, vec![JointTracks::default(); 2]);

    let mut out = Vec::new();
    sample_clip(&clip, &skeleton, 0.25, &mut out);
    assert_eq!(out.len(), 2);

    // Expected: rebuild by hand.
    let root_local = Mat4::from_translation(Vec3::new(2.0, 0.0, 0.0));
    let child_local = Mat4::from_translation(Vec3::new(0.0, 3.0, 0.0));
    let root_world = root_local;
    let child_world = root_world * child_local;
    let expected_child = child_world * child_ib;

    assert_mat4_eq(
        Mat4::from_cols_array_2d(&out[1].matrix),
        expected_child,
        "child skinning = parentWorld * childLocal * childInverseBind",
    );
    assert_mat4_eq(
        Mat4::from_cols_array_2d(&out[0].matrix),
        root_world * root_ib,
        "root skinning = rootWorld * rootInverseBind",
    );
}

/// Two translation keys → midpoint sample is the lerped position.
#[test]
fn translation_track_lerps_at_midpoint() {
    let skeleton = Skeleton {
        joints: vec![joint(None, Mat4::IDENTITY, RestLocal::default())],
    };
    let tracks = JointTracks {
        translation: Track {
            times: vec![0.0, 2.0],
            values: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, -4.0, 2.0)],
            ..Default::default()
        },
        ..Default::default()
    };
    let clip = translation_clip("t", 2.0, vec![tracks]);

    let mut out = Vec::new();
    // t = 1.0 is the midpoint of [0, 2].
    sample_clip(&clip, &skeleton, 1.0, &mut out);
    let translation = Mat4::from_cols_array_2d(&out[0].matrix).w_axis.truncate();
    assert_vec3_eq(translation, Vec3::new(5.0, -2.0, 1.0), "midpoint lerp");
}

/// Two rotation keys → midpoint slerp is the half-angle rotation.
#[test]
fn rotation_track_slerps_at_midpoint() {
    let skeleton = Skeleton {
        joints: vec![joint(None, Mat4::IDENTITY, RestLocal::default())],
    };
    let q0 = Quat::IDENTITY;
    let q1 = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2); // 90°
    let tracks = JointTracks {
        rotation: Track {
            times: vec![0.0, 1.0],
            values: vec![q0, q1],
            ..Default::default()
        },
        ..Default::default()
    };
    let clip = translation_clip("r", 1.0, vec![tracks]);

    let mut out = Vec::new();
    sample_clip(&clip, &skeleton, 0.5, &mut out);
    let sampled = Quat::from_mat4(&Mat4::from_cols_array_2d(&out[0].matrix));
    let expected = Quat::from_rotation_z(std::f32::consts::FRAC_PI_4); // 45°
    assert_quat_eq(sampled, expected, "midpoint slerp = half angle");
}

/// An empty SCALE track holds the joint's rest scale (not identity 1,1,1).
/// An empty translation/rotation track holds rest translation/rotation too.
#[test]
fn empty_channel_holds_rest_pose() {
    let rest = RestLocal {
        translation: Vec3::new(1.0, 2.0, 3.0),
        rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_3),
        scale: Vec3::new(0.5, 0.5, 0.5),
    };
    let skeleton = Skeleton {
        joints: vec![joint(None, Mat4::IDENTITY, rest)],
    };
    // Clip animates ONLY translation; scale + rotation tracks are empty and
    // must fall back to rest (rest scale 0.5, NOT 1.0).
    let tracks = JointTracks {
        translation: Track {
            times: vec![0.0, 1.0],
            values: vec![Vec3::new(1.0, 2.0, 3.0), Vec3::new(1.0, 2.0, 3.0)],
            ..Default::default()
        },
        ..Default::default()
    };
    let clip = translation_clip("partial", 1.0, vec![tracks]);

    let mut out = Vec::new();
    sample_clip(&clip, &skeleton, 0.5, &mut out);

    let m = Mat4::from_cols_array_2d(&out[0].matrix);
    let (scale, rotation, translation) = m.to_scale_rotation_translation();
    assert_vec3_eq(scale, rest.scale, "empty scale track holds rest scale");
    assert_quat_eq(rotation, rest.rotation, "empty rotation track holds rest");
    assert_vec3_eq(
        translation,
        rest.translation,
        "translation animated to rest value",
    );
}

/// Sampling at t = duration + ε equals sampling at ε (the clip loops).
#[test]
fn time_wraps_at_duration() {
    let skeleton = Skeleton {
        joints: vec![joint(None, Mat4::IDENTITY, RestLocal::default())],
    };
    let tracks = JointTracks {
        translation: Track {
            times: vec![0.0, 2.0],
            values: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(8.0, 0.0, 0.0)],
            ..Default::default()
        },
        ..Default::default()
    };
    let clip = translation_clip("wrap", 2.0, vec![tracks]);

    let eps = 0.1f32;
    let mut early = Vec::new();
    sample_clip(&clip, &skeleton, eps, &mut early);
    let mut wrapped = Vec::new();
    sample_clip(&clip, &skeleton, clip.duration + eps, &mut wrapped);

    let p_early = Mat4::from_cols_array_2d(&early[0].matrix).w_axis.truncate();
    let p_wrapped = Mat4::from_cols_array_2d(&wrapped[0].matrix)
        .w_axis
        .truncate();
    assert_vec3_eq(p_wrapped, p_early, "t = duration + eps wraps to t = eps");
}

#[test]
fn invalid_parent_link_degrades_to_root_instead_of_panicking() {
    let skeleton = Skeleton {
        joints: vec![joint(Some(1), Mat4::IDENTITY, RestLocal::default())],
    };
    let tracks = JointTracks {
        translation: Track {
            times: vec![0.0],
            values: vec![Vec3::new(3.0, 0.0, 0.0)],
            ..Default::default()
        },
        ..Default::default()
    };
    let clip = translation_clip("invalid-parent", 0.0, vec![tracks]);

    let mut out = Vec::new();
    sample_clip(&clip, &skeleton, 0.0, &mut out);

    assert_eq!(out.len(), 1);
    let translation = Mat4::from_cols_array_2d(&out[0].matrix).w_axis.truncate();
    assert_vec3_eq(
        translation,
        Vec3::new(3.0, 0.0, 0.0),
        "invalid parent composes as a root",
    );
}

/// Tripwire 2 (CPU-only, no GPU): measure per-frame `sample_clip` cost on the
/// real shipped skeleton + clip and print a min/mean/max summary. This is the
/// CPU pose-sampling figure `findings.md` projects to wave scale; it needs no
/// renderer, so it runs here. Gated on the asset existing (mirrors the loader's
/// real-model test) and `#[ignore]`d so it only runs on demand:
///   cargo test -p postretro-model --release sample_clip_cpu_cost -- --ignored --nocapture
/// (Run `--release` for a representative steady-state figure; debug is far
/// slower and not the number to report.)
#[test]
#[ignore = "measurement; run explicitly with --ignored --nocapture (prefer --release)"]
fn sample_clip_cpu_cost_on_real_model() {
    use std::path::PathBuf;
    use std::time::Instant;

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/models/decraniated_low_poly_retro_pixel/scene.gltf");
    if !path.exists() {
        eprintln!("skipping: model asset not present at {}", path.display());
        return;
    }
    let model = crate::gltf_loader::load_model(&path).expect("model loads");
    let clip = model.clips.first().expect("model has one clip");
    let skeleton = &model.skeleton;

    // Warm the thread-local scratch + caches so the first sample doesn't skew
    // the window.
    let mut out = Vec::new();
    for i in 0..64 {
        sample_clip(clip, skeleton, i as f32 * 0.016, &mut out);
    }

    const SAMPLES: u32 = 100_000;
    let mut min = u64::MAX;
    let mut max = 0u64;
    let mut total: u128 = 0;
    let mut t = 0.0f32;
    for _ in 0..SAMPLES {
        t += 0.016; // advance ~1 frame at 60fps so we sweep the whole clip
        let start = Instant::now();
        sample_clip(clip, skeleton, t, &mut out);
        let ns = start.elapsed().as_nanos() as u64;
        min = min.min(ns);
        max = max.max(ns);
        total += ns as u128;
    }
    let mean_us = (total as f64 / SAMPLES as f64) / 1000.0;
    eprintln!(
        "[sample_clip CPU cost] joints={} samples={} min={:.3}us mean={:.3}us max={:.3}us | \
             projected wave N=200: {:.1}us/frame ({:.3}ms)",
        skeleton.joints.len(),
        SAMPLES,
        min as f64 / 1000.0,
        mean_us,
        max as f64 / 1000.0,
        mean_us * 200.0,
        mean_us * 200.0 / 1000.0,
    );

    // Sanity only — the measurement is the print above, not a threshold.
    assert!(out.len() == skeleton.joints.len());
}

/// A STEP translation track holds the lower keyframe's value between keys and
/// snaps to a keyframe's value at/after that keyframe's time — no lerp.
#[test]
fn step_translation_track_holds_lower_keyframe() {
    let skeleton = Skeleton {
        joints: vec![joint(None, Mat4::IDENTITY, RestLocal::default())],
    };
    // Three keys so the snap-at-key assertion lands on an interior key (t=2),
    // away from t = duration where `sample_clip` wraps the time to 0.
    let k0 = Vec3::new(0.0, 0.0, 0.0);
    let k1 = Vec3::new(10.0, 0.0, 0.0);
    let k2 = Vec3::new(20.0, 0.0, 0.0);
    let tracks = JointTracks {
        translation: Track {
            times: vec![0.0, 2.0, 4.0],
            values: vec![k0, k1, k2],
            mode: Interp::Step,
        },
        ..Default::default()
    };
    let clip = translation_clip("step", 4.0, vec![tracks]);

    let sample_at = |t: f32| {
        let mut out = Vec::new();
        sample_clip(&clip, &skeleton, t, &mut out);
        Mat4::from_cols_array_2d(&out[0].matrix).w_axis.truncate()
    };
    // Between two keys: holds the LOWER key, not the midpoint lerp a LINEAR
    // track would yield ((5,0,0) on [k0,k1]).
    assert_vec3_eq(sample_at(1.0), k0, "STEP holds lower key mid-span");
    assert_vec3_eq(sample_at(1.99), k0, "STEP holds lower key just before next");
    // At and after the (interior) keyframe time: snaps to that key's value.
    assert_vec3_eq(sample_at(2.0), k1, "STEP snaps at the keyframe time");
    assert_vec3_eq(sample_at(3.0), k1, "STEP holds k1 until the next key");
}

/// LINEAR remains the default and still interpolates (regression guard that
/// adding the mode field did not change default behavior).
#[test]
fn linear_default_track_still_lerps() {
    let skeleton = Skeleton {
        joints: vec![joint(None, Mat4::IDENTITY, RestLocal::default())],
    };
    // Field-elided construction: `mode` defaults to Interp::Linear.
    let tracks = JointTracks {
        translation: Track {
            times: vec![0.0, 2.0],
            values: vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)],
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        tracks.translation.mode,
        Interp::Linear,
        "mode defaults LINEAR"
    );
    let clip = translation_clip("lin", 2.0, vec![tracks]);
    let mut out = Vec::new();
    sample_clip(&clip, &skeleton, 1.0, &mut out);
    let p = Mat4::from_cols_array_2d(&out[0].matrix).w_axis.truncate();
    assert_vec3_eq(p, Vec3::new(5.0, 0.0, 0.0), "LINEAR midpoint lerps");
}

/// A STEP rotation track holds the lower keyframe (no slerp between keys).
#[test]
fn step_rotation_track_holds_lower_keyframe() {
    let skeleton = Skeleton {
        joints: vec![joint(None, Mat4::IDENTITY, RestLocal::default())],
    };
    let q0 = Quat::IDENTITY;
    let q1 = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    let tracks = JointTracks {
        rotation: Track {
            times: vec![0.0, 1.0],
            values: vec![q0, q1],
            mode: Interp::Step,
        },
        ..Default::default()
    };
    let clip = translation_clip("stepr", 1.0, vec![tracks]);
    let mut out = Vec::new();
    // Midpoint: a LINEAR track would slerp to 45°; STEP holds q0 (0°).
    sample_clip(&clip, &skeleton, 0.5, &mut out);
    let sampled = Quat::from_mat4(&Mat4::from_cols_array_2d(&out[0].matrix));
    assert_quat_eq(sampled, q0, "STEP rotation holds lower key (no slerp)");
}

/// `out` is cleared and refilled, and reuse across calls does not change the
/// result — the steady-state allocation-free reuse path the renderer uses.
#[test]
fn reused_out_buffer_is_cleared_and_refilled() {
    let skeleton = Skeleton {
        joints: vec![joint(None, Mat4::IDENTITY, RestLocal::default())],
    };
    let clip = translation_clip("rest", 1.0, vec![JointTracks::default()]);

    let mut out = vec![
        BonePaletteEntry {
            matrix: [[9.0; 4]; 4]
        };
        5
    ];
    sample_clip(&clip, &skeleton, 0.0, &mut out);
    assert_eq!(
        out.len(),
        1,
        "out resized to joint count, stale entries gone"
    );
    // Second call reuses the same buffer and yields the same result.
    let first = out[0];
    sample_clip(&clip, &skeleton, 0.0, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0], first, "reuse is deterministic");
}

// --- Blended and loop-policy sampling ---

/// Decompose a palette entry's skinning matrix back to (translation,
/// rotation, scale). The blend tests use identity inverse-binds and a single
/// root joint, so the skinning matrix *is* the joint's local transform.
fn decompose(entry: BonePaletteEntry) -> (Vec3, Quat, Vec3) {
    let (s, r, t) = Mat4::from_cols_array_2d(&entry.matrix).to_scale_rotation_translation();
    (t, r, s)
}

/// A single-root skeleton with identity inverse-bind, so a sampled palette
/// entry decomposes straight back to the joint's local TRS.
fn single_root_skeleton() -> Skeleton {
    Skeleton {
        joints: vec![joint(None, Mat4::IDENTITY, RestLocal::default())],
    }
}

/// A one-joint clip that holds a constant local TRS (single key on each
/// channel), so it samples to exactly `(t, r, s)` at any time.
fn constant_pose_clip(name: &str, t: Vec3, r: Quat, s: Vec3) -> AnimationClip {
    let tracks = JointTracks {
        translation: Track {
            times: vec![0.0],
            values: vec![t],
            ..Default::default()
        },
        rotation: Track {
            times: vec![0.0],
            values: vec![r],
            ..Default::default()
        },
        scale: Track {
            times: vec![0.0],
            values: vec![s],
            ..Default::default()
        },
    };
    AnimationClip {
        name: name.to_string(),
        duration: 1.0,
        joints: vec![tracks],
    }
}

/// Blend weight 0 reproduces source A's pose; weight 1 reproduces source B's;
/// the midpoint differs from both. The endpoints must be exact (slerp
/// hemisphere handling) — a blend is not allowed to perturb an endpoint.
#[test]
fn blend_endpoints_reproduce_each_source_midpoint_differs() {
    let skeleton = single_root_skeleton();
    let pose_a = (Vec3::new(1.0, 0.0, 0.0), Quat::IDENTITY, Vec3::splat(1.0));
    let pose_b = (
        Vec3::new(0.0, 4.0, 0.0),
        Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
        Vec3::splat(2.0),
    );
    let clip_a = constant_pose_clip("a", pose_a.0, pose_a.1, pose_a.2);
    let clip_b = constant_pose_clip("b", pose_b.0, pose_b.1, pose_b.2);
    let src_a = BlendSource::Clip {
        clip: &clip_a,
        time: 0.0,
        loop_policy: Loop::Wrap,
    };
    let src_b = BlendSource::Clip {
        clip: &clip_b,
        time: 0.0,
        loop_policy: Loop::Wrap,
    };

    let mut out = Vec::new();

    sample_blended(&src_a, &src_b, 0.0, &skeleton, &mut out);
    let (t0, r0, s0) = decompose(out[0]);
    assert_vec3_eq(t0, pose_a.0, "weight 0 → A translation");
    assert_quat_eq(r0, pose_a.1, "weight 0 → A rotation");
    assert_vec3_eq(s0, pose_a.2, "weight 0 → A scale");

    sample_blended(&src_a, &src_b, 1.0, &skeleton, &mut out);
    let (t1, r1, s1) = decompose(out[0]);
    assert_vec3_eq(t1, pose_b.0, "weight 1 → B translation");
    assert_quat_eq(r1, pose_b.1, "weight 1 → B rotation");
    assert_vec3_eq(s1, pose_b.2, "weight 1 → B scale");

    sample_blended(&src_a, &src_b, 0.5, &skeleton, &mut out);
    let (tm, rm, sm) = decompose(out[0]);
    // Translation/scale lerp; rotation slerps to the half angle.
    assert_vec3_eq(
        tm,
        pose_a.0.lerp(pose_b.0, 0.5),
        "midpoint translation lerp",
    );
    assert_vec3_eq(sm, pose_a.2.lerp(pose_b.2, 0.5), "midpoint scale lerp");
    assert_quat_eq(
        rm,
        Quat::from_rotation_z(std::f32::consts::FRAC_PI_4),
        "midpoint rotation = half angle",
    );
    // And the midpoint is genuinely between, not at either endpoint.
    assert!((tm - pose_a.0).length() > EPS, "midpoint differs from A");
    assert!((tm - pose_b.0).length() > EPS, "midpoint differs from B");
}

/// Shortest-path slerp: blending 170° and −170° about Z goes the short way
/// (through 180°), not the long way (through 0°). The midpoint is 180°.
/// The endpoints being in opposite hemispheres is the case the manual flip guards.
#[test]
fn blend_rotation_takes_shortest_path() {
    let skeleton = single_root_skeleton();
    // 170° each side of zero about Z: shortest arc between them passes through
    // 180°, the long arc through 0°. Midpoint must land near 180°, not 0°.
    let r_a = Quat::from_rotation_z(170f32.to_radians());
    let r_b = Quat::from_rotation_z(-170f32.to_radians());
    let clip_a = constant_pose_clip("a", Vec3::ZERO, r_a, Vec3::ONE);
    let clip_b = constant_pose_clip("b", Vec3::ZERO, r_b, Vec3::ONE);
    let src_a = BlendSource::Clip {
        clip: &clip_a,
        time: 0.0,
        loop_policy: Loop::Wrap,
    };
    let src_b = BlendSource::Clip {
        clip: &clip_b,
        time: 0.0,
        loop_policy: Loop::Wrap,
    };

    let mut out = Vec::new();
    sample_blended(&src_a, &src_b, 0.5, &skeleton, &mut out);
    let (_, rm, _) = decompose(out[0]);
    // Shortest arc midpoint is ±180° about Z (through the back), NOT identity:
    // 170° and -170° are 20° apart the short way, so their midpoint is 180°.
    assert_quat_eq(
        rm,
        Quat::from_rotation_z(180f32.to_radians()),
        "shortest-path midpoint of 170°/-170° is 180°, not 0°",
    );
}

/// A looping clip wraps past its duration; a non-looping clip clamps and holds
/// its final keyframe forever after the clip ends.
#[test]
fn loop_policy_wraps_or_clamps_past_duration() {
    let skeleton = single_root_skeleton();
    // Translation 0 → 8 over [0, 2]. After the end: Wrap repeats (t=2.1 ≡ 0.1),
    // Clamp holds the final key (8).
    let tracks = JointTracks {
        translation: Track {
            times: vec![0.0, 2.0],
            values: vec![Vec3::ZERO, Vec3::new(8.0, 0.0, 0.0)],
            ..Default::default()
        },
        ..Default::default()
    };
    let clip = translation_clip("loopclamp", 2.0, vec![tracks]);

    let pos = |time: f32, policy: Loop| {
        let mut out = Vec::new();
        sample_clip_looped(&clip, &skeleton, time, policy, &mut out);
        Mat4::from_cols_array_2d(&out[0].matrix).w_axis.truncate()
    };

    // Just past the end.
    let wrapped = pos(2.1, Loop::Wrap);
    let clamped = pos(2.1, Loop::Clamp);
    // Wrap ≡ sampling at 0.1 (linearly 0.4 along x).
    assert_vec3_eq(wrapped, pos(0.1, Loop::Wrap), "Wrap repeats the clip");
    // Clamp holds the final keyframe value (8,0,0).
    assert_vec3_eq(
        clamped,
        Vec3::new(8.0, 0.0, 0.0),
        "Clamp holds final keyframe",
    );
    // Far past the end the clamp still holds — the death pose persists.
    assert_vec3_eq(
        pos(100.0, Loop::Clamp),
        Vec3::new(8.0, 0.0, 0.0),
        "Clamp holds indefinitely",
    );

    // `sample_clip` (the Wrap shorthand) matches `sample_clip_looped(Wrap)`.
    let mut shorthand = Vec::new();
    sample_clip(&clip, &skeleton, 2.1, &mut shorthand);
    assert_vec3_eq(
        Mat4::from_cols_array_2d(&shorthand[0].matrix)
            .w_axis
            .truncate(),
        wrapped,
        "sample_clip defaults to Wrap",
    );
}

/// `capture_blend` produces a per-joint local-TRS buffer that, fed back as a
/// `Snapshot` blend source, reproduces the captured pose exactly — so a
/// "smooth" interrupt resumes from the live blended pose with no discontinuity.
#[test]
fn captured_snapshot_reproduces_blended_pose() {
    let skeleton = single_root_skeleton();
    let clip_a = constant_pose_clip(
        "a",
        Vec3::new(1.0, 2.0, 3.0),
        Quat::from_rotation_y(0.3),
        Vec3::splat(1.5),
    );
    let clip_b = constant_pose_clip(
        "b",
        Vec3::new(-4.0, 0.0, 5.0),
        Quat::from_rotation_x(1.1),
        Vec3::splat(0.5),
    );
    let src_a = BlendSource::Clip {
        clip: &clip_a,
        time: 0.0,
        loop_policy: Loop::Wrap,
    };
    let src_b = BlendSource::Clip {
        clip: &clip_b,
        time: 0.0,
        loop_policy: Loop::Wrap,
    };

    // The live blended palette at an arbitrary mid-fade weight.
    let mut live = Vec::new();
    sample_blended(&src_a, &src_b, 0.4, &skeleton, &mut live);

    // Capture that same blend into a snapshot buffer.
    let mut snapshot = Vec::new();
    capture_blend(&src_a, &src_b, 0.4, &skeleton, &mut snapshot);
    assert_eq!(snapshot.len(), skeleton.joints.len());

    // Feeding the snapshot back at weight 0 (snapshot vs anything) reproduces
    // the captured pose — the interrupt has no discontinuity.
    let snap_src = BlendSource::Snapshot(&snapshot);
    let mut resumed = Vec::new();
    sample_blended(&snap_src, &src_a, 0.0, &skeleton, &mut resumed);
    for (l, r) in live.iter().zip(resumed.iter()) {
        assert_mat4_eq(
            Mat4::from_cols_array_2d(&r.matrix),
            Mat4::from_cols_array_2d(&l.matrix),
            "snapshot reproduces live blended pose",
        );
    }
}

/// A snapshot-fade interrupted again captures `blend(snapshot, clip)` through
/// the same path — the snapshot arm works as either blend operand.
#[test]
fn capture_blends_snapshot_against_clip() {
    let skeleton = single_root_skeleton();
    let snapshot = vec![LocalTrs {
        translation: Vec3::new(2.0, 0.0, 0.0),
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    }];
    let clip = constant_pose_clip("c", Vec3::new(0.0, 6.0, 0.0), Quat::IDENTITY, Vec3::ONE);
    let snap_src = BlendSource::Snapshot(&snapshot);
    let clip_src = BlendSource::Clip {
        clip: &clip,
        time: 0.0,
        loop_policy: Loop::Wrap,
    };

    let mut captured = Vec::new();
    capture_blend(&snap_src, &clip_src, 0.5, &skeleton, &mut captured);
    // Component lerp of the two translations at 0.5.
    assert_vec3_eq(
        captured[0].translation,
        Vec3::new(1.0, 3.0, 0.0),
        "snapshot×clip translation lerp",
    );
}

/// A snapshot source shorter than the skeleton falls back to rest for the
/// joints past its end (mirroring a short clip) — no panic, no garbage.
#[test]
fn snapshot_shorter_than_skeleton_holds_rest() {
    // Two joints; rest scale 0.5 on the second so a rest fallback is visible.
    let rest_child = RestLocal {
        translation: Vec3::new(0.0, 7.0, 0.0),
        rotation: Quat::IDENTITY,
        scale: Vec3::splat(0.5),
    };
    let skeleton = Skeleton {
        joints: vec![
            joint(None, Mat4::IDENTITY, RestLocal::default()),
            joint(Some(0), Mat4::IDENTITY, rest_child),
        ],
    };
    // Snapshot covers only joint 0.
    let snapshot = vec![LocalTrs {
        translation: Vec3::new(3.0, 0.0, 0.0),
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    }];
    let snap_src = BlendSource::Snapshot(&snapshot);

    let mut captured = Vec::new();
    // Blend snapshot against itself at weight 0 so the output equals the
    // snapshot's resolved locals (rest fallback included for joint 1).
    capture_blend(&snap_src, &snap_src, 0.0, &skeleton, &mut captured);
    assert_eq!(captured.len(), 2);
    assert_vec3_eq(
        captured[0].translation,
        Vec3::new(3.0, 0.0, 0.0),
        "joint 0 from snapshot",
    );
    assert_vec3_eq(
        captured[1].translation,
        rest_child.translation,
        "joint 1 holds rest translation",
    );
    assert_vec3_eq(
        captured[1].scale,
        rest_child.scale,
        "joint 1 holds rest scale",
    );
}

/// A two-joint skeleton with NON-IDENTITY inverse-bind matrices on both
/// joints, so `worldPose * inverseBind` is a meaningful transform — an
/// identity inverse-bind would let the world/palette comparison pass even if
/// the factored core were wrong.
fn two_joint_skeleton_nonidentity_ib() -> (Skeleton, Mat4, Mat4) {
    let root_ib = Mat4::from_translation(Vec3::new(-2.0, 1.0, 0.0));
    let child_ib = Mat4::from_scale_rotation_translation(
        Vec3::new(2.0, 2.0, 2.0),
        Quat::from_rotation_x(0.7),
        Vec3::new(0.0, -5.0, 3.0),
    );
    let skeleton = Skeleton {
        joints: vec![
            joint(
                None,
                root_ib,
                RestLocal {
                    translation: Vec3::new(2.0, 0.0, 0.0),
                    ..Default::default()
                },
            ),
            joint(
                Some(0),
                child_ib,
                RestLocal {
                    translation: Vec3::new(0.0, 3.0, 0.0),
                    ..Default::default()
                },
            ),
        ],
    };
    (skeleton, root_ib, child_ib)
}

/// The world-joint sampler's output, multiplied per joint by that joint's
/// inverse-bind matrix, equals the skinning palette for the SAME single-clip
/// inputs (with a loop policy). Non-identity inverse-binds make the per-joint
/// multiply load-bearing, so this proves the shared forward-sweep core
/// produces the world pose the palette path applies inverse-bind to.
#[test]
fn world_clip_sampler_times_inverse_bind_equals_palette() {
    let (skeleton, root_ib, child_ib) = two_joint_skeleton_nonidentity_ib();
    // Animate the child translation so the pose is non-trivial at the sampled
    // time, and use Clamp past the end so the loop policy is exercised too.
    let child_tracks = JointTracks {
        translation: Track {
            times: vec![0.0, 2.0],
            values: vec![Vec3::new(0.0, 3.0, 0.0), Vec3::new(4.0, 3.0, -1.0)],
            ..Default::default()
        },
        ..Default::default()
    };
    let clip = translation_clip("walk", 2.0, vec![JointTracks::default(), child_tracks]);

    let ibs = [root_ib, child_ib];
    for (time, policy) in [(0.5, Loop::Wrap), (3.0, Loop::Clamp), (2.1, Loop::Wrap)] {
        let mut palette = Vec::new();
        sample_clip_looped(&clip, &skeleton, time, policy, &mut palette);
        let mut world = Vec::new();
        sample_clip_looped_world(&clip, &skeleton, time, policy, &mut world);

        assert_eq!(world.len(), skeleton.joints.len());
        for (j, ib) in ibs.iter().enumerate() {
            let recovered = world[j] * *ib;
            assert_mat4_eq(
                recovered,
                Mat4::from_cols_array_2d(&palette[j].matrix),
                &format!("joint {j} worldPose*inverseBind == palette (time={time}, {policy:?})"),
            );
        }
    }
}

/// Same equivalence for a two-source blend at a weight: the world-joint
/// blend sampler's output, multiplied per joint by the inverse-bind matrix,
/// equals the blended skinning palette. Non-identity inverse-binds again make
/// the comparison meaningful.
#[test]
fn world_blend_sampler_times_inverse_bind_equals_palette() {
    let (skeleton, root_ib, child_ib) = two_joint_skeleton_nonidentity_ib();
    let clip_a = constant_pose_clip(
        "a",
        Vec3::new(1.0, 2.0, 0.0),
        Quat::from_rotation_z(0.4),
        Vec3::splat(1.0),
    );
    let clip_b = constant_pose_clip(
        "b",
        Vec3::new(-3.0, 0.0, 2.0),
        Quat::from_rotation_y(1.2),
        Vec3::splat(1.5),
    );
    let src_a = BlendSource::Clip {
        clip: &clip_a,
        time: 0.0,
        loop_policy: Loop::Wrap,
    };
    let src_b = BlendSource::Clip {
        clip: &clip_b,
        time: 0.0,
        loop_policy: Loop::Wrap,
    };

    let ibs = [root_ib, child_ib];
    for weight in [0.0, 0.35, 1.0] {
        let mut palette = Vec::new();
        sample_blended(&src_a, &src_b, weight, &skeleton, &mut palette);
        let mut world = Vec::new();
        sample_blended_world(&src_a, &src_b, weight, &skeleton, &mut world);

        assert_eq!(world.len(), skeleton.joints.len());
        for (j, ib) in ibs.iter().enumerate() {
            let recovered = world[j] * *ib;
            assert_mat4_eq(
                recovered,
                Mat4::from_cols_array_2d(&palette[j].matrix),
                &format!("joint {j} blended worldPose*inverseBind == palette (weight={weight})"),
            );
        }
    }
}

/// The world-joint samplers honor the caller-reused-buffer allocation
/// contract: `out` is cleared/resized to the joint count, and a warmed
/// steady-state call neither reallocates `out` nor changes the result.
#[test]
fn world_samplers_reuse_out_buffer_steady_state() {
    let (skeleton, _, _) = two_joint_skeleton_nonidentity_ib();
    let clip = translation_clip(
        "rest",
        1.0,
        vec![JointTracks::default(), JointTracks::default()],
    );

    // Stale, oversized buffer must be cleared and resized to the joint count.
    let mut out = vec![Mat4::from_scale(Vec3::splat(9.0)); 5];
    sample_clip_looped_world(&clip, &skeleton, 0.0, Loop::Wrap, &mut out);
    assert_eq!(
        out.len(),
        skeleton.joints.len(),
        "out resized to joint count"
    );

    // Warm so capacity is sized, then assert steady-state reuse allocates
    // nothing and stays deterministic.
    let cap = out.capacity();
    let first = out.clone();
    for _ in 0..16 {
        sample_clip_looped_world(&clip, &skeleton, 0.0, Loop::Wrap, &mut out);
    }
    assert_eq!(
        out.capacity(),
        cap,
        "world clip sampler does not reallocate out"
    );
    assert_eq!(out, first, "world clip sampler reuse is deterministic");
}

/// Steady-state blended sampling reuses both thread-locals and the caller's
/// `out`, so a warmed call allocates nothing. Probed by capacity stability:
/// after a warm-up the buffers are sized, and a subsequent call neither grows
/// `out`'s capacity nor changes the result.
#[test]
fn blended_sampling_reuses_scratch_steady_state() {
    let skeleton = single_root_skeleton();
    let clip_a = constant_pose_clip("a", Vec3::new(1.0, 0.0, 0.0), Quat::IDENTITY, Vec3::ONE);
    let clip_b = constant_pose_clip("b", Vec3::new(0.0, 1.0, 0.0), Quat::IDENTITY, Vec3::ONE);
    let src_a = BlendSource::Clip {
        clip: &clip_a,
        time: 0.0,
        loop_policy: Loop::Wrap,
    };
    let src_b = BlendSource::Clip {
        clip: &clip_b,
        time: 0.0,
        loop_policy: Loop::Wrap,
    };

    let mut out = Vec::new();
    // Warm-up: grows `out` and the thread-locals to skeleton size once.
    sample_blended(&src_a, &src_b, 0.5, &skeleton, &mut out);
    let cap_after_warm = out.capacity();
    let first = out[0];

    // Steady state: the reused `out` must not reallocate.
    for _ in 0..16 {
        sample_blended(&src_a, &src_b, 0.5, &skeleton, &mut out);
    }
    assert_eq!(
        out.capacity(),
        cap_after_warm,
        "out not reallocated in steady state"
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0], first, "blended reuse is deterministic");
}

#[test]
fn modified_samplers_fall_through_when_stack_or_inputs_are_absent() {
    let skeleton = single_root_skeleton();
    let clip_a = constant_pose_clip("a", Vec3::new(1.0, 0.0, 0.0), Quat::IDENTITY, Vec3::ONE);
    let clip_b = constant_pose_clip("b", Vec3::new(0.0, 1.0, 0.0), Quat::IDENTITY, Vec3::ONE);
    let inputs = PoseInputs {
        aim_pitch: 0.25,
        aim_yaw: 0.5,
        heading_yaw: 0.1,
    };

    let mut expected_clip = Vec::new();
    sample_clip_looped(&clip_a, &skeleton, 0.0, Loop::Wrap, &mut expected_clip);
    let mut actual_clip = Vec::new();
    sample_clip_looped_modified(
        &clip_a,
        &skeleton,
        0.0,
        Loop::Wrap,
        &PoseModifierStack::default(),
        Some(&inputs),
        &mut actual_clip,
    );
    assert_eq!(actual_clip, expected_clip);
    assert_eq!(
        bytemuck::cast_slice::<BonePaletteEntry, u8>(&actual_clip),
        bytemuck::cast_slice::<BonePaletteEntry, u8>(&expected_clip),
        "an empty stack is byte-identical to the unmodified palette"
    );

    let stack = PoseModifierStack::new(vec![ModifierEntry {
        mask: Default::default(),
        modifier: PoseModifier::AimPitchBend {
            bend_weights: Vec::new(),
        },
    }]);
    let source_a = BlendSource::Clip {
        clip: &clip_a,
        time: 0.0,
        loop_policy: Loop::Wrap,
    };
    let source_b = BlendSource::Clip {
        clip: &clip_b,
        time: 0.0,
        loop_policy: Loop::Wrap,
    };
    let mut expected_blend = Vec::new();
    sample_blended(&source_a, &source_b, 0.4, &skeleton, &mut expected_blend);
    let mut actual_blend = Vec::new();
    sample_blended_modified(
        &source_a,
        &source_b,
        0.4,
        &skeleton,
        &stack,
        None,
        &mut actual_blend,
    );
    assert_eq!(actual_blend, expected_blend);
}

fn mask(indices: &[usize]) -> JointMask {
    let mut mask = JointMask::new();
    for &index in indices {
        assert!(mask.insert(index));
    }
    mask
}

fn rest_skeleton(parents: &[Option<usize>], rotations: &[Quat]) -> Skeleton {
    assert_eq!(parents.len(), rotations.len());
    Skeleton {
        joints: parents
            .iter()
            .zip(rotations)
            .map(|(&parent, &rotation)| {
                joint(
                    parent,
                    Mat4::IDENTITY,
                    RestLocal {
                        rotation,
                        ..Default::default()
                    },
                )
            })
            .collect(),
    }
}

fn rest_clip(joint_count: usize) -> AnimationClip {
    translation_clip("rest", 1.0, vec![JointTracks::default(); joint_count])
}

fn palette_rotation(palette: &[BonePaletteEntry], joint_index: usize) -> Quat {
    Mat4::from_cols_array_2d(&palette[joint_index].matrix)
        .to_scale_rotation_translation()
        .1
}

fn sample_modified_rest(
    skeleton: &Skeleton,
    stack: &PoseModifierStack,
    inputs: &PoseInputs,
) -> Vec<BonePaletteEntry> {
    let clip = rest_clip(skeleton.joints.len());
    let mut palette = Vec::new();
    sample_clip_looped_modified(
        &clip,
        skeleton,
        0.0,
        Loop::Wrap,
        stack,
        Some(inputs),
        &mut palette,
    );
    palette
}

#[test]
fn aim_pitch_bend_reaches_chain_tip_and_leaves_off_chain_root_unchanged() {
    let skeleton = rest_skeleton(
        &[None, Some(0), None],
        &[Quat::IDENTITY, Quat::IDENTITY, Quat::from_rotation_z(0.2)],
    );
    let pitch = 0.6;
    let stack = PoseModifierStack::new(vec![ModifierEntry {
        mask: mask(&[0, 1]),
        modifier: PoseModifier::AimPitchBend {
            bend_weights: Vec::new(),
        },
    }]);

    let palette = sample_modified_rest(
        &skeleton,
        &stack,
        &PoseInputs {
            aim_pitch: pitch,
            ..Default::default()
        },
    );

    assert_quat_eq(
        palette_rotation(&palette, 1),
        Quat::from_rotation_x(pitch),
        "equal bend shares sum to the requested tip pitch",
    );
    assert_quat_eq(
        palette_rotation(&palette, 2),
        Quat::from_rotation_z(0.2),
        "off-chain root holds its sampled rotation",
    );

    let tip_forward = palette_rotation(&palette, 1) * -Vec3::Z;
    assert!(
        tip_forward.y > 0.0,
        "positive aim pitch bends -Z forward upward"
    );
}

#[test]
fn weighted_aim_pitch_bend_preserves_ratio_and_total_tip_pitch() {
    let skeleton = rest_skeleton(&[None, Some(0)], &[Quat::IDENTITY; 2]);
    let pitch = 0.9;
    let stack = PoseModifierStack::new(vec![ModifierEntry {
        mask: mask(&[0, 1]),
        modifier: PoseModifier::AimPitchBend {
            bend_weights: vec![1.0, 2.0],
        },
    }]);
    let palette = sample_modified_rest(
        &skeleton,
        &stack,
        &PoseInputs {
            aim_pitch: pitch,
            ..Default::default()
        },
    );

    let root = palette_rotation(&palette, 0);
    let tip = palette_rotation(&palette, 1);
    assert_quat_eq(
        root,
        Quat::from_rotation_x(pitch / 3.0),
        "root gets weight 1",
    );
    assert_quat_eq(
        root.inverse() * tip,
        Quat::from_rotation_x(pitch * 2.0 / 3.0),
        "child gets twice the root bend",
    );
    assert_quat_eq(
        tip,
        Quat::from_rotation_x(pitch),
        "normalized weights preserve total tip pitch",
    );
}

#[test]
fn upper_lower_split_wraps_delta_and_twists_upper_with_half_seam() {
    let skeleton = rest_skeleton(&[None, None, None, None], &[Quat::IDENTITY; 4]);
    let stack = PoseModifierStack::new(vec![ModifierEntry {
        mask: mask(&[1, 2]),
        modifier: PoseModifier::UpperLowerSplit {
            lower_body_mask: mask(&[0, 1]),
        },
    }]);
    let delta = 20.0_f32.to_radians();
    let palette = sample_modified_rest(
        &skeleton,
        &stack,
        &PoseInputs {
            aim_yaw: (-170.0_f32).to_radians(),
            heading_yaw: 170.0_f32.to_radians(),
            ..Default::default()
        },
    );

    assert_quat_eq(
        palette_rotation(&palette, 0),
        Quat::IDENTITY,
        "lower-only joint",
    );
    assert_quat_eq(
        palette_rotation(&palette, 1),
        Quat::from_rotation_y(delta * 0.5),
        "upper/lower seam gets half the shortest yaw delta",
    );
    assert_quat_eq(
        palette_rotation(&palette, 2),
        Quat::from_rotation_y(delta),
        "upper-only joint gets the shortest yaw delta",
    );
    assert_quat_eq(
        palette_rotation(&palette, 3),
        Quat::IDENTITY,
        "unmasked joint",
    );
}

#[test]
fn overlapping_pose_modifiers_compose_in_stack_list_order() {
    let skeleton = rest_skeleton(&[None], &[Quat::IDENTITY]);
    let joint = mask(&[0]);
    let split = ModifierEntry {
        mask: joint,
        modifier: PoseModifier::UpperLowerSplit {
            lower_body_mask: JointMask::new(),
        },
    };
    let bend = ModifierEntry {
        mask: joint,
        modifier: PoseModifier::AimPitchBend {
            bend_weights: Vec::new(),
        },
    };
    let inputs = PoseInputs {
        aim_pitch: 0.4,
        aim_yaw: 0.6,
        heading_yaw: 0.0,
    };

    let split_then_bend = sample_modified_rest(
        &skeleton,
        &PoseModifierStack::new(vec![split.clone(), bend.clone()]),
        &inputs,
    );
    let bend_then_split = sample_modified_rest(
        &skeleton,
        &PoseModifierStack::new(vec![bend, split]),
        &inputs,
    );

    let first = palette_rotation(&split_then_bend, 0);
    let second = palette_rotation(&bend_then_split, 0);
    assert_quat_eq(
        first,
        Quat::from_rotation_y(0.6) * Quat::from_rotation_x(0.4),
        "later bend observes and composes after split",
    );
    assert_quat_eq(
        second,
        Quat::from_rotation_x(0.4) * Quat::from_rotation_y(0.6),
        "reversing entries reverses composition",
    );
    assert!(
        first.dot(second).abs() < 0.999,
        "noncommuting order is observable"
    );
}

#[test]
fn identical_pose_inputs_produce_byte_identical_palettes() {
    let skeleton = rest_skeleton(&[None, Some(0)], &[Quat::IDENTITY; 2]);
    let stack = PoseModifierStack::new(vec![ModifierEntry {
        mask: mask(&[0, 1]),
        modifier: PoseModifier::AimPitchBend {
            bend_weights: vec![3.0, 1.0],
        },
    }]);
    let inputs = PoseInputs {
        aim_pitch: 0.37,
        aim_yaw: -0.8,
        heading_yaw: 0.2,
    };

    let first = sample_modified_rest(&skeleton, &stack, &inputs);
    let second = sample_modified_rest(&skeleton, &stack, &inputs);
    assert_eq!(
        bytemuck::cast_slice::<BonePaletteEntry, u8>(&first),
        bytemuck::cast_slice::<BonePaletteEntry, u8>(&second),
    );
}

#[test]
fn active_bend_does_not_modify_world_pose_samplers_used_by_hit_zones() {
    let skeleton = rest_skeleton(&[None, Some(0)], &[Quat::IDENTITY; 2]);
    let clip = rest_clip(2);
    let stack = PoseModifierStack::new(vec![ModifierEntry {
        mask: mask(&[0, 1]),
        modifier: PoseModifier::AimPitchBend {
            bend_weights: Vec::new(),
        },
    }]);
    let inputs = PoseInputs {
        aim_pitch: 0.5,
        ..Default::default()
    };

    let mut expected_world = Vec::new();
    sample_clip_looped_world(&clip, &skeleton, 0.0, Loop::Wrap, &mut expected_world);
    let mut modified_palette = Vec::new();
    sample_clip_looped_modified(
        &clip,
        &skeleton,
        0.0,
        Loop::Wrap,
        &stack,
        Some(&inputs),
        &mut modified_palette,
    );
    let mut actual_world = Vec::new();
    sample_clip_looped_world(&clip, &skeleton, 0.0, Loop::Wrap, &mut actual_world);

    assert_eq!(
        actual_world, expected_world,
        "hit-zone sampler remains unmodified"
    );
    assert_ne!(
        Mat4::from_cols_array_2d(&modified_palette[1].matrix),
        actual_world[1],
        "the active visual bend is observable only in the palette path"
    );

    let source_a = BlendSource::Clip {
        clip: &clip,
        time: 0.0,
        loop_policy: Loop::Wrap,
    };
    let source_b = BlendSource::Clip {
        clip: &clip,
        time: 0.5,
        loop_policy: Loop::Wrap,
    };
    let mut expected_blended_world = Vec::new();
    sample_blended_world(
        &source_a,
        &source_b,
        0.5,
        &skeleton,
        &mut expected_blended_world,
    );
    let mut modified_blended_palette = Vec::new();
    sample_blended_modified(
        &source_a,
        &source_b,
        0.5,
        &skeleton,
        &stack,
        Some(&inputs),
        &mut modified_blended_palette,
    );
    let mut actual_blended_world = Vec::new();
    sample_blended_world(
        &source_a,
        &source_b,
        0.5,
        &skeleton,
        &mut actual_blended_world,
    );

    assert_eq!(
        actual_blended_world, expected_blended_world,
        "blended hit-zone sampler remains unmodified"
    );
    assert_ne!(
        Mat4::from_cols_array_2d(&modified_blended_palette[1].matrix),
        actual_blended_world[1],
        "active blended visual bend is absent from hit-zone output"
    );
}

#[test]
fn combined_split_and_bend_aims_torso_while_legs_keep_heading() {
    let heading = 0.2;
    let aim_yaw = 0.7;
    let pitch = 0.3;
    let skeleton = rest_skeleton(
        &[None, Some(0), Some(0)],
        &[
            Quat::from_rotation_y(heading),
            Quat::IDENTITY,
            Quat::IDENTITY,
        ],
    );
    let torso = mask(&[1]);
    let lower = mask(&[0, 2]);
    let stack = PoseModifierStack::new(vec![
        ModifierEntry {
            mask: torso,
            modifier: PoseModifier::UpperLowerSplit {
                lower_body_mask: lower,
            },
        },
        ModifierEntry {
            mask: torso,
            modifier: PoseModifier::AimPitchBend {
                bend_weights: Vec::new(),
            },
        },
    ]);
    let palette = sample_modified_rest(
        &skeleton,
        &stack,
        &PoseInputs {
            aim_pitch: pitch,
            aim_yaw,
            heading_yaw: heading,
        },
    );

    assert_quat_eq(
        palette_rotation(&palette, 1),
        Quat::from_rotation_y(aim_yaw) * Quat::from_rotation_x(pitch),
        "torso tracks scalar aim yaw and pitch",
    );
    assert_quat_eq(
        palette_rotation(&palette, 2),
        Quat::from_rotation_y(heading),
        "leg keeps the sampled body heading",
    );
}
