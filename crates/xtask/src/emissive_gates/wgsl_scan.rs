// WGSL scanning for the emissive gates: every emissive texture sample must
// end `.rgb`.

use crate::surface_gates::{is_identifier_char, line_of};

use super::script_shapes::{Lang, strip_script_comments};
use super::{identifier_at, matching_close};

/// WGSL with `//` and `/* */` comments blanked.
pub(super) fn strip_wgsl_comments(text: &str) -> String {
    strip_script_comments(text, Lang::Ts)
}

/// Each sample of an emissive texture in `wgsl` that does not end `.rgb`, as
/// (line, binding); plus how many samples were read. An emissive texture is a
/// `var` whose name names `emissive` and whose type is a texture.
pub(super) fn emissive_alpha_reads(wgsl: &str) -> (Vec<(usize, String)>, usize) {
    let code = strip_wgsl_comments(wgsl);
    let mut bindings = Vec::new();
    for at in identifier_at(&code, "var") {
        let rest = &code[at + 3..];
        let rest = rest
            .trim_start()
            .strip_prefix('<')
            .and_then(|r| r.split_once('>').map(|(_, r)| r))
            .unwrap_or(rest)
            .trim_start();
        let name: String = rest
            .chars()
            .take_while(|&c| is_identifier_char(c))
            .collect();
        let ty = rest[name.len()..]
            .trim_start()
            .strip_prefix(':')
            .map(str::trim_start);
        if name.to_lowercase().contains("emissive")
            && ty.is_some_and(|ty| ty.starts_with("texture_"))
        {
            bindings.push(name);
        }
    }
    let mut reads = Vec::new();
    let mut samples = 0;
    for binding in bindings {
        for at in identifier_at(&code, &binding) {
            // The declaration itself.
            if code[..at].trim_end().ends_with('>') || code[..at].trim_end().ends_with("var") {
                continue;
            }
            samples += 1;
            let mut depth = 0i32;
            let open = code[..at]
                .char_indices()
                .rev()
                .find_map(|(offset, c)| match c {
                    ')' => {
                        depth += 1;
                        None
                    }
                    '(' if depth == 0 => Some(offset),
                    '(' => {
                        depth -= 1;
                        None
                    }
                    ';' | '{' | '}' if depth == 0 => Some(usize::MAX),
                    _ => None,
                });
            let ends_rgb = open
                .filter(|&open| open != usize::MAX)
                .and_then(|open| matching_close(&code, open))
                .is_some_and(|close| {
                    let after = code[close + 1..].trim_start();
                    after
                        .strip_prefix(".rgb")
                        .is_some_and(|rest| !rest.chars().next().is_some_and(is_identifier_char))
                });
            if !ends_rgb {
                reads.push((line_of(&code, at), binding.clone()));
            }
        }
    }
    (reads, samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emissive_sample_scan_flags_alpha_and_whole_texel_reads() {
        let wgsl = "@group(1) @binding(1) var emissive_texture: texture_2d<f32>;\n\
                    // textureSample(emissive_texture, s, uv).a in a comment\n\
                    fn f() {\n\
                      let a = sample(emissive_texture, s, uv, d(x)).rgb;\n\
                      let b = textureSample(emissive_texture, s, uv).a;\n\
                      let c = textureSample(emissive_texture, s, uv).rgba;\n\
                      let d = textureSample(emissive_texture, s, uv);\n\
                    }\n";
        let (reads, samples) = emissive_alpha_reads(wgsl);
        assert_eq!(samples, 4);
        let lines: Vec<usize> = reads.iter().map(|(line, _)| *line).collect();
        assert_eq!(lines, [5, 6, 7]);
    }
}
