use imgproc::{
    parse_size, process_texture, read_manifest, SpecProfile, TextureJob, DEFAULT_NORMAL_STRENGTH,
    DEFAULT_SPEC_SCALE, DEFAULT_TEXTURE_SIZE,
};
use std::env;
use std::error::Error;
use std::io;
use std::path::PathBuf;
use std::str::FromStr;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        print_usage();
        return Err(invalid_input("missing command"));
    };

    match command.as_str() {
        "process" => {
            let command = ProcessCommand::parse(args.collect())?;
            process_texture(&command.job, &command.out_dir)?;
            println!("wrote {}", command.job.stem);
        }
        "batch" => {
            let command = BatchCommand::parse(args.collect())?;
            let jobs = read_manifest(&command.manifest)?;
            if jobs.is_empty() {
                return Err(invalid_input(format!(
                    "{} contains no texture entries",
                    command.manifest.display()
                )));
            }
            for job in jobs {
                process_texture(&job, &command.out_dir)?;
                println!("wrote {}", job.stem);
            }
        }
        "-h" | "--help" | "help" => {
            print_usage();
        }
        _ => {
            print_usage();
            return Err(invalid_input(format!("unknown command {command:?}")));
        }
    }

    Ok(())
}

struct ProcessCommand {
    job: TextureJob,
    out_dir: PathBuf,
}

impl ProcessCommand {
    fn parse(args: Vec<String>) -> Result<Self, Box<dyn Error>> {
        let mut parser = ArgParser::new(args);
        let src = parser.required_path("--src")?;
        let stem = parser.required_string("--stem")?;
        let out_dir = parser.required_path("--out-dir")?;
        let size = parser
            .optional_size("--size")?
            .unwrap_or(DEFAULT_TEXTURE_SIZE);
        let tileable = parser.flag("--tileable")?;
        let spec_scale = parser
            .optional_f32("--spec-scale")?
            .unwrap_or(DEFAULT_SPEC_SCALE);
        let spec_profile = parser
            .optional_spec_profile("--spec-profile")?
            .unwrap_or(SpecProfile::Luminance);
        let spec_base = parser.optional_f32("--spec-base")?;
        let spec_gamma = parser.optional_f32("--spec-gamma")?;
        let spec_edge_damping = parser.optional_f32("--spec-edge-damping")?;
        let normal_strength = parser
            .optional_f32("--normal-strength")?
            .unwrap_or(DEFAULT_NORMAL_STRENGTH);
        let quantize_levels = parser.optional_quantize_levels("--quantize-levels")?;
        let height_strength = parser.optional_f32("--height-strength")?;
        let height_quantize_levels = parser.optional_quantize_levels("--height-quantize-levels")?;
        parser.finish()?;

        let job = TextureJob {
            src,
            stem,
            width: size.width,
            height: size.height,
            tileable,
            spec_scale,
            spec_profile,
            spec_base,
            spec_gamma,
            spec_edge_damping,
            normal_strength,
            quantize_levels,
            height_strength,
            height_quantize_levels,
        };
        job.validate().map_err(invalid_input)?;

        Ok(Self { job, out_dir })
    }
}

struct BatchCommand {
    manifest: PathBuf,
    out_dir: PathBuf,
}

impl BatchCommand {
    fn parse(args: Vec<String>) -> Result<Self, Box<dyn Error>> {
        let mut parser = ArgParser::new(args);
        let manifest = parser.required_path("--manifest")?;
        let out_dir = parser.required_path("--out-dir")?;
        parser.finish()?;
        Ok(Self { manifest, out_dir })
    }
}

struct ArgParser {
    args: Vec<String>,
    used: Vec<bool>,
}

impl ArgParser {
    fn new(args: Vec<String>) -> Self {
        let used = vec![false; args.len()];
        Self { args, used }
    }

    fn required_path(&mut self, flag: &str) -> Result<PathBuf, Box<dyn Error>> {
        Ok(PathBuf::from(self.required_string(flag)?))
    }

    fn required_string(&mut self, flag: &str) -> Result<String, Box<dyn Error>> {
        self.optional_string(flag)?
            .ok_or_else(|| invalid_input(format!("missing required flag {flag}")))
    }

    fn optional_f32(&mut self, flag: &str) -> Result<Option<f32>, Box<dyn Error>> {
        self.optional_string(flag)?
            .map(|value| {
                value
                    .parse::<f32>()
                    .map_err(|_| invalid_input(format!("{flag} must be a number")))
                    .and_then(|parsed| {
                        if parsed.is_finite() && parsed >= 0.0 {
                            Ok(parsed)
                        } else {
                            Err(invalid_input(format!(
                                "{flag} must be a finite non-negative number"
                            )))
                        }
                    })
            })
            .transpose()
    }

    fn optional_spec_profile(&mut self, flag: &str) -> Result<Option<SpecProfile>, Box<dyn Error>> {
        self.optional_string(flag)?
            .map(|value| SpecProfile::from_str(&value).map_err(invalid_input))
            .transpose()
    }

    fn optional_quantize_levels(&mut self, flag: &str) -> Result<Option<u8>, Box<dyn Error>> {
        self.optional_string(flag)?
            .map(|value| {
                value
                    .parse::<u8>()
                    .map_err(|_| invalid_input(format!("{flag} must be a u8")))
                    .map(|levels| if levels == 0 { None } else { Some(levels) })
            })
            .transpose()
            .map(Option::flatten)
    }

    fn optional_size(
        &mut self,
        flag: &str,
    ) -> Result<Option<imgproc::TextureDimensions>, Box<dyn Error>> {
        self.optional_string(flag)?
            .map(|value| {
                parse_size(&value).map_err(|message| invalid_input(format!("{flag} {message}")))
            })
            .transpose()
    }

    fn optional_string(&mut self, flag: &str) -> Result<Option<String>, Box<dyn Error>> {
        let mut found = None;
        for index in 0..self.args.len() {
            if self.used[index] || self.args[index] != flag {
                continue;
            }
            if found.is_some() {
                return Err(invalid_input(format!("duplicate flag {flag}")));
            }
            let value_index = index + 1;
            if value_index >= self.args.len()
                || self.used[value_index]
                || self.args[value_index].starts_with("--")
            {
                return Err(invalid_input(format!("missing value for {flag}")));
            }
            self.used[index] = true;
            self.used[value_index] = true;
            found = Some(self.args[value_index].clone());
        }
        Ok(found)
    }

    fn flag(&mut self, flag: &str) -> Result<bool, Box<dyn Error>> {
        let mut found = false;
        for index in 0..self.args.len() {
            if self.used[index] || self.args[index] != flag {
                continue;
            }
            if found {
                return Err(invalid_input(format!("duplicate flag {flag}")));
            }
            self.used[index] = true;
            found = true;
        }
        Ok(found)
    }

    fn finish(&self) -> Result<(), Box<dyn Error>> {
        let leftovers: Vec<&str> = self
            .args
            .iter()
            .zip(self.used.iter())
            .filter_map(|(arg, used)| (!used).then_some(arg.as_str()))
            .collect();
        if leftovers.is_empty() {
            Ok(())
        } else {
            Err(invalid_input(format!(
                "unexpected argument(s): {}",
                leftovers.join(" ")
            )))
        }
    }
}

const USAGE: &str = "\
Usage:
  imgproc process --src <png> --stem <name> --out-dir <dir> [--size <N|WxH>] [--tileable] [--spec-profile <name>] [--spec-scale <f32>] [--spec-base <0..1>] [--spec-gamma <f32>] [--spec-edge-damping <0..1>] [--normal-strength <f32>] [--quantize-levels <u8>] [--height-strength <f32>] [--height-quantize-levels <u8>]
  imgproc batch --manifest <path> --out-dir <dir>
  imgproc help | -h | --help";

fn print_usage() {
    eprintln!("{USAGE}");
}

fn invalid_input(message: impl Into<String>) -> Box<dyn Error> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

#[cfg(test)]
mod tests {
    use super::USAGE;

    /// Flag-shaped names: `-x` or `--name`.
    fn is_flag_name(token: &str) -> bool {
        let bytes = token.as_bytes();
        match bytes {
            [b'-', c] => c.is_ascii_alphabetic(),
            [b'-', b'-', first, rest @ ..] => {
                first.is_ascii_lowercase()
                    && rest
                        .iter()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
            }
            _ => false,
        }
    }

    /// Every flag-shaped string literal (`"--name"`) in the top-level item of
    /// `source` that begins at `marker`, read up to that item's closing brace.
    fn parser_flags(source: &str, marker: &str) -> std::collections::BTreeSet<String> {
        let start = source.find(marker).expect("parser item is present");
        let item = &source[start..];
        let item = &item[..item.find("\n}\n").expect("parser item closes")];
        item.split('"')
            .filter(|piece| is_flag_name(piece))
            .map(str::to_owned)
            .collect()
    }

    /// Every flag-shaped word the help text names.
    fn help_flags(help: &str) -> std::collections::BTreeSet<String> {
        help.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .filter(|word| is_flag_name(word))
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn help_lists_every_flag_the_process_and_batch_parsers_accept() {
        let source = include_str!("main.rs");
        let mut parsed = parser_flags(source, "impl ProcessCommand");
        parsed.extend(parser_flags(source, "impl BatchCommand"));
        let listed = help_flags(USAGE);
        let listed: std::collections::BTreeSet<_> = listed
            .into_iter()
            .filter(|flag| flag != "-h" && flag != "--help")
            .collect();
        assert!(!parsed.is_empty());
        assert_eq!(parsed, listed, "help and parser disagree");
    }
}
