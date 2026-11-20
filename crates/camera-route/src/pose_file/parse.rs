// The pose file's strict parser.
// See: context/lib/rendering_pipeline.md §12

use glam::Vec3;

use super::{
    GeneratedKind, HEADER_LINES, POSE_FILE_MAGIC, POSE_FILE_MAGIC_PREFIX, ParseReason, PoseFile,
    PoseFileError, Projection, RouteHeader, RouteOrigin,
};
use crate::CADENCE_HZ;
use crate::exact::RoutePose;

/// Parses a pose file strictly: every header line once and in order, at
/// least one pose, finite floats, nothing else. Four Windows-editor habits
/// are tolerated because they carry no meaning: a leading UTF-8 BOM, CRLF
/// line endings, trailing whitespace on a line and blank lines at the end of
/// the file. A blank line anywhere else is refused.
pub fn parse_pose_file(text: &str) -> Result<PoseFile, PoseFileError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut all: Vec<(usize, &str)> = text
        .lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line.trim_end()))
        .collect();
    while all.last().is_some_and(|(_, line)| line.is_empty()) {
        all.pop();
    }
    let mut lines = all.into_iter();
    let Some((_, magic)) = lines.next() else {
        return Err(PoseFileError {
            line: 1,
            reason: ParseReason::Empty,
        });
    };
    if magic != POSE_FILE_MAGIC {
        let version = magic
            .strip_prefix(POSE_FILE_MAGIC_PREFIX)
            .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()));
        return Err(PoseFileError {
            line: 1,
            reason: match version {
                Some(version) => ParseReason::UnsupportedVersion {
                    version: version.to_string(),
                },
                None => ParseReason::NotAPoseFile {
                    found: magic.to_string(),
                },
            },
        });
    }

    let mut header = HeaderParts::default();
    let mut poses = Vec::new();
    let mut last = 1;
    for (number, line) in lines {
        last = number;
        let at = |reason| PoseFileError {
            line: number,
            reason,
        };
        if line.is_empty() {
            return Err(at(ParseReason::BlankLine));
        }
        let (keyword, rest) = line.split_once(' ').unwrap_or((line, ""));
        let next_header = HEADER_LINES.get(header.seen);
        match next_header {
            Some(&expected) if keyword == expected => {
                header.take(keyword, rest).map_err(at)?;
                header.seen += 1;
            }
            Some(&expected) if is_keyword(keyword) => {
                return Err(at(ParseReason::OutOfOrder {
                    expected,
                    found: keyword.to_string(),
                }));
            }
            Some(_) => return Err(at(ParseReason::UnknownLine(line.to_string()))),
            None => match keyword {
                "spawn" if header.spawn.is_none() && poses.is_empty() => {
                    header.spawn = Some(parse_vec3(rest, "spawn").map_err(at)?);
                }
                "pose" => poses.push(parse_pose(rest).map_err(at)?),
                keyword if is_keyword(keyword) => {
                    return Err(at(ParseReason::OutOfOrder {
                        expected: "pose",
                        found: keyword.to_string(),
                    }));
                }
                _ => return Err(at(ParseReason::UnknownLine(line.to_string()))),
            },
        }
    }
    let end = |reason| PoseFileError {
        line: last + 1,
        reason,
    };
    if let Some(&expected) = HEADER_LINES.get(header.seen) {
        return Err(end(ParseReason::Missing { expected }));
    }
    if poses.is_empty() {
        return Err(end(ParseReason::NoPoses));
    }
    Ok(PoseFile {
        header: header.finish(),
        poses,
    })
}

/// The hex digits `bake_identity` keeps of the id-51 hash.
const ID51_HEX_LEN: usize = 16;

fn is_keyword(keyword: &str) -> bool {
    HEADER_LINES.contains(&keyword) || keyword == "spawn" || keyword == "pose"
}

/// The header as parsed so far; `seen` counts its lines.
#[derive(Default)]
struct HeaderParts {
    seen: usize,
    origin: Option<RouteOrigin>,
    params: Vec<(String, String)>,
    projection: Option<Projection>,
    bake: String,
    cadence_hz: u32,
    spawn: Option<Vec3>,
}

impl HeaderParts {
    fn take(&mut self, keyword: &str, rest: &str) -> Result<(), ParseReason> {
        match keyword {
            "origin" => self.origin = Some(parse_origin(rest)?),
            "params" => self.params = parse_params(rest)?,
            "projection" => self.projection = Some(parse_projection(rest)?),
            "bake" => self.bake = parse_bake(rest)?,
            "cadence_hz" => {
                self.cadence_hz = parse_int::<u32>(rest, "cadence_hz")?;
                if self.cadence_hz != CADENCE_HZ {
                    return Err(malformed(
                        "cadence_hz",
                        format!(
                            "the cadence is fixed at {CADENCE_HZ} Hz, found {}",
                            self.cadence_hz
                        ),
                    ));
                }
            }
            _ => unreachable!("only header keywords reach `take`"),
        }
        Ok(())
    }

    fn finish(self) -> RouteHeader {
        RouteHeader {
            origin: self.origin.expect("origin parsed"),
            params: self.params,
            projection: self.projection.expect("projection parsed"),
            bake: self.bake,
            cadence_hz: self.cadence_hz,
            spawn: self.spawn,
        }
    }
}

fn malformed(line: &'static str, why: impl Into<String>) -> ParseReason {
    ParseReason::Malformed {
        line,
        why: why.into(),
    }
}

fn parse_float(text: &str, field: &'static str) -> Result<f32, ParseReason> {
    text.parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| ParseReason::BadFloat {
            field,
            text: text.to_string(),
        })
}

fn parse_int<T: std::str::FromStr>(text: &str, field: &'static str) -> Result<T, ParseReason> {
    text.parse::<T>().map_err(|_| ParseReason::BadInteger {
        field,
        text: text.to_string(),
    })
}

fn parse_origin(rest: &str) -> Result<RouteOrigin, ParseReason> {
    if rest == "recorded" {
        return Ok(RouteOrigin::Recorded);
    }
    rest.strip_prefix("generated ")
        .and_then(GeneratedKind::from_key)
        .map(RouteOrigin::Generated)
        .ok_or_else(|| {
            malformed(
                "origin",
                format!(
                    "expected `generated far-point|random|smoothed` or `recorded`, found {rest:?}"
                ),
            )
        })
}

fn parse_params(rest: &str) -> Result<Vec<(String, String)>, ParseReason> {
    let mut params: Vec<(String, String)> = Vec::new();
    if rest.is_empty() {
        return Ok(params);
    }
    for token in rest.split(' ') {
        let Some((key, value)) = token.split_once('=').filter(|(key, _)| !key.is_empty()) else {
            return Err(malformed(
                "params",
                format!("expected key=value, found {token:?}"),
            ));
        };
        if params.iter().any(|(k, _)| k == key) {
            return Err(malformed("params", format!("duplicate key {key:?}")));
        }
        params.push((key.to_string(), value.to_string()));
    }
    Ok(params)
}

fn parse_projection(rest: &str) -> Result<Projection, ParseReason> {
    let mut tokens = rest.split(' ');
    let mut field = |key: &'static str| {
        let token = tokens.next().unwrap_or_default();
        let value = token
            .strip_prefix(key)
            .and_then(|t| t.strip_prefix('='))
            .ok_or_else(|| {
                malformed(
                    "projection",
                    format!("expected {key}=<f32>, found {token:?}"),
                )
            })?;
        let value = parse_float(value, key)?;
        if value <= 0.0 {
            return Err(malformed("projection", format!("{key} must be positive")));
        }
        Ok(value)
    };
    let projection = Projection {
        hfov_degrees: field("hfov_degrees")?,
        aspect: field("aspect")?,
    };
    if let Some(extra) = tokens.next() {
        return Err(malformed("projection", format!("unexpected {extra:?}")));
    }
    Ok(projection)
}

/// `id51=<16 lowercase hex|none> walk_epoch=<u32> size=<u64> mtime=<u64>
/// prl=<path>`, the path running to the end of the line. `id51` and
/// `walk_epoch` must be in the form `bake_identity` writes, because the
/// engine compares them as raw strings.
fn parse_bake(rest: &str) -> Result<String, ParseReason> {
    let mut parts = rest.splitn(5, ' ');
    let mut value = |key: &'static str| {
        let token = parts.next().unwrap_or_default();
        token
            .strip_prefix(key)
            .and_then(|t| t.strip_prefix('='))
            .filter(|v| !v.is_empty())
            .ok_or_else(|| malformed("bake", format!("expected {key}=…, found {token:?}")))
    };
    let id51 = value("id51")?;
    let canonical_hash = id51.len() == ID51_HEX_LEN
        && id51
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if id51 != "none" && !canonical_hash {
        return Err(malformed(
            "bake",
            format!("id51 is {ID51_HEX_LEN} lowercase hex digits or `none`, not {id51:?}"),
        ));
    }
    let epoch = value("walk_epoch")?;
    parse_int::<u32>(epoch, "walk_epoch")?;
    if !epoch.bytes().all(|b| b.is_ascii_digit()) || (epoch.len() > 1 && epoch.starts_with('0')) {
        return Err(malformed(
            "bake",
            format!("walk_epoch is plain decimal with no sign or leading zeros, not {epoch:?}"),
        ));
    }
    parse_int::<u64>(value("size")?, "size")?;
    parse_int::<u64>(value("mtime")?, "mtime")?;
    value("prl")?;
    Ok(rest.to_string())
}

fn parse_vec3(rest: &str, line: &'static str) -> Result<Vec3, ParseReason> {
    let parts: Vec<&str> = rest.split(' ').collect();
    let [x, y, z] = parts[..] else {
        return Err(malformed(line, "expected three floats"));
    };
    Ok(Vec3::new(
        parse_float(x, "x")?,
        parse_float(y, "y")?,
        parse_float(z, "z")?,
    ))
}

fn parse_pose(rest: &str) -> Result<RoutePose, ParseReason> {
    let parts: Vec<&str> = rest.split(' ').collect();
    if parts.len() < 5 {
        return Err(malformed("pose", "expected x y z yaw pitch"));
    }
    let mut pose = RoutePose {
        eye: Vec3::new(
            parse_float(parts[0], "x")?,
            parse_float(parts[1], "y")?,
            parse_float(parts[2], "z")?,
        ),
        yaw: parse_float(parts[3], "yaw")?,
        pitch: parse_float(parts[4], "pitch")?,
        walk: None,
        cut: false,
    };
    let mut tail = &parts[5..];
    if let ["walk", step, cell, more @ ..] = tail {
        pose.walk = Some((parse_int(step, "step")?, parse_int(cell, "cell")?));
        tail = more;
    }
    if let ["cut", more @ ..] = tail {
        pose.cut = true;
        tail = more;
    }
    if let Some(extra) = tail.first() {
        return Err(malformed("pose", format!("unexpected {extra:?}")));
    }
    Ok(pose)
}
