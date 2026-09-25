//! The single thread that issues every SH positional read, in the order the
//! scheduler plans, and hands encoded bytes to the decode pool.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency".

use std::ops::Range;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError};
use std::time::Instant;

use postretro_level_loader::PrlLoadError;

use super::schedule::{ChunkSlot, plan_next_read, split_span};
use super::{
    DecodeJob, ShWorkerCompletion, ShWorkerResult, ShWorkerSource, SubmittedRequest, WorkerShared,
};
use crate::sh_streaming::controller::ShClusterRequest;

pub(super) struct IssuerChannels {
    pub(super) requests: Receiver<SubmittedRequest>,
    pub(super) decode: SyncSender<DecodeJob>,
    pub(super) completed: SyncSender<ShWorkerCompletion>,
}

impl IssuerChannels {
    /// False once the session has dropped its receiver; the issuer then exits.
    fn complete(&self, request: ShClusterRequest, result: ShWorkerResult) -> bool {
        self.completed
            .send(ShWorkerCompletion {
                request,
                result,
                ready_bytes: 0,
            })
            .is_ok()
    }
}

struct PendingRead {
    request: ShClusterRequest,
    range: Range<u64>,
    submitted_at: Instant,
}

/// Runs until cancellation or until the session drops its request sender.
///
/// Each pass takes every newly submitted request, drops pending work whose
/// cluster left the target set (completing it `Cancelled`), then issues one
/// planned physical read. Taking new requests between reads lets fresh
/// mandatory work preempt the rest of the optional tier.
pub(super) fn issuer_loop(
    channels: IssuerChannels,
    shared: &WorkerShared,
    source: &dyn ShWorkerSource,
) {
    let mut pending: Vec<PendingRead> = Vec::new();
    loop {
        if pending.is_empty() {
            let Ok(submitted) = channels.requests.recv() else {
                return;
            };
            if !accept(submitted, &mut pending, &channels, shared, source) {
                return;
            }
        }
        loop {
            match channels.requests.try_recv() {
                Ok(submitted) => {
                    if !accept(submitted, &mut pending, &channels, shared, source) {
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        if shared.cancelled() {
            return;
        }

        let mut targeted = Vec::with_capacity(pending.len());
        for read in pending.drain(..) {
            if shared.targets.contains(read.request.cluster_id) {
                targeted.push(read);
            } else if !channels.complete(read.request, ShWorkerResult::Cancelled) {
                return;
            }
        }
        pending = targeted;

        let slots: Vec<ChunkSlot> = pending
            .iter()
            .map(|read| ChunkSlot {
                mandatory: read.request.mandatory,
                range: read.range.clone(),
            })
            .collect();
        let Some(plan) = plan_next_read(&slots) else {
            continue;
        };
        let gap_bytes = plan.gap_bytes(&slots);
        let mut taken: Vec<Option<PendingRead>> = pending.drain(..).map(Some).collect();
        let members: Vec<PendingRead> = plan
            .members
            .iter()
            .map(|&index| taken[index].take().expect("planned members are distinct"))
            .collect();
        pending = taken.into_iter().flatten().collect();
        if !issue_read(plan.span, members, gap_bytes, &channels, shared, source) {
            return;
        }
    }
}

/// Resolves a request's file range. A canonical empty cluster needs no read
/// and goes straight to the pool; a range error completes as a failure.
fn accept(
    submitted: SubmittedRequest,
    pending: &mut Vec<PendingRead>,
    channels: &IssuerChannels,
    shared: &WorkerShared,
    source: &dyn ShWorkerSource,
) -> bool {
    let request = submitted.request;
    match source.chunk_file_range(request.cluster_id) {
        Err(error) => channels.complete(request, ShWorkerResult::Failed(error)),
        Ok(range) if range.is_empty() => {
            if !shared.targets.contains(request.cluster_id) {
                return channels.complete(request, ShWorkerResult::Cancelled);
            }
            channels
                .decode
                .send(DecodeJob {
                    request,
                    bytes: Vec::new(),
                })
                .is_ok()
        }
        Ok(range) => {
            pending.push(PendingRead {
                request,
                range,
                submitted_at: submitted.submitted_at,
            });
            true
        }
    }
}

/// One physical read for `members`, split into per-chunk decode jobs. Gap
/// bytes are dropped with the span buffer.
fn issue_read(
    span: Range<u64>,
    members: Vec<PendingRead>,
    gap_bytes: u64,
    channels: &IssuerChannels,
    shared: &WorkerShared,
    source: &dyn ShWorkerSource,
) -> bool {
    let span_bytes = span.end - span.start;
    let encoded_bytes = span_bytes.saturating_sub(gap_bytes);
    shared
        .phases
        .lock()
        .expect("SH phase mutex poisoned")
        .encoded
        .add(encoded_bytes, "encoded worker bytes")
        .expect("validated encoded bound");
    let result = source.read_file_span(span.clone());
    let in_hand = Instant::now();
    let result = result.and_then(|bytes| {
        if bytes.len() as u64 == span_bytes {
            Ok(bytes)
        } else {
            Err(span_error(&span, format!("returned {} bytes", bytes.len())))
        }
    });
    if shared.cancelled() || result.is_err() {
        shared
            .phases
            .lock()
            .expect("SH phase mutex poisoned")
            .encoded
            .remove(encoded_bytes, "encoded worker bytes")
            .expect("balanced encoded read");
    }
    if shared.cancelled() {
        return false;
    }
    let bytes = match result {
        Ok(bytes) => bytes,
        Err(error) => return fail_members(members, error, &span, channels),
    };
    {
        let mut stats = shared.stats.lock().expect("SH stats mutex poisoned");
        stats.record_read(span_bytes, members.len(), gap_bytes);
        for member in &members {
            stats
                .read_latency
                .record(in_hand.duration_since(member.submitted_at));
        }
    }
    let ranges: Vec<Range<u64>> = members.iter().map(|member| member.range.clone()).collect();
    let parts = split_span(&span, bytes, &ranges);
    for (member, bytes) in members.into_iter().zip(parts) {
        let job = DecodeJob {
            request: member.request,
            bytes,
        };
        if channels.decode.send(job).is_err() {
            return false;
        }
    }
    true
}

/// A failed coalesced read fails every chunk it covered; each gets its own
/// error because `PrlLoadError` is not `Clone`.
fn fail_members(
    members: Vec<PendingRead>,
    error: PrlLoadError,
    span: &Range<u64>,
    channels: &IssuerChannels,
) -> bool {
    let shared_message = (members.len() > 1).then(|| error.to_string());
    let mut original = Some(error);
    for member in members {
        let error = match &shared_message {
            Some(message) => span_error(span, message.clone()),
            None => original.take().expect("single member takes the original"),
        };
        if !channels.complete(member.request, ShWorkerResult::Failed(error)) {
            return false;
        }
    }
    true
}

fn span_error(span: &Range<u64>, message: String) -> PrlLoadError {
    PrlLoadError::SectionValidation {
        section: "SH streaming",
        message: format!("read of file span {}..{}: {message}", span.start, span.end),
    }
}
