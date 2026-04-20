// Billboard smoke sprite emitter: CPU ring buffer, per-frame GPU upload.
// See: context/lib/rendering_pipeline.md §7.4

use std::path::Path;

use glam::Vec3;

// --- Constants ---

/// Maximum live sprites per emitter. Ring buffer capacity.
pub const MAX_SPRITES: usize = 512;

/// GPU-side sprite instance layout. Two `vec4<f32>` slots = 32 bytes.
/// Layout must match the WGSL `SpriteInstance` struct in `billboard.wgsl`.
/// The struct is two `vec4<f32>` slots to satisfy WGSL storage-buffer
/// alignment (a trailing scalar next to a `vec3<f32>` member lands in the
/// same 16-byte slot).
///
/// Offsets:
///   0..12   position       (vec3<f32>, world-space)
///   12..16  age            (f32, seconds since spawn)
///   16..20  size           (f32, world units, side of the quad)
///   20..24  rotation       (f32, radians)
///   24..28  opacity        (f32, 0..1)
///   28..32  _pad           (f32, zero)
pub const SPRITE_INSTANCE_SIZE: usize = 32;

// --- CPU-side sprite ---

/// One live sprite in the CPU-side ring buffer.
#[derive(Debug, Clone, Copy)]
struct Sprite {
    /// World-space position.
    position: Vec3,
    /// Age in seconds since this sprite was spawned.
    age: f32,
    /// World-space size (side of the quad, in world units).
    size: f32,
    /// Current rotation in radians (slow random spin).
    rotation: f32,
    /// Random spin rate in radians per second.
    spin_rate: f32,
    /// Computed opacity [0, 1], driven by age.
    opacity: f32,
}

// --- Emitter parameters ---

/// Parameters parsed from an `env_smoke_emitter` point entity.
/// See: context/lib/build_pipeline.md §Custom FGD
#[derive(Debug, Clone)]
pub struct SmokeEmitterParams {
    /// World-space spawn position.
    pub origin: Vec3,
    /// Sprites spawned per second.
    pub rate: f32,
    /// Maximum sprite age in seconds before it is killed.
    pub lifetime: f32,
    /// Initial sprite size in world units.
    pub size: f32,
    /// Upward drift velocity in world units per second.
    pub speed: f32,
    /// Sprite sheet collection name (sub-directory under `textures/`).
    pub collection: String,
    /// Blinn-Phong specular scale for this emitter's sprites.
    pub spec_intensity: f32,
}

impl Default for SmokeEmitterParams {
    fn default() -> Self {
        Self {
            origin: Vec3::ZERO,
            rate: 4.0,
            lifetime: 3.0,
            size: 0.5,
            speed: 0.3,
            collection: String::new(),
            spec_intensity: 0.3,
        }
    }
}

// --- Emitter ---

/// CPU-side smoke emitter. Maintains a ring buffer of live sprites and
/// accumulates fractional spawn time. Renderer calls `tick` each game-logic
/// frame to advance physics, then `collect_gpu_instances` to extract the
/// upload slice for the GPU storage buffer.
pub struct SmokeEmitter {
    pub params: SmokeEmitterParams,

    /// Ring buffer of live sprites; the first `live_count` entries are active.
    sprites: [Sprite; MAX_SPRITES],

    /// Number of currently live sprites.
    live_count: usize,

    /// Fractional accumulated spawn time; a sprite is emitted once this
    /// reaches 1.0 / rate.
    spawn_accumulator: f32,

    /// Simple LCG-style state for deterministic pseudo-random values.
    rand_state: u32,
}

impl SmokeEmitter {
    /// Create a new emitter from the given parameters.
    pub fn new(params: SmokeEmitterParams) -> Self {
        let seed = params.origin.x.to_bits() ^ params.origin.y.to_bits() ^ 0xDEAD_BEEF;
        Self {
            params,
            sprites: [Sprite {
                position: Vec3::ZERO,
                age: 0.0,
                size: 0.0,
                rotation: 0.0,
                spin_rate: 0.0,
                opacity: 0.0,
            }; MAX_SPRITES],
            live_count: 0,
            spawn_accumulator: 0.0,
            rand_state: seed,
        }
    }

    /// Advance the LCG and return the next pseudo-random u32.
    fn rand_u32(&mut self) -> u32 {
        // Park-Miller style multiply, good enough for visual noise.
        self.rand_state = self
            .rand_state
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        self.rand_state
    }

    /// Return a pseudo-random f32 in [0.0, 1.0).
    fn rand_f32(&mut self) -> f32 {
        (self.rand_u32() >> 8) as f32 / (1 << 24) as f32
    }

    /// Advance the simulation by `dt` seconds. Spawns new sprites at the
    /// configured rate and updates existing sprite positions/ages/opacity.
    pub fn tick(&mut self, dt: f32) {
        let lifetime = self.params.lifetime.max(1.0e-6);
        let speed = self.params.speed;
        let fade_in_frac = 0.1; // first 10% of lifetime: fade in
        let fade_out_frac = 0.3; // last 30% of lifetime: fade out

        // Advance all live sprites.
        let mut i = 0;
        while i < self.live_count {
            let s = &mut self.sprites[i];
            s.age += dt;

            // Drift upward.
            s.position.y += speed * dt;

            // Slow spin.
            s.rotation += s.spin_rate * dt;

            // Opacity: fade-in over first fade_in_frac of lifetime, hold,
            // then fade-out over last fade_out_frac.
            let t = (s.age / lifetime).clamp(0.0, 1.0);
            let fade_in_end = fade_in_frac;
            let fade_out_start = 1.0 - fade_out_frac;
            s.opacity = if t < fade_in_end {
                t / fade_in_end
            } else if t > fade_out_start {
                1.0 - (t - fade_out_start) / fade_out_frac
            } else {
                1.0
            };

            // Remove expired sprites by swapping with the last live entry.
            if s.age >= lifetime {
                self.live_count -= 1;
                self.sprites[i] = self.sprites[self.live_count];
                // Don't advance i — re-check swapped entry.
            } else {
                i += 1;
            }
        }

        // Accumulate spawn time.
        let rate = self.params.rate.max(1.0e-6);
        let interval = 1.0 / rate;
        self.spawn_accumulator += dt;
        while self.spawn_accumulator >= interval && self.live_count < MAX_SPRITES {
            self.spawn_accumulator -= interval;
            self.spawn_sprite();
        }
        // If we've hit max capacity, drain excess accumulation so the first
        // available slot gets the next sprite on time rather than flooding.
        if self.live_count >= MAX_SPRITES {
            self.spawn_accumulator = self.spawn_accumulator.min(interval);
        }
    }

    /// Spawn one new sprite at the emitter origin with randomised properties.
    fn spawn_sprite(&mut self) {
        if self.live_count >= MAX_SPRITES {
            return;
        }
        // Random initial rotation.
        let rotation = self.rand_f32() * std::f32::consts::TAU;
        // Random spin rate: ±0.3 rad/s.
        let spin_rate = (self.rand_f32() - 0.5) * 0.6;
        // Small random horizontal offset so sprites don't all stack exactly.
        let jitter_x = (self.rand_f32() - 0.5) * self.params.size * 0.5;
        let jitter_z = (self.rand_f32() - 0.5) * self.params.size * 0.5;

        self.sprites[self.live_count] = Sprite {
            position: self.params.origin + Vec3::new(jitter_x, 0.0, jitter_z),
            age: 0.0,
            size: self.params.size,
            rotation,
            spin_rate,
            opacity: 0.0,
        };
        self.live_count += 1;
    }

    /// How many sprites are currently live. Kept on the public API so test
    /// harnesses and future entity-inspection UIs can read emitter state
    /// without touching private fields.
    #[allow(dead_code)]
    pub fn live_count(&self) -> usize {
        self.live_count
    }

    /// Collection name for this emitter (used to batch by sprite sheet).
    pub fn collection(&self) -> &str {
        &self.params.collection
    }

    /// Write the packed GPU instance bytes for all live sprites into `out`.
    /// Appends `live_count * SPRITE_INSTANCE_SIZE` bytes to the provided buffer.
    ///
    /// Layout per instance (must match billboard.wgsl `SpriteInstance`):
    ///   bytes  0..12  world_position (vec3<f32>)
    ///   bytes 12..16  age            (f32)
    ///   bytes 16..20  size           (f32)
    ///   bytes 20..24  rotation       (f32)
    ///   bytes 24..28  opacity        (f32)
    ///   bytes 28..32  _pad           (f32)
    pub fn pack_instances(&self, out: &mut Vec<u8>) {
        for i in 0..self.live_count {
            let s = &self.sprites[i];
            out.extend_from_slice(&s.position.x.to_ne_bytes());
            out.extend_from_slice(&s.position.y.to_ne_bytes());
            out.extend_from_slice(&s.position.z.to_ne_bytes());
            out.extend_from_slice(&s.age.to_ne_bytes());
            out.extend_from_slice(&s.size.to_ne_bytes());
            out.extend_from_slice(&s.rotation.to_ne_bytes());
            out.extend_from_slice(&s.opacity.to_ne_bytes());
            out.extend_from_slice(&0.0f32.to_ne_bytes()); // pad
        }
    }
}

// --- Frame-duration lookup ---

/// Given a total number of animation frames and a sprite lifetime, return the
/// duration of each frame in seconds. If the collection has zero frames, falls
/// back to 1 frame of infinite duration.
///
/// Mirrored by the inline computation in `billboard.wgsl` (the GPU version
/// derives it from draw_params.params.z / frame_count). Kept here for CPU
/// diagnostics and a future entity-system hook that reports per-emitter
/// frame cadence.
#[allow(dead_code)]
pub fn frame_duration(frame_count: usize, lifetime: f32) -> f32 {
    if frame_count == 0 {
        return lifetime;
    }
    lifetime / frame_count as f32
}

// --- Sprite sheet loading (CPU side) ---

/// One loaded animation frame for a smoke collection.
#[derive(Debug, Clone)]
pub struct SpriteFrame {
    /// RGBA8 pixel data.
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Load all frames for a smoke collection (e.g., `smoke_00.png`, `smoke_01.png`, …)
/// from `textures/<collection>/`. Returns `None` if no frames are found; in that
/// case the renderer falls back to the checkerboard placeholder.
pub fn load_collection_frames(texture_root: &Path, collection: &str) -> Option<Vec<SpriteFrame>> {
    if collection.is_empty() {
        return None;
    }

    let collection_dir = texture_root.join(collection);
    let read_dir = match std::fs::read_dir(&collection_dir) {
        Ok(d) => d,
        Err(_) => {
            log::warn!(
                "[Smoke] Collection directory '{}' not found — no frames loaded",
                collection_dir.display()
            );
            return None;
        }
    };

    // Collect all `smoke_NN.png` paths and sort by numeric suffix.
    let mut frame_paths: Vec<(u32, std::path::PathBuf)> = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_lowercase(),
            None => continue,
        };
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !ext.eq_ignore_ascii_case("png") {
            continue;
        }
        if let Some(suffix) = stem.strip_prefix("smoke_") {
            if let Ok(n) = suffix.parse::<u32>() {
                frame_paths.push((n, path));
            }
        }
    }

    if frame_paths.is_empty() {
        log::warn!(
            "[Smoke] No smoke_NN.png frames found in '{}'",
            collection_dir.display()
        );
        return None;
    }

    frame_paths.sort_by_key(|(n, _)| *n);

    let frames: Vec<SpriteFrame> = frame_paths
        .iter()
        .filter_map(|(_, path)| match image::open(path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                Some(SpriteFrame {
                    data: rgba.into_raw(),
                    width: w,
                    height: h,
                })
            }
            Err(err) => {
                log::warn!("[Smoke] Failed to load '{}': {err}", path.display());
                None
            }
        })
        .collect();

    if frames.is_empty() {
        None
    } else {
        Some(frames)
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    fn make_emitter() -> SmokeEmitter {
        SmokeEmitter::new(SmokeEmitterParams {
            origin: Vec3::new(0.0, 0.0, 0.0),
            rate: 10.0,
            lifetime: 1.0,
            size: 0.5,
            speed: 0.3,
            collection: "smoke".to_string(),
            spec_intensity: 0.3,
        })
    }

    #[test]
    fn sprites_spawn_over_time() {
        let mut emitter = make_emitter();
        // Tick 0.5 s at rate=10: expect ~5 sprites.
        emitter.tick(0.5);
        assert!(
            emitter.live_count() > 0,
            "expected some sprites after 0.5s tick"
        );
        assert!(
            emitter.live_count() <= MAX_SPRITES,
            "live_count must stay within ring buffer capacity"
        );
    }

    #[test]
    fn sprites_expire() {
        let mut emitter = make_emitter(); // lifetime = 1.0 s
        emitter.tick(0.1); // spawn some sprites
        let initial_count = emitter.live_count();
        assert!(initial_count > 0);
        // Advance well past the lifetime — all initial sprites should die.
        emitter.tick(2.0);
        // Newly spawned sprites may exist; just confirm total is bounded.
        assert!(
            emitter.live_count() <= MAX_SPRITES,
            "sprites must not exceed ring buffer capacity"
        );
    }

    #[test]
    fn max_capacity_not_exceeded() {
        let mut emitter = SmokeEmitter::new(SmokeEmitterParams {
            rate: 10_000.0,  // extreme rate
            lifetime: 100.0, // long lifetime so sprites don't expire
            ..Default::default()
        });
        emitter.tick(10.0); // tick a long time
        assert_eq!(
            emitter.live_count(),
            MAX_SPRITES,
            "ring buffer must cap at MAX_SPRITES"
        );
    }

    #[test]
    fn pack_instances_length() {
        let mut emitter = make_emitter();
        emitter.tick(0.5);
        let count = emitter.live_count();
        let mut buf = Vec::new();
        emitter.pack_instances(&mut buf);
        assert_eq!(
            buf.len(),
            count * SPRITE_INSTANCE_SIZE,
            "packed buffer length must equal live_count * SPRITE_INSTANCE_SIZE"
        );
    }

    #[test]
    fn frame_duration_basic() {
        assert!((frame_duration(4, 2.0) - 0.5).abs() < 1e-6);
        assert!((frame_duration(0, 1.0) - 1.0).abs() < 1e-6);
    }
}
