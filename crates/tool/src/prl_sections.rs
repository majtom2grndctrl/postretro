//! `prl-sections`: a compiled level's section sizes, largest first, so a
//! level that is too big on disk shows which data makes it so.
//!
//! See: context/lib/build_pipeline.md §PRL section IDs

use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use postretro_level_format::{SectionId, read_container, validate_container_bounds};

pub(crate) fn run(args: Vec<OsString>) -> Result<i32, String> {
    let path = parse_args(args)?;
    let report = read_report(&path)?;
    print!("{}", report.render(&path));
    Ok(0)
}

fn parse_args(args: Vec<OsString>) -> Result<PathBuf, String> {
    let mut path = None;
    for arg in args {
        if arg.to_str().is_some_and(|arg| arg.starts_with('-')) {
            return Err(usage(&format!(
                "unknown prl-sections option {:?}",
                arg.to_string_lossy()
            )));
        }
        if path.replace(PathBuf::from(arg)).is_some() {
            return Err(usage("prl-sections accepts exactly one .prl path"));
        }
    }
    path.ok_or_else(|| usage("prl-sections requires a .prl path"))
}

fn usage(error: &str) -> String {
    format!("{error}\n\nUsage: postretro-tool prl-sections <level.prl>")
}

/// One entry of the PRL section table.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SectionRow {
    id: u32,
    /// The registry's name, or `None` for an id this build does not know.
    name: Option<String>,
    /// The section table's version field. A section's own payload epoch,
    /// where it has one, sits inside its bytes and is not read here.
    version: u16,
    bytes: u64,
}

#[derive(Debug)]
struct SectionReport {
    file_bytes: u64,
    /// Largest first; ties by id.
    rows: Vec<SectionRow>,
}

/// Read only the header and section table: a multi-gigabyte level reports
/// without loading any payload.
fn read_report(path: &Path) -> Result<SectionReport, String> {
    let file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let file_bytes = file
        .metadata()
        .map_err(|error| format!("read {}: {error}", path.display()))?
        .len();
    let meta = read_container(&mut BufReader::new(file))
        .map_err(|error| format!("{} is not a readable PRL: {error}", path.display()))?;
    validate_container_bounds(&meta, file_bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let mut rows: Vec<SectionRow> = meta
        .sections
        .iter()
        .map(|entry| SectionRow {
            id: entry.section_id,
            name: SectionId::from_u32(entry.section_id).map(|id| format!("{id:?}")),
            version: entry.version,
            bytes: entry.size,
        })
        .collect();
    rows.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.id.cmp(&b.id)));
    Ok(SectionReport { file_bytes, rows })
}

impl SectionReport {
    fn render(&self, path: &Path) -> String {
        let share = |bytes: u64| {
            if self.file_bytes == 0 {
                0.0
            } else {
                bytes as f64 * 100.0 / self.file_bytes as f64
            }
        };
        let name_width = self
            .rows
            .iter()
            .filter_map(|row| row.name.as_deref().map(str::len))
            .chain(["(header and section table)".len()])
            .max()
            .unwrap_or(0);
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{}: {} bytes ({}), {} sections",
            path.display(),
            grouped(self.file_bytes),
            human(self.file_bytes),
            self.rows.len()
        );
        let _ = writeln!(
            out,
            "{:>4}  {:<name_width$} {:>9} {:>17} {:>10} {:>8}",
            "id", "section", "table ver", "bytes", "size", "share"
        );
        for row in &self.rows {
            let _ = writeln!(
                out,
                "{:>4}  {:<name_width$} {:>9} {:>17} {:>10} {:>7.2}%",
                row.id,
                row.name.as_deref().unwrap_or("(unknown id)"),
                row.version,
                grouped(row.bytes),
                human(row.bytes),
                share(row.bytes)
            );
        }
        let sections: u64 = self.rows.iter().map(|row| row.bytes).sum();
        let overhead = self.file_bytes.saturating_sub(sections);
        let _ = writeln!(
            out,
            "{:>4}  {:<name_width$} {:>9} {:>17} {:>10} {:>7.2}%",
            "",
            "(header and section table)",
            "",
            grouped(overhead),
            human(overhead),
            share(overhead)
        );
        let _ = writeln!(
            out,
            "{:>4}  {:<name_width$} {:>9} {:>17} {:>10} {:>7.2}%",
            "",
            "total",
            "",
            grouped(self.file_bytes),
            human(self.file_bytes),
            share(self.file_bytes)
        );
        out
    }
}

/// `1234567` as `1,234,567`.
fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Binary units, one decimal place.
fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = "B";
    for next in UNITS {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next;
    }
    format!("{value:.1} {unit}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::{SectionBlob, write_prl};

    fn write_level(dir: &Path) -> PathBuf {
        let path = dir.join("level.prl");
        let sections = [
            (SectionId::Geometry as u32, 2, 300),
            (SectionId::Lightmap as u32, 3, 5_000),
            (9_999, 1, 300),
        ]
        .map(|(section_id, version, len)| SectionBlob {
            section_id,
            version,
            data: vec![0xAB; len],
        });
        let mut bytes = Vec::new();
        write_prl(&mut bytes, &sections).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn reports_each_section_largest_first_with_its_share_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_level(dir.path());
        let report = read_report(&path).unwrap();
        let file_bytes = std::fs::metadata(&path).unwrap().len();
        assert_eq!(report.file_bytes, file_bytes);
        let rows: Vec<(u32, Option<&str>, u16, u64)> = report
            .rows
            .iter()
            .map(|row| (row.id, row.name.as_deref(), row.version, row.bytes))
            .collect();
        assert_eq!(
            rows,
            [
                (22, Some("Lightmap"), 3, 5_000),
                (17, Some("Geometry"), 2, 300),
                (9_999, None, 1, 300),
            ]
        );
        let text = report.render(&path);
        let lightmap = text
            .lines()
            .find(|line| line.contains("Lightmap"))
            .expect("a lightmap row");
        let share = format!("{:.2}%", 5_000.0 * 100.0 / file_bytes as f64);
        assert!(
            lightmap.contains("5,000") && lightmap.contains(&share),
            "{text}"
        );
        assert!(text.contains("(unknown id)"), "{text}");
        let overhead = file_bytes - 5_600;
        assert!(
            text.lines()
                .any(|line| line.contains("(header and section table)")
                    && line.contains(&format!(" {overhead} B"))),
            "{text}"
        );
        assert!(text.lines().last().unwrap().contains("100.00%"), "{text}");
    }

    #[test]
    fn rejects_a_file_that_is_not_a_level_and_a_bad_command_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.prl");
        std::fs::write(&path, b"not a level").unwrap();
        assert!(
            read_report(&path)
                .unwrap_err()
                .contains("not a readable PRL")
        );
        // A section table that points past the end of the file.
        let level = write_level(dir.path());
        let bytes = std::fs::read(&level).unwrap();
        std::fs::write(&level, &bytes[..bytes.len() - 1]).unwrap();
        assert!(read_report(&level).is_err());

        assert!(parse_args(Vec::new()).is_err());
        assert!(parse_args(vec!["a.prl".into(), "b.prl".into()]).is_err());
        assert!(parse_args(vec!["--json".into()]).is_err());
        assert_eq!(
            parse_args(vec!["a.prl".into()]).unwrap(),
            PathBuf::from("a.prl")
        );
    }

    #[test]
    fn sizes_group_digits_and_scale_units() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1536), "1.5 KiB");
        assert_eq!(human(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }
}
