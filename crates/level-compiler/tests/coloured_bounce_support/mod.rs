// Shared real-CLI harness for the coloured-bounce suites: fixture roots, bakes and readers.
// See: context/lib/build_pipeline.md §Compiler pipeline

#![allow(dead_code)] // Each suite uses a different subset of the harness.

#[path = "../../src/coloured_bounce_test_fixtures.rs"]
pub mod fixtures;

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use postretro_level_format::texture_names::TextureNamesSection;
use postretro_level_format::{SectionId, read_container, read_section_data};

/// Flags every coloured-bounce bake shares: tiny lightmaps, a dense probe
/// grid, and uncompressed irradiance so SH values compare before encoding.
/// Callers add `--jobs`, the cache mode and anything scenario-specific.
pub const FIXTURE_FLAGS: [&str; 6] = [
    "--no-tui",
    "--lightmap-density",
    "0.25",
    "--sh-probe-spacing",
    "1",
    "--uncompressed-irradiance",
];

/// Fixed soft-shadow sampling for suites whose fixtures need a cheaper bake.
pub const FOUR_SHADOW_SAMPLES: [&str; 2] = ["--soft-shadow-samples", "4"];

/// Stage-start lines the plain reporter prints; their order is the stage order.
pub const SH_START: &str = "SH volume bake...";
pub const DELTA_START: &str = "Delta SH volume bake...";
pub const MATERIALS_START: &str = "World material bake...";
pub const TEXTURE_VALIDATION_START: &str = "Texture color-space validation...";
pub const TAIL_START: &str = "Texture mip bake...";

pub fn current_compiler() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_prl-build"))
}

/// SHA-256 of the saved pre-colour `prl-build`, the artifact the checked-in
/// oracle digests were recorded from at the indirect-scale commit.
pub const GREY_COMPILER_SHA256: &str =
    "098a6fc94c21ef020e065806b3a2e9600fccea3208c3c5240dccc6c72a332496";
pub const GREY_COMPILER_ENV: &str = "POSTRETRO_GREY_PRL_BUILD";

/// Resolve a saved pre-change compiler for a one-shot pre-change oracle.
///
/// The binary lives outside the repository, so the path comes only from
/// `var`. Unset means this oracle cannot run here: print why and return
/// `None` so the rest of `--ignored` stays green. A set path must hash to the
/// pinned artifact; anything else is a misconfiguration and fails.
pub fn pinned_old_compiler(var: &str, test: &str) -> Option<PathBuf> {
    let Some(path) = std::env::var_os(var) else {
        eprintln!(
            "SKIPPED {test}: one-shot pre-change oracle; set {var} to the saved \
             pre-colour prl-build (sha256 {GREY_COMPILER_SHA256}) to run it"
        );
        return None;
    };
    let path = PathBuf::from(path);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("{var}={}: unreadable: {error}", path.display()));
    assert_eq!(
        sha256_hex(&bytes),
        GREY_COMPILER_SHA256,
        "{var}={} is not the saved pre-colour compiler",
        path.display()
    );
    Some(path)
}

pub fn grey_compiler(test: &str) -> Option<PathBuf> {
    pinned_old_compiler(GREY_COMPILER_ENV, test)
}

/// FIPS 180-4 SHA-256. The pin predates this harness, and the workspace
/// carries no SHA-2 crate; blake3 cannot verify a SHA-256 digest.
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in message.as_chunks::<64>().0 {
        let mut schedule = [0_u32; 64];
        for (word, bytes) in schedule.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*bytes);
        }
        for i in 16..64 {
            let (w15, w2) = (schedule[i - 15], schedule[i - 2]);
            let s0 = w15.rotate_right(7) ^ w15.rotate_right(18) ^ (w15 >> 3);
            let s1 = w2.rotate_right(17) ^ w2.rotate_right(19) ^ (w2 >> 10);
            schedule[i] = schedule[i - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for (k, w) in K.iter().zip(schedule) {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(*k)
                .wrapping_add(w);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(majority);
            (h, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
        }
        for (word, add) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *word = word.wrapping_add(add);
        }
    }
    state.iter().map(|word| format!("{word:08x}")).collect()
}

/// A temporary content root: `maps/coloured-bounce.map`, `textures/`, and
/// per-label outputs beside them. Dropping it removes everything.
pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub map: PathBuf,
}

impl Fixture {
    pub fn new(map_text: &str, textured: bool) -> Self {
        let dir = tempfile::tempdir().expect("isolated fixture root");
        std::fs::create_dir(dir.path().join("maps")).unwrap();
        std::fs::create_dir(dir.path().join("textures")).unwrap();
        if textured {
            fixtures::write_textures(dir.path());
        }
        let map = dir.path().join("maps/coloured-bounce.map");
        std::fs::write(&map, map_text).unwrap();
        Self { dir, map }
    }

    /// The shared red-wall room with both lights and every fixture PNG.
    pub fn red_wall() -> Self {
        Self::new(
            &fixtures::room_map(fixtures::Appearance::RedWall, true, true),
            true,
        )
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn texture(&self, name: &str) -> PathBuf {
        self.root()
            .join("textures/bounce")
            .join(format!("{name}.png"))
    }

    /// Sidecar root for one label's bakes; repeated runs of a label reuse it.
    pub fn materials(&self, label: &str) -> PathBuf {
        self.root().join(format!("{label}-baked/materials"))
    }

    pub fn edit_map(&self, edit: impl FnOnce(String) -> String) {
        let text = std::fs::read_to_string(&self.map).unwrap();
        std::fs::write(&self.map, edit(text)).unwrap();
    }

    /// Run `compiler` on the map, writing `<label>.prl` and `<label>-baked/`.
    pub fn run(&self, compiler: &Path, label: &str, rust_log: &str, flags: &[&str]) -> Run {
        let prl = self.root().join(format!("{label}.prl"));
        let output = Command::new(compiler)
            .env("RUST_LOG", rust_log)
            .arg(&self.map)
            .arg("-o")
            .arg(&prl)
            .arg("--baked-root")
            .arg(self.root().join(format!("{label}-baked")))
            .args(FIXTURE_FLAGS)
            .args(flags)
            .output()
            .unwrap_or_else(|error| panic!("spawn {}: {error}", compiler.display()));
        Run {
            log: diagnostic(&output),
            output,
            prl,
            materials: self.materials(label),
        }
    }

    /// A run that must succeed, read back into sections and sidecars.
    pub fn bake(&self, compiler: &Path, label: &str, rust_log: &str, flags: &[&str]) -> Bake {
        self.run(compiler, label, rust_log, flags).into_bake()
    }
}

pub struct Run {
    pub output: Output,
    pub log: String,
    pub prl: PathBuf,
    pub materials: PathBuf,
}

impl Run {
    pub fn succeeded(&self) -> bool {
        self.output.status.success()
    }

    pub fn into_bake(self) -> Bake {
        assert!(self.succeeded(), "fixture compile failed:\n{}", self.log);
        Bake {
            sections: read_sections(&self.prl),
            sidecars: sidecars(&self.materials),
            log: self.log,
        }
    }
}

pub fn diagnostic(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Every section the PRL carries, keyed by id; an absent section stays absent.
pub fn read_sections(prl: &Path) -> BTreeMap<u32, Vec<u8>> {
    let mut cursor = Cursor::new(std::fs::read(prl).expect("compiled PRL"));
    let metadata = read_container(&mut cursor).unwrap();
    let ids: Vec<u32> = metadata
        .sections
        .iter()
        .map(|entry| entry.section_id)
        .collect();
    ids.into_iter()
        .filter_map(|id| {
            read_section_data(&mut cursor, &metadata, id)
                .unwrap_or_else(|error| panic!("read section {id}: {error}"))
                .map(|bytes| (id, bytes))
        })
        .collect()
}

/// Every `.prm` sidecar under `root`, by file name; names are content keys,
/// so a duplicate would mean two directories disagree about one bundle.
pub fn sidecars(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn collect(directory: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().is_some_and(|extension| extension == "prm") {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                let bytes = std::fs::read(&path).unwrap();
                assert!(out.insert(name, bytes).is_none(), "unique sidecar names");
            }
        }
    }
    let mut out = BTreeMap::new();
    if root.exists() {
        collect(root, &mut out);
    }
    out
}

pub struct Bake {
    pub log: String,
    pub sections: BTreeMap<u32, Vec<u8>>,
    pub sidecars: BTreeMap<String, Vec<u8>>,
}

impl Bake {
    pub fn section(&self, id: SectionId) -> &[u8] {
        self.sections
            .get(&(id as u32))
            .unwrap_or_else(|| panic!("required {id:?} section missing"))
    }

    /// Only the ids named, each required: a missing section is a fixture bug.
    pub fn required(&self, ids: &[u32]) -> BTreeMap<u32, Vec<u8>> {
        ids.iter()
            .map(|&id| {
                let bytes = self
                    .sections
                    .get(&id)
                    .unwrap_or_else(|| panic!("fixture must exercise section {id}"));
                (id, bytes.clone())
            })
            .collect()
    }

    pub fn names(&self) -> Vec<String> {
        TextureNamesSection::from_bytes(self.section(SectionId::TextureNames))
            .unwrap()
            .names
    }

    pub fn skipped(&self, stage: &str) -> bool {
        self.log
            .lines()
            .any(|line| line.trim() == format!("{stage}: skipped"))
    }

    /// The texture byte report: totals, per-slot, per-mip and first-name
    /// lines. Timings and paths elsewhere in the log do not participate.
    pub fn byte_report(&self) -> Vec<String> {
        byte_report(&self.log)
    }
}

pub fn byte_report(log: &str) -> Vec<String> {
    log.lines()
        .filter_map(|line| line.split_once("[prl-build] ").map(|(_, text)| text))
        .filter(|text| {
            ["texture bytes:", "  by slot:", "  by mip:", "  largest:"]
                .iter()
                .any(|prefix| text.starts_with(prefix))
        })
        .map(str::to_owned)
        .collect()
}

/// `[cache] <stage> hit|miss` tallies per stage. Delta entries log at debug,
/// so runs that read them set `prl_build::delta_sh_cache=debug`.
pub fn cache_tally(log: &str) -> BTreeMap<String, (usize, usize)> {
    let mut tally: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for line in log.lines() {
        let Some((_, rest)) = line.split_once("[cache] ") else {
            continue;
        };
        let mut words = rest.split_whitespace();
        let (Some(stage), Some(outcome)) = (words.next(), words.next()) else {
            continue;
        };
        let entry = tally.entry(stage.to_owned()).or_default();
        match outcome {
            "hit" => entry.0 += 1,
            "miss" => entry.1 += 1,
            _ => {}
        }
    }
    tally.retain(|_, (hits, misses)| *hits + *misses > 0);
    tally
}

pub fn write_png(path: &Path, texel: impl Fn(usize) -> [u8; 4]) {
    let pixels: Vec<u8> = (0..256).flat_map(texel).collect();
    let mut encoder = png::Encoder::new(std::fs::File::create(path).unwrap(), 16, 16);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&pixels)
        .unwrap();
}

/// Keep the IHDR and chunk layout valid, but break compressed IDAT bytes.
/// Header-only validation succeeds; the production texel decoder must fail.
pub fn corrupt_pixels(path: &Path) {
    let mut bytes = std::fs::read(path).unwrap();
    let mut offset = 8;
    loop {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        if &bytes[offset + 4..offset + 8] == b"IDAT" {
            assert!(length > 0);
            bytes[offset + 8] ^= 0xff;
            break;
        }
        offset += length + 12;
        assert!(offset < bytes.len(), "fixture PNG has IDAT");
    }
    std::fs::write(path, bytes).unwrap();
}
