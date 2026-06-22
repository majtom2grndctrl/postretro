use image::{imageops, ImageBuffer, Rgba};
use imgproc::read_manifest;
use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const CELL_W: u32 = 320;
const CELL_H: u32 = 192;
const COLUMNS: u32 = 2;
const DIFFUSE_BOX: (u32, u32) = (128, 128);
const SURFACE_BOX: (u32, u32) = (64, 64);
const REPEAT_BOX: (u32, u32) = (136, 72);

#[derive(Clone, Debug)]
struct ContactEntry {
    stem: String,
    tileable: Option<bool>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print_usage();
        return Ok(());
    }

    let command = ContactCommand::parse(args)?;
    let entries = if let Some(manifest) = &command.manifest {
        read_manifest(manifest)?
            .into_iter()
            .map(|job| ContactEntry {
                stem: job.stem,
                tileable: Some(job.tileable),
            })
            .collect()
    } else {
        scan_texture_dir(&command.dir)?
    };

    if entries.is_empty() {
        return Err(invalid_input(format!(
            "{} contains no complete diffuse/spec/normal texture bundles",
            command.dir.display()
        )));
    }

    write_contact_sheet(&command.dir, &entries, &command.out)?;
    println!("{}", command.out.display());
    Ok(())
}

struct ContactCommand {
    dir: PathBuf,
    out: PathBuf,
    manifest: Option<PathBuf>,
}

impl ContactCommand {
    fn parse(args: Vec<String>) -> Result<Self, Box<dyn Error>> {
        let mut parser = ArgParser::new(args);
        let dir = parser.required_path("--dir")?;
        let out = parser.required_path("--out")?;
        let manifest = parser.optional_path("--manifest")?;
        parser.finish()?;

        Ok(Self { dir, out, manifest })
    }
}

fn write_contact_sheet(
    dir: &Path,
    entries: &[ContactEntry],
    out: &Path,
) -> Result<(), Box<dyn Error>> {
    let rows = (entries.len() as u32).div_ceil(COLUMNS);
    let mut sheet =
        ImageBuffer::from_pixel(CELL_W * COLUMNS, CELL_H * rows, Rgba([238, 238, 232, 255]));

    for (idx, entry) in entries.iter().enumerate() {
        let diffuse = image::open(dir.join(format!("{}.png", entry.stem)))?.to_rgba8();
        let spec = image::open(dir.join(format!("{}_s.png", entry.stem)))?.to_rgba8();
        let normal = image::open(dir.join(format!("{}_n.png", entry.stem)))?.to_rgba8();
        let ox = (idx as u32 % COLUMNS) * CELL_W + 12;
        let oy = (idx as u32 / COLUMNS) * CELL_H + 12;

        paste_fit(&mut sheet, &diffuse, ox, oy, DIFFUSE_BOX.0, DIFFUSE_BOX.1);
        paste_fit(
            &mut sheet,
            &spec,
            ox + 140,
            oy,
            SURFACE_BOX.0,
            SURFACE_BOX.1,
        );
        paste_fit(
            &mut sheet,
            &normal,
            ox + 212,
            oy,
            SURFACE_BOX.0,
            SURFACE_BOX.1,
        );

        if entry.tileable == Some(true) {
            let mut repeat = ImageBuffer::from_pixel(
                diffuse.width() * 2,
                diffuse.height() * 2,
                Rgba([0, 0, 0, 255]),
            );
            paste(&mut repeat, &diffuse, 0, 0);
            paste(&mut repeat, &diffuse, diffuse.width(), 0);
            paste(&mut repeat, &diffuse, 0, diffuse.height());
            paste(&mut repeat, &diffuse, diffuse.width(), diffuse.height());
            paste_fit(
                &mut sheet,
                &repeat,
                ox + 140,
                oy + 76,
                REPEAT_BOX.0,
                REPEAT_BOX.1,
            );
        }
    }

    if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    sheet.save(out)?;
    Ok(())
}

fn scan_texture_dir(dir: &Path) -> Result<Vec<ContactEntry>, Box<dyn Error>> {
    let mut entries = Vec::new();
    scan_texture_dir_inner(dir, dir, &mut entries)?;
    entries.sort_by(|a, b| a.stem.cmp(&b.stem));
    Ok(entries)
}

fn scan_texture_dir_inner(
    root: &Path,
    dir: &Path,
    entries: &mut Vec<ContactEntry>,
) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            scan_texture_dir_inner(root, &path, entries)?;
            continue;
        }

        if !is_png(&path) || is_surface_map(&path) {
            continue;
        }

        let stem = path
            .strip_prefix(root)?
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/");
        if root.join(format!("{stem}_s.png")).is_file()
            && root.join(format!("{stem}_n.png")).is_file()
        {
            entries.push(ContactEntry {
                stem,
                tileable: None,
            });
        }
    }
    Ok(())
}

fn is_png(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
}

fn is_surface_map(path: &Path) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.ends_with("_s") || stem.ends_with("_n"))
}

fn paste(
    dst: &mut ImageBuffer<Rgba<u8>, Vec<u8>>,
    src: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    ox: u32,
    oy: u32,
) {
    for y in 0..src.height() {
        for x in 0..src.width() {
            if ox + x < dst.width() && oy + y < dst.height() {
                dst.put_pixel(ox + x, oy + y, *src.get_pixel(x, y));
            }
        }
    }
}

fn paste_fit(
    dst: &mut ImageBuffer<Rgba<u8>, Vec<u8>>,
    src: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    ox: u32,
    oy: u32,
    max_w: u32,
    max_h: u32,
) {
    let scaled = resize_to_fit(src, max_w, max_h);
    let centered_x = ox + (max_w - scaled.width()) / 2;
    let centered_y = oy + (max_h - scaled.height()) / 2;
    paste(dst, &scaled, centered_x, centered_y);
}

fn resize_to_fit(
    src: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    max_w: u32,
    max_h: u32,
) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    let (src_w, src_h) = src.dimensions();
    let fit_width = (src_w as u64) * (max_h as u64) > (src_h as u64) * (max_w as u64);
    let (target_w, target_h) = if fit_width {
        let target_h = ((src_h as u64) * (max_w as u64) / (src_w as u64)).max(1) as u32;
        (max_w, target_h)
    } else {
        let target_w = ((src_w as u64) * (max_h as u64) / (src_h as u64)).max(1) as u32;
        (target_w, max_h)
    };

    imageops::resize(src, target_w, target_h, imageops::FilterType::Nearest)
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
        self.optional_path(flag)?
            .ok_or_else(|| invalid_input(format!("missing required flag {flag}")))
    }

    fn optional_path(&mut self, flag: &str) -> Result<Option<PathBuf>, Box<dyn Error>> {
        self.optional_string(flag)
            .map(|value| value.map(PathBuf::from))
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

fn print_usage() {
    eprintln!("Usage:\n  contact --dir <texture-dir> --out <png> [--manifest <manifest>]");
}

fn invalid_input(message: impl Into<String>) -> Box<dyn Error> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}
