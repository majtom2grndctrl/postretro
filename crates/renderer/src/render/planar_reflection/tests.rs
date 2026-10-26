// GPU-free planar proofs: shader validation, IP shape and the frame's planar call order.
// See: context/lib/rendering_pipeline.md §7.3a

use super::pipelines::{MASK_SHADER_SOURCE, RESOLVE_SHADER_SOURCE};

const PLANAR_SHADERS: [(&str, &str); 2] = [
    ("planar_mask.wgsl", MASK_SHADER_SOURCE),
    ("planar_resolve.wgsl", RESOLVE_SHADER_SOURCE),
];

/// Code only: comments removed, whitespace kept.
fn code(source: &str) -> String {
    source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `text` names `identifier` as a whole word.
fn names(text: &str, identifier: &str) -> bool {
    text.match_indices(identifier).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + identifier.len()..].chars().next();
        let word = |ch: Option<char>| ch.is_some_and(|ch| ch.is_alphanumeric() || ch == '_');
        !word(before) && !word(after)
    })
}

/// The argument text of each call to `function` in `text`.
fn call_arguments<'a>(text: &'a str, function: &str) -> Vec<&'a str> {
    let needle = format!("{function}(");
    text.match_indices(&needle)
        .filter(|(at, _)| {
            !text[..*at]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
        })
        .map(|(at, _)| {
            let open = at + needle.len();
            let mut depth = 1;
            let close = text[open..]
                .char_indices()
                .find_map(|(offset, ch)| {
                    match ch {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    (depth == 0).then_some(open + offset)
                })
                .expect("call closes");
            &text[open..close]
        })
        .collect()
}

/// The condition text of each `if` in `text`, up to its opening brace.
fn if_conditions(text: &str) -> Vec<&str> {
    text.match_indices("if ")
        .filter(|(at, _)| {
            !text[..*at]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
        })
        .map(|(at, _)| {
            let start = at + 3;
            &text[start..start + text[start..].find('{').expect("if body")]
        })
        .collect()
}

#[test]
fn planar_shaders_validate_without_an_adapter() {
    for (name, source) in PLANAR_SHADERS {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|error| panic!("{name} parses: {error}"));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("{name} validates: {error:?}"));
    }
}

// IP gate on the planar-authored shader text (rendering_pipeline.md §7.3a
// Planar layer IP rules). No Fresnel, no probe binding or probe helper, no
// reflecting-gate or seen-from-above call; no direction-valued identifier, nor
// the reprojected clip w, inside a comparison, select, step or smoothstep: the
// mask, resolve and weight cut on coverage, occlusion and distance alone. Task
// 6 widens it to the tilt's D4 helpers.
#[test]
fn planar_shaders_preserve_ip_boundaries() {
    const DIRECTION: [&str; 10] = [
        "view",
        "view_dir",
        "ray",
        "reflected",
        "tilted",
        "normal",
        "outward",
        "direction",
        "dir",
        "incident",
    ];
    for (name, source) in PLANAR_SHADERS {
        let code = code(source);
        let lower = code.to_lowercase();
        for banned in [
            "fresnel",
            "fluid_probe",
            "fluid_reflection_gate",
            "fluid_reflection_seen_from_above",
            "fluid_reflection_cell_within_box",
            "fluid_reflection_point",
            "probes[",
        ] {
            assert!(!lower.contains(banned), "{name} names `{banned}`");
        }
        let mut guarded = Vec::new();
        for function in ["select", "step", "smoothstep"] {
            guarded.extend(call_arguments(&code, function));
        }
        guarded.extend(if_conditions(&code));
        for text in &guarded {
            for identifier in DIRECTION {
                assert!(
                    !names(text, identifier),
                    "{name}: `{identifier}` inside a decision: {text}"
                );
            }
            assert!(
                !text.contains("mirror_clip.w"),
                "{name}: the reprojected clip w inside a decision: {text}"
            );
        }
        // The full-screen view-ray reconstruction is gone: the resolve's world
        // position is the fragment's own.
        for retired in ["abs(ray", "ray.y", "inverse_main_vp"] {
            assert!(!code.contains(retired), "{name} keeps `{retired}`");
        }
    }
}

// The frame records the planar pass once, after the ghost pass and before
// the fluid gate, on the frame's encoder; the call reads no tier, so the
// frame's one tier read stays the fluid passes' snapshot.
#[test]
fn planar_pass_records_between_the_ghost_pass_and_the_fluid_gate() {
    let frame = include_str!("../renderer_render_frame.rs");
    let ghost = frame.find(".ghost_order.record(").expect("ghost pass");
    let planar = frame
        .find("self.record_planar_reflection_pass(")
        .expect("planar pass");
    let gate = frame
        .find("if render_world && self.full().fluid_surface.has_draws()")
        .expect("fluid gate");
    assert!(ghost < planar && planar < gate, "{ghost} {planar} {gate}");
    assert_eq!(
        frame.matches("self.record_planar_reflection_pass(").count(),
        1
    );
    let call = &frame[planar..planar + frame[planar..].find(';').expect("call ends")];
    assert_eq!(
        call,
        "self.record_planar_reflection_pass(encoder, render_world, world_mesh_frame_plan)"
    );
    let record = include_str!("record.rs");
    for second_encoder in [
        "create_command_encoder",
        "RenderBundle",
        "thread::spawn",
        "rayon",
    ] {
        assert!(
            !record.contains(second_encoder),
            "planar recording stays on the frame encoder: `{second_encoder}`"
        );
    }
}
