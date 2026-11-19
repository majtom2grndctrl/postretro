// Install-cost model for the drain measurement helpers: warm and cold drains
// told apart by staging-buffer creation, per-shape percentiles, and a
// least-squares fit of drain CPU time to items and MiB. Test-only; prints.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency), §12

use crate::render::uploads::{StagingPool, UploadQueue};

pub(crate) const MIB: f64 = 1024.0 * 1024.0;

/// Buffers created so far by `pools` and by `queue`'s frame pool: every
/// staging pool a drain, and the compose after it, can draw from.
pub(crate) fn staging_buffers_created(pools: &[&StagingPool], queue: &UploadQueue) -> u64 {
    pools.iter().map(|pool| pool.counts().1).sum::<u64>() + queue.pool_counts().1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrainClass {
    /// No staging pool created a buffer, and no pool work ran.
    Warm,
    /// A staging pool created `created` buffers during the drain: the cold
    /// rate, whatever else it did.
    Cold { created: u64 },
    /// Work the install model does not price, such as pool growth or repack.
    Excluded(&'static str),
}

impl DrainClass {
    /// Cold wins over every other verdict: a drain that created a staging
    /// buffer never counts warm, wherever it falls in the run.
    pub(crate) fn of(
        created_before: u64,
        created_after: u64,
        excluded: Option<&'static str>,
    ) -> Self {
        let created = created_after.saturating_sub(created_before);
        if created > 0 {
            Self::Cold { created }
        } else if let Some(reason) = excluded {
            Self::Excluded(reason)
        } else {
            Self::Warm
        }
    }
}

/// One drain shape: `items` installs of `bytes` decoded bytes in all.
#[derive(Debug, Clone)]
pub(crate) struct Shape {
    pub(crate) label: String,
    pub(crate) items: u32,
    pub(crate) bytes: u64,
    /// The drain-size bucket it represents, in MiB.
    pub(crate) nominal_mib: u32,
}

impl Shape {
    pub(crate) fn mib(&self) -> f64 {
        self.bytes as f64 / MIB
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DrainSample {
    pub(crate) shape: usize,
    pub(crate) micros: u64,
    pub(crate) class: DrainClass,
}

/// CPU µs = per drain + per item × items + per MiB × MiB.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LinearFit {
    pub(crate) per_drain_us: f64,
    pub(crate) per_item_us: f64,
    pub(crate) per_mib_us: f64,
    pub(crate) rms_us: f64,
}

impl LinearFit {
    pub(crate) fn predict_us(&self, items: f64, mib: f64) -> f64 {
        self.per_drain_us + self.per_item_us * items + self.per_mib_us * mib
    }
}

/// Least squares over `(items, mib, micros)` points. With `intercept`
/// false the per-drain term is pinned at zero. `None` when the points do not
/// separate the terms.
pub(crate) fn fit(points: &[(f64, f64, f64)], intercept: bool) -> Option<LinearFit> {
    let columns = |&(items, mib, _): &(f64, f64, f64)| -> Vec<f64> {
        if intercept {
            vec![1.0, items, mib]
        } else {
            vec![items, mib]
        }
    };
    let n = if intercept { 3 } else { 2 };
    if points.len() < n {
        return None;
    }
    // Normal equations, solved by Gaussian elimination with partial pivoting.
    let mut a = vec![vec![0.0f64; n + 1]; n];
    for point in points {
        let x = columns(point);
        for row in 0..n {
            for col in 0..n {
                a[row][col] += x[row] * x[col];
            }
            a[row][n] += x[row] * point.2;
        }
    }
    // Pivots are judged against the matrix scale: the entries reach ~1e7, so a
    // fixed absolute floor would let a near-collinear set through as a noisy solve.
    let scale = (0..n).map(|i| a[i][i].abs()).fold(0.0f64, f64::max);
    for pivot in 0..n {
        let best = (pivot..n).max_by(|&i, &j| a[i][pivot].abs().total_cmp(&a[j][pivot].abs()))?;
        a.swap(pivot, best);
        if a[pivot][pivot].abs() <= 1e-12 * scale {
            return None;
        }
        let pivot_row = a[pivot].clone();
        for (index, row) in a.iter_mut().enumerate() {
            if index != pivot {
                let factor = row[pivot] / pivot_row[pivot];
                for (cell, &above) in row[pivot..].iter_mut().zip(&pivot_row[pivot..]) {
                    *cell -= factor * above;
                }
            }
        }
    }
    let solution: Vec<f64> = (0..n).map(|i| a[i][n] / a[i][i]).collect();
    let (per_drain_us, per_item_us, per_mib_us) = if intercept {
        (solution[0], solution[1], solution[2])
    } else {
        (0.0, solution[0], solution[1])
    };
    let mut model = LinearFit {
        per_drain_us,
        per_item_us,
        per_mib_us,
        rms_us: 0.0,
    };
    let squares: f64 = points
        .iter()
        .map(|&(items, mib, micros)| (model.predict_us(items, mib) - micros).powi(2))
        .sum();
    model.rms_us = (squares / points.len() as f64).sqrt();
    Some(model)
}

/// Nearest-rank percentile of an ascending slice.
fn percentile(sorted: &[u64], q: f64) -> u64 {
    sorted[((sorted.len() as f64 * q).ceil() as usize).clamp(1, sorted.len()) - 1]
}

fn ms(micros: impl Into<f64>) -> f64 {
    micros.into() / 1000.0
}

/// A drain shape priced from the fit.
pub(crate) struct Price {
    pub(crate) label: &'static str,
    pub(crate) items: f64,
    pub(crate) mib: f64,
}

/// One measured series: its shapes and every drain, in run order.
pub(crate) struct CostRun {
    pub(crate) title: String,
    pub(crate) shapes: Vec<Shape>,
    pub(crate) samples: Vec<DrainSample>,
}

impl CostRun {
    pub(crate) fn new(title: impl Into<String>, shapes: Vec<Shape>) -> Self {
        Self {
            title: title.into(),
            shapes,
            samples: Vec::new(),
        }
    }

    pub(crate) fn record(&mut self, shape: usize, micros: u64, class: DrainClass) {
        self.samples.push(DrainSample {
            shape,
            micros,
            class,
        });
    }

    fn warm_sorted(&self, keep: impl Fn(&Shape) -> bool) -> Vec<u64> {
        let mut warm: Vec<u64> = self
            .samples
            .iter()
            .filter(|s| s.class == DrainClass::Warm && keep(&self.shapes[s.shape]))
            .map(|s| s.micros)
            .collect();
        warm.sort_unstable();
        warm
    }

    /// The fit over each shape's warm median: one point per shape, so a
    /// shape drained more often weighs no more and a jittered drain moves
    /// nothing.
    pub(crate) fn median_fit(&self, intercept: bool) -> Option<LinearFit> {
        let points: Vec<(f64, f64, f64)> = (0..self.shapes.len())
            .filter_map(|index| {
                let warm = self.warm_sorted(|shape| std::ptr::eq(shape, &self.shapes[index]));
                (!warm.is_empty()).then(|| {
                    let shape = &self.shapes[index];
                    (
                        f64::from(shape.items),
                        shape.mib(),
                        percentile(&warm, 0.5) as f64,
                    )
                })
            })
            .collect();
        fit(&points, intercept)
    }

    /// Print the shape table, the size buckets, both fits, the cold and the
    /// excluded drains, then `prices` at the fitted rates.
    pub(crate) fn report(&self, context: &str, prices: &[Price]) {
        let full = self.median_fit(true);
        let through_origin = self.median_fit(false);
        println!("\n== {} ==", self.title);
        println!("{context}");
        println!(
            "drains {}: warm {}, cold {}, excluded {}",
            self.samples.len(),
            self.samples
                .iter()
                .filter(|s| s.class == DrainClass::Warm)
                .count(),
            self.samples
                .iter()
                .filter(|s| matches!(s.class, DrainClass::Cold { .. }))
                .count(),
            self.samples
                .iter()
                .filter(|s| matches!(s.class, DrainClass::Excluded(_)))
                .count(),
        );
        // Per-shape samples are few (rounds per shape), and nearest-rank p95 is
        // the max below 20 samples, so the shape table carries p50 and max only.
        println!(
            "{:<24} {:>6} {:>8} {:>5} {:>8} {:>8} {:>9} {:>9}",
            "shape", "items", "MiB", "n", "p50 ms", "max ms", "fit ms", "fit0 ms"
        );
        for shape in &self.shapes {
            let warm = self.warm_sorted(|s| std::ptr::eq(s, shape));
            let (items, mib) = (f64::from(shape.items), shape.mib());
            let fitted = full.map_or(f64::NAN, |f| ms(f.predict_us(items, mib)));
            let fitted0 = through_origin.map_or(f64::NAN, |f| ms(f.predict_us(items, mib)));
            if warm.is_empty() {
                println!(
                    "{:<24} {:>6} {:>8.2} {:>5} {:>8} {:>8} {:>9.2} {:>9.2}",
                    shape.label, shape.items, mib, 0, "-", "-", fitted, fitted0
                );
                continue;
            }
            println!(
                "{:<24} {:>6} {:>8.2} {:>5} {:>8.2} {:>8.2} {:>9.2} {:>9.2}",
                shape.label,
                shape.items,
                mib,
                warm.len(),
                ms(percentile(&warm, 0.5) as f64),
                ms(percentile(&warm, 1.0) as f64),
                fitted,
                fitted0
            );
        }
        println!("fit ms: with per-drain term; fit0 ms: per item + per MiB only");
        let mut buckets: Vec<u32> = self.shapes.iter().map(|s| s.nominal_mib).collect();
        buckets.sort_unstable();
        buckets.dedup();
        println!("by drain size (warm drains of every shape in the bucket):");
        for bucket in buckets {
            let warm = self.warm_sorted(|s| s.nominal_mib == bucket);
            if warm.is_empty() {
                continue;
            }
            println!(
                "  ~{bucket:>2} MiB: n {:>4}, p50 {:.2} ms, p95 {:.2} ms (n {}), max {:.2} ms",
                warm.len(),
                ms(percentile(&warm, 0.5) as f64),
                ms(percentile(&warm, 0.95) as f64),
                warm.len(),
                ms(percentile(&warm, 1.0) as f64),
            );
        }
        match full {
            Some(f) => println!(
                "fit (warm per-shape medians): {:.1} µs/drain + {:.2} µs/item + {:.1} µs/MiB \
                 ({:.3} ms/MiB); rms residual {:.1} µs",
                f.per_drain_us,
                f.per_item_us,
                f.per_mib_us,
                f.per_mib_us / 1000.0,
                f.rms_us
            ),
            None => println!("fit: the warm shapes do not separate the terms"),
        }
        if let Some(f) = through_origin {
            println!(
                "fit, no per-drain term: {:.2} µs/item + {:.1} µs/MiB ({:.3} ms/MiB); rms {:.1} µs",
                f.per_item_us,
                f.per_mib_us,
                f.per_mib_us / 1000.0,
                f.rms_us
            );
        }
        println!("cold drains (a staging pool created a buffer), run order:");
        for (at, sample) in self.samples.iter().enumerate() {
            if let DrainClass::Cold { created } = sample.class {
                let shape = &self.shapes[sample.shape];
                println!(
                    "  #{at:<4} {:<24} {:>8.2} MiB {:>8.2} ms = {:.3} ms/MiB, {created} buffer(s) created",
                    shape.label,
                    shape.mib(),
                    ms(sample.micros as f64),
                    ms(sample.micros as f64) / shape.mib(),
                );
            }
        }
        for (at, sample) in self.samples.iter().enumerate() {
            if let DrainClass::Excluded(reason) = sample.class {
                let shape = &self.shapes[sample.shape];
                println!(
                    "  excluded #{at:<4} {:<24} {:>8.2} ms: {reason}",
                    shape.label,
                    ms(sample.micros as f64),
                );
            }
        }
        if full.is_some() || through_origin.is_some() {
            println!("priced at the fit (with per-drain term | per item + per MiB only):");
            let priced = |f: Option<LinearFit>, price: &Price| {
                f.map_or("n/a".to_string(), |f| {
                    format!("{:.2} ms", ms(f.predict_us(price.items, price.mib)))
                })
            };
            for price in prices {
                println!(
                    "  {:<34} {:>4} items {:>6.1} MiB -> {} | {}",
                    price.label,
                    price.items,
                    price.mib,
                    priced(full, price),
                    priced(through_origin, price)
                );
            }
        }
    }
}

/// The build, adapter and fixture line every report opens with.
pub(crate) fn run_context(device: &wgpu::Device, fixture: &str) -> String {
    let info = device.adapter_info();
    let profile = if cfg!(debug_assertions) {
        "debug (debug assertions on)"
    } else {
        "release"
    };
    format!(
        "build: {profile}; adapter: {} ({:?}); fixture: {fixture}",
        info.name, info.backend
    )
}

/// The showcase's real drain shapes at the caps the sweep measured.
pub(crate) const SHOWCASE_PRICES: [Price; 4] = [
    Price {
        label: "8 MiB cap: 8 items x ~1 MiB",
        items: 8.0,
        mib: 8.0,
    },
    Price {
        label: "single 14 MiB pair (first-item rule)",
        items: 1.0,
        mib: 14.0,
    },
    Price {
        label: "16 MiB cap: 16 items x ~1 MiB",
        items: 16.0,
        mib: 16.0,
    },
    Price {
        label: "32 MiB cap: 26 items",
        items: 26.0,
        mib: 32.0,
    },
];

/// A fixed shuffle of `0..len`, so runs interleave shapes the same way.
pub(crate) fn shuffled(len: usize, seed: u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    let mut state = seed | 1;
    for i in (1..len).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        order.swap(i, (state % (i as u64 + 1)) as usize);
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_recovers_per_drain_per_item_and_per_mib_terms() {
        let points: Vec<(f64, f64, f64)> = [(1.0, 2.0), (10.0, 2.0), (1.0, 32.0), (50.0, 16.0)]
            .iter()
            .map(|&(items, mib)| (items, mib, 40.0 + 3.0 * items + 250.0 * mib))
            .collect();
        let f = fit(&points, true).expect("separable");
        assert!((f.per_drain_us - 40.0).abs() < 1e-6);
        assert!((f.per_item_us - 3.0).abs() < 1e-6);
        assert!((f.per_mib_us - 250.0).abs() < 1e-6);
        // Items and MiB in fixed proportion cannot separate.
        let collinear = [(1.0, 1.0, 5.0), (2.0, 2.0, 9.0), (4.0, 4.0, 17.0)];
        assert!(fit(&collinear, true).is_none());
        // Near-collinear, with large magnitudes: the scale-relative pivot floor
        // reports singular where an absolute 1e-9 would solve noise.
        let near: Vec<(f64, f64, f64)> = [1.0e3, 2.0e3, 3.0e3, 4.0e3]
            .iter()
            .map(|&x| (x, x * (1.0 + 1e-15), 7.0 * x))
            .collect();
        assert!(fit(&near, false).is_none());
    }

    #[test]
    fn a_drain_that_created_a_staging_buffer_is_cold_even_when_also_excluded() {
        assert_eq!(DrainClass::of(3, 3, None), DrainClass::Warm);
        assert_eq!(DrainClass::of(3, 5, None), DrainClass::Cold { created: 2 });
        assert_eq!(
            DrainClass::of(3, 4, Some("pool growth")),
            DrainClass::Cold { created: 1 }
        );
        assert_eq!(
            DrainClass::of(3, 3, Some("pool growth")),
            DrainClass::Excluded("pool growth")
        );
    }
}
