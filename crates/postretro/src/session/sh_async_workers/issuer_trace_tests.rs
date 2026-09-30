//! SH streaming regression baseline, issuer half: submits a fixed request mix
//! to the real issuer thread and compares its physical read order, coalesced
//! spans and cancellations against a committed trace. Recorded before the
//! shared streaming layer was extracted; the extraction must replay it
//! unchanged (brief AC 18). The controller half is
//! `sh_streaming/controller_trace_tests.rs`.
//!
//! Every read blocks until the test releases it, so each step's publishes and
//! submissions land while the issuer is held in a known read. From idle, the
//! step's first request is submitted alone and its read starts before the rest
//! are queued, as a real idle issuer would do. Each read returns bytes tagged
//! with its read index, so the decoded chunk names the read that served it.
//!
//! Regenerate only when a behaviour change is intended:
//! `POSTRETRO_REGEN_SH_TRACE=1 cargo test -p postretro --bin postretro sh_trace`.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency".

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::ops::Range;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_level_loader::PrlLoadError;

use super::*;
use crate::sh_streaming::trace_fixture::assert_matches_baseline;

const BASELINE: &str = "sh_issuer_trace_baseline.txt";
const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
/// Mirrors the issuer's 256 KiB coalescing gap cap; the layout sits on it.
const GAP_CAP: u64 = 256 * KIB;

/// Absolute file range of each cluster's chunk.
fn layout() -> Vec<Range<u64>> {
    let c4_start = 2 * MIB + 64 * KIB + GAP_CAP;
    let c12_start = c4_start + 32 * KIB + GAP_CAP + 1;
    vec![
        0..64 * KIB,                               // 0
        64 * KIB..128 * KIB,                       // 1: adjacent to 0
        228 * KIB..292 * KIB,                      // 2: 100 KiB after 1
        2 * MIB..2 * MIB + 64 * KIB,               // 3
        c4_start..c4_start + 32 * KIB,             // 4: exactly the gap cap after 3
        150 * KIB..170 * KIB,                      // 5: inside the 1..2 gap
        8 * MIB..18 * MIB,                         // 6: 10 MiB
        18 * MIB..26 * MIB,                        // 7: 8 MiB, adjacent; 6+7 exceeds the span cap
        26 * MIB..26 * MIB,                        // 8: canonical empty chunk
        40 * MIB..40 * MIB + 64 * KIB,             // 9
        41 * MIB..41 * MIB + 64 * KIB,             // 10
        64 * MIB..64 * MIB + 64,                   // 11
        c12_start..c12_start + 16 * KIB,           // 12: one byte past the gap cap after 4
        80 * MIB..80 * MIB + 64 * KIB,             // 13
        80 * MIB + 64 * KIB..80 * MIB + 128 * KIB, // 14: adjacent to 13
        100 * MIB..100 * MIB + 64 * KIB,           // 15
    ]
}

/// One test action taken while the issuer is idle or held in a read. The
/// held read is released after it.
struct Step {
    note: &'static str,
    publish: Option<&'static [u32]>,
    /// (cluster, mandatory), in submission order.
    submit: &'static [(u32, bool)],
}

const ALL: &[u32] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
const ALL_BUT_10: &[u32] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13, 14, 15];

const STEPS: &[Step] = &[
    Step {
        note: "idle; mixed tiers in shuffled order",
        publish: Some(ALL),
        submit: &[
            (11, true),
            (9, false),
            (10, false),
            (5, false),
            (2, true),
            (0, true),
            (1, true),
            (13, false),
        ],
    },
    Step {
        note: "10 leaves the target set before its read; gap-cap neighbours arrive",
        publish: Some(ALL_BUT_10),
        submit: &[(3, true), (4, true), (12, true)],
    },
    Step {
        note: "span-cap pair, an empty chunk, and an optional neighbour of 13",
        publish: None,
        submit: &[(14, false), (6, true), (7, true), (8, true)],
    },
    Step {
        note: "no new work",
        publish: None,
        submit: &[],
    },
    Step {
        note: "no new work",
        publish: None,
        submit: &[],
    },
    Step {
        note: "no new work",
        publish: None,
        submit: &[],
    },
    Step {
        note: "fresh mandatory work arrives during the optional tier",
        publish: None,
        submit: &[(15, true)],
    },
    Step {
        note: "no new work",
        publish: None,
        submit: &[],
    },
    Step {
        note: "no new work",
        publish: None,
        submit: &[],
    },
    Step {
        note: "no new work",
        publish: None,
        submit: &[],
    },
    Step {
        note: "idle again; 10 is targeted and requested anew",
        publish: Some(ALL),
        submit: &[(10, false)],
    },
];

/// Records each physical read and holds it until the test releases it.
struct SteppedSource {
    ranges: Vec<Range<u64>>,
    reads: Mutex<Vec<Range<u64>>>,
    /// Reads with an index below this may return.
    released: Mutex<usize>,
    gate: Condvar,
}

impl SteppedSource {
    fn reads(&self) -> Vec<Range<u64>> {
        self.reads.lock().unwrap().clone()
    }

    fn release_through(&self, read_count: usize) {
        *self.released.lock().unwrap() = read_count;
        self.gate.notify_all();
    }
}

impl ShWorkerSource for SteppedSource {
    fn cluster_count(&self) -> u32 {
        self.ranges.len() as u32
    }

    fn chunk_file_range(&self, cluster_id: u32) -> Result<Range<u64>, PrlLoadError> {
        Ok(self.ranges[cluster_id as usize].clone())
    }

    fn decoded_bytes(&self, cluster_id: u32) -> u64 {
        let range = &self.ranges[cluster_id as usize];
        range.end - range.start
    }

    fn read_file_span(&self, range: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
        let index = {
            let mut reads = self.reads.lock().unwrap();
            reads.push(range.clone());
            reads.len() - 1
        };
        let mut released = self.released.lock().unwrap();
        while *released <= index {
            released = self.gate.wait(released).unwrap();
        }
        Ok(vec![index as u8; (range.end - range.start) as usize])
    }

    fn decode(
        &self,
        cluster_id: u32,
        bytes: Vec<u8>,
    ) -> Result<DecodedClusterShPayload, PrlLoadError> {
        Ok(DecodedClusterShPayload {
            cluster_id,
            bytes,
            blocks: Vec::new(),
        })
    }
}

struct ReleaseAllOnDrop(Arc<SteppedSource>);

impl Drop for ReleaseAllOnDrop {
    fn drop(&mut self) {
        // A poisoned lock still holds the counter; release through it.
        let mut released = self.0.released.lock().unwrap_or_else(|p| p.into_inner());
        *released = usize::MAX;
        self.0.gate.notify_all();
    }
}

fn request(cluster_id: u32, mandatory: bool) -> ShClusterRequest {
    ShClusterRequest {
        generation: 1,
        content_tag: [1; 32],
        cluster_id,
        chunk_hash: [cluster_id as u8; 32],
        mandatory,
    }
}

fn ids(ids: impl IntoIterator<Item = u32>) -> String {
    let ids: Vec<String> = ids.into_iter().map(|id| id.to_string()).collect();
    format!("[{}]", ids.join(" "))
}

fn span(range: &Range<u64>) -> String {
    format!(
        "{}..{} ({}B)",
        range.start,
        range.end,
        range.end - range.start
    )
}

/// Drains completions until `done` holds for the recorded read count and the
/// number of completions received so far.
fn wait_until(
    workers: &ShAsyncWorkers,
    source: &SteppedSource,
    completions: &mut Vec<ShWorkerCompletion>,
    done: impl Fn(usize, usize) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        while let Some(completion) = workers.try_completion().unwrap() {
            completions.push(completion);
        }
        if done(source.reads.lock().unwrap().len(), completions.len()) {
            while let Some(completion) = workers.try_completion().unwrap() {
                completions.push(completion);
            }
            return;
        }
        assert!(Instant::now() < deadline, "issuer trace step stalled");
        std::thread::yield_now();
    }
}

fn run_schedule() -> String {
    let ranges = layout();
    let source = Arc::new(SteppedSource {
        ranges: ranges.clone(),
        reads: Mutex::new(Vec::new()),
        released: Mutex::new(0),
        gate: Condvar::new(),
    });
    let mut workers = ShAsyncWorkers::with_source(source.clone(), 2).unwrap();
    // Dropped before `workers`, so a failed assertion releases a held read
    // instead of hanging the join in `ShAsyncWorkers::drop`.
    let _release_on_unwind = ReleaseAllOnDrop(Arc::clone(&source));
    let mut out = String::new();
    writeln!(
        out,
        "# SH issuer trace baseline. Produced by\n\
         # session::sh_async_workers::issuer_trace_tests::sh_trace_issuer_schedule_matches_baseline\n\
         # from the pre-extraction SH read issuer (brief AC 18).\n\
         # Steps act while the issuer is idle or held in a read, then release it.\n\
         # Reads list each physical span in issue order with the chunks it served\n\
         # and the discarded gap bytes. Do not regenerate to make a refactor pass."
    )
    .unwrap();
    writeln!(out, "layout:").unwrap();
    for (cluster_id, range) in ranges.iter().enumerate() {
        writeln!(out, "  chunk {cluster_id}: {}", span(range)).unwrap();
    }

    let mut completions: Vec<ShWorkerCompletion> = Vec::new();
    let mut submitted = 0usize;
    let mut held: Option<usize> = None;
    for (index, step) in STEPS.iter().enumerate() {
        match held {
            Some(read) => writeln!(out, "step {index} (held in read {read}): {}", step.note),
            None => writeln!(out, "step {index} (idle): {}", step.note),
        }
        .unwrap();
        if let Some(targets) = step.publish {
            workers.publish_targets(&targets.iter().copied().collect::<BTreeSet<_>>());
            writeln!(out, "  publish targets {}", ids(targets.iter().copied())).unwrap();
        }
        let mut submissions = step.submit.iter().copied();
        if held.is_none() {
            let (cluster_id, mandatory) = submissions
                .next()
                .expect("an idle step starts the issuer with a request");
            // Counted before submitting: an idle issuer may start the read
            // before `submit` returns.
            let reads_before = source.reads().len();
            workers.submit(request(cluster_id, mandatory)).unwrap();
            submitted += 1;
            wait_until(&workers, &source, &mut completions, |reads, _| {
                reads > reads_before
            });
            held = Some(reads_before);
            writeln!(
                out,
                "  submit {cluster_id} {}; read {reads_before} starts {}",
                if mandatory { "m" } else { "o" },
                span(&source.reads()[reads_before])
            )
            .unwrap();
        }
        let rest: Vec<String> = submissions
            .map(|(cluster_id, mandatory)| {
                workers.submit(request(cluster_id, mandatory)).unwrap();
                submitted += 1;
                format!("{cluster_id} {}", if mandatory { "m" } else { "o" })
            })
            .collect();
        if !rest.is_empty() {
            writeln!(out, "  submit {}", rest.join(", ")).unwrap();
        }

        let read = held.expect("a step always ends holding a read");
        let completed_before = completions.len();
        source.release_through(read + 1);
        wait_until(&workers, &source, &mut completions, |reads, done| {
            reads > read + 1 || done == submitted
        });
        let cancelled: Vec<u32> = completions[completed_before..]
            .iter()
            .filter(|completion| matches!(completion.result, ShWorkerResult::Cancelled))
            .map(|completion| completion.request.cluster_id)
            .collect();
        if !cancelled.is_empty() {
            writeln!(out, "  cancelled unread: {}", ids(cancelled)).unwrap();
        }
        let reads = source.reads();
        if reads.len() > read + 1 {
            held = Some(read + 1);
            writeln!(
                out,
                "  release read {read}; read {} starts {}",
                read + 1,
                span(&reads[read + 1])
            )
            .unwrap();
        } else {
            held = None;
            writeln!(out, "  release read {read}; issuer idle").unwrap();
        }
    }
    assert!(held.is_none(), "the schedule ends with the issuer idle");
    assert_eq!(completions.len(), submitted);
    workers.stop();

    // Attribute each prepared chunk to the read whose index tags its bytes.
    let mut members: BTreeMap<usize, Vec<u32>> = BTreeMap::new();
    let mut no_read = Vec::new();
    for completion in &completions {
        let cluster_id = completion.request.cluster_id;
        match &completion.result {
            ShWorkerResult::Prepared(chunk) => {
                let range = &ranges[cluster_id as usize];
                assert_eq!(chunk.bytes.len() as u64, range.end - range.start);
                match chunk.bytes.first() {
                    Some(&read) => members.entry(read as usize).or_default().push(cluster_id),
                    None => no_read.push(cluster_id),
                }
            }
            ShWorkerResult::Cancelled => {}
            ShWorkerResult::Failed(error) => panic!("cluster {cluster_id} failed: {error}"),
        }
    }
    writeln!(out, "reads:").unwrap();
    for (index, read) in source.reads().iter().enumerate() {
        let mut served = members.remove(&index).unwrap_or_default();
        served.sort_by_key(|&cluster_id| ranges[cluster_id as usize].start);
        let chunk_bytes: u64 = served
            .iter()
            .map(|&cluster_id| {
                let range = &ranges[cluster_id as usize];
                range.end - range.start
            })
            .sum();
        writeln!(
            out,
            "  read {index}: {} chunks {} gap {}B",
            span(read),
            ids(served),
            (read.end - read.start) - chunk_bytes
        )
        .unwrap();
    }
    no_read.sort_unstable();
    writeln!(out, "prepared without a read: {}", ids(no_read)).unwrap();
    out
}

#[test]
fn sh_trace_issuer_schedule_matches_baseline() {
    assert_matches_baseline(BASELINE, &run_schedule());
}
