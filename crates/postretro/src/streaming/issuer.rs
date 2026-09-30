//! The one read issuer thread: every streamed positional read, tiered, in offset order.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency"

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use postretro_level_loader::PrlLoadError;

use super::request::{MAX_READ_RANGES, ReadRequest, StreamResource};
use super::schedule::{RangeSlot, ReadPlan, plan_next_read, split_span};

/// The issuer thread's name. It predates the shared layer; tests and
/// diagnostics name it.
pub(crate) const READ_ISSUER_THREAD_NAME: &str = "sh-probe-read";

/// One resource's side of the shared issuer: its target membership, its
/// reader, its in-memory byte accounting, and where its bytes go. SH hands
/// read bytes to its decode pool; lightmap blocks, which upload as stored,
/// hand them straight to a completion queue.
///
/// Every method runs on the issuer thread. An implementor must not block
/// except in `read_span` and in `deliver`'s bounded queue send.
pub(crate) trait ReadRoute: Send {
    /// Whether the resource still targets `key`. Checked before every
    /// physical read; a request whose key fails it is cancelled whole.
    fn is_targeted(&self, key: u32) -> bool;
    /// One physical positional read. Must return exactly the span's length,
    /// and may reject a span outside the resource's own sections.
    fn read_span(&self, span: Range<u64>) -> Result<Vec<u8>, PrlLoadError>;
    /// Payload bytes (gaps excluded) a read is about to bring into memory.
    /// They stay charged after delivery until the resource consumes them.
    fn charge_read_bytes(&self, bytes: u64);
    /// Returns charged bytes that will never be delivered: a failed or
    /// cancelled read, a request cancelled or failed after a partial read,
    /// or a partly read request still pending when the issuer exits.
    fn release_read_bytes(&self, bytes: u64);
    /// Counters for one successful physical read.
    fn record_read(&self, read: &ReadRecord<'_>);
    /// Hands over a request's one outcome. False when the resource's
    /// receiver is gone; the issuer then exits.
    fn deliver(&self, request: ReadRequest, outcome: ReadOutcome) -> bool;
}

/// One successful physical read, as its route's counters see it.
#[derive(Debug)]
pub(crate) struct ReadRecord<'a> {
    pub(crate) span_bytes: u64,
    /// Ranges the read served; more than one means it was coalesced.
    pub(crate) ranges_served: usize,
    /// Bytes read only to bridge ranges, then discarded.
    pub(crate) gap_bytes: u64,
    /// Submission-to-bytes-in-hand time of each served range, in read order.
    pub(crate) latencies: &'a [Duration],
}

#[derive(Debug)]
pub(crate) enum ReadOutcome {
    /// Every range was read.
    Read(ReadParts),
    /// A read the request needed failed; the whole request fails.
    Failed(PrlLoadError),
    /// The resource stopped targeting the key before every range was read.
    Cancelled,
}

/// One buffer per request range, in the request's range order. An empty
/// range yields an empty buffer.
#[derive(Debug, Default)]
pub(crate) struct ReadParts {
    parts: [Vec<u8>; MAX_READ_RANGES],
    len: usize,
}

impl IntoIterator for ReadParts {
    type Item = Vec<u8>;
    type IntoIter = std::iter::Take<std::array::IntoIter<Vec<u8>, MAX_READ_RANGES>>;

    fn into_iter(self) -> Self::IntoIter {
        self.parts.into_iter().take(self.len)
    }
}

/// The routes an issuer serves, one per resource at most.
#[derive(Default)]
pub(crate) struct ReadRoutes {
    routes: [Option<Box<dyn ReadRoute>>; StreamResource::COUNT],
}

impl ReadRoutes {
    pub(crate) fn with(mut self, resource: StreamResource, route: Box<dyn ReadRoute>) -> Self {
        self.routes[resource.index()] = Some(route);
        self
    }

    fn route(&self, resource: StreamResource) -> &dyn ReadRoute {
        self.routes[resource.index()]
            .as_deref()
            .expect("submit admits only routed resources")
    }
}

/// Frame-side handle of the issuer thread. The thread exits once every clone
/// is dropped and its queue is empty, or at [`Self::cancel`], before its next
/// read. A level-scope owner clones it for each resource that submits.
#[derive(Debug, Clone)]
pub(crate) struct ReadIssuer {
    requests: SyncSender<IssuerMessage>,
    cancel: Arc<AtomicBool>,
    routed: [bool; StreamResource::COUNT],
}

impl ReadIssuer {
    /// Spawns the issuer thread. `queue_capacity` must cover every request
    /// the routed resources can have outstanding (their permits), so a
    /// submission never finds the queue full.
    pub(crate) fn spawn(
        routes: ReadRoutes,
        queue_capacity: usize,
    ) -> std::io::Result<(Self, JoinHandle<()>)> {
        let (requests, receiver) = sync_channel(queue_capacity);
        let cancel = Arc::new(AtomicBool::new(false));
        let routed = routes.routes.each_ref().map(Option::is_some);
        let thread_cancel = Arc::clone(&cancel);
        let handle = thread::Builder::new()
            .name(READ_ISSUER_THREAD_NAME.into())
            .spawn(move || issuer_loop(&receiver, &routes, &thread_cancel))?;
        Ok((
            Self {
                requests,
                cancel,
                routed,
            },
            handle,
        ))
    }

    /// Queues one request without blocking. Rejects a resource the issuer
    /// has no route for.
    pub(crate) fn submit(&self, request: ReadRequest) -> Result<(), &'static str> {
        if !self.routed[request.resource.index()] {
            return Err("read issuer has no route for this resource");
        }
        self.requests
            .try_send(IssuerMessage::Read(SubmittedRead {
                request,
                submitted_at: Instant::now(),
            }))
            .map_err(|_| "read issuer queue full or disconnected")
    }

    /// Stops the thread before its next read, and wakes it if it is idle, so
    /// it exits even while another clone of this handle is alive. An OS read
    /// already in flight finishes; its bytes are released, never delivered.
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
        // Best effort: a full queue already wakes the thread, and a thread
        // that has exited needs no wake.
        let _ = self.requests.try_send(IssuerMessage::Wake);
    }
}

/// What the frame side sends the issuer thread.
enum IssuerMessage {
    Read(SubmittedRead),
    /// Sent by [`ReadIssuer::cancel`] so an idle thread sees the flag.
    Wake,
}

/// A request stamped when the frame thread handed it over, so read latency
/// covers queueing behind earlier reads.
struct SubmittedRead {
    request: ReadRequest,
    submitted_at: Instant,
}

struct PendingRead {
    request: ReadRequest,
    submitted_at: Instant,
    parts: [Vec<u8>; MAX_READ_RANGES],
    /// Bit `i` is set while range `i` is still unread.
    unread: u8,
    /// Charged bytes of ranges already read, held until the request resolves.
    held_bytes: u64,
}

impl PendingRead {
    fn into_parts(self) -> ReadParts {
        ReadParts {
            parts: self.parts,
            len: self.request.ranges.len(),
        }
    }
}

/// The issuer thread's pending reads. Every exit (cancel, disconnect, a gone
/// receiver) drops them, returning each partly read request's held bytes to
/// its route: a kept resource's ledger outlives this thread.
struct PendingReads<'a> {
    reads: Vec<PendingRead>,
    routes: &'a ReadRoutes,
}

impl Drop for PendingReads<'_> {
    fn drop(&mut self) {
        for read in self.reads.drain(..) {
            if read.held_bytes != 0 {
                self.routes
                    .route(read.request.resource)
                    .release_read_bytes(read.held_bytes);
            }
        }
    }
}

/// Planner input rebuilt before every read, kept across reads so steady
/// streaming reuses its capacity.
#[derive(Default)]
struct PlanScratch {
    slots: Vec<RangeSlot>,
    /// `(pending index, range index)` of each slot.
    owners: Vec<(usize, usize)>,
    latencies: Vec<Duration>,
}

impl PlanScratch {
    fn collect(&mut self, pending: &[PendingRead]) {
        self.slots.clear();
        self.owners.clear();
        for (pending_index, read) in pending.iter().enumerate() {
            for range_index in 0..read.request.ranges.len() {
                if read.unread & (1 << range_index) != 0 {
                    self.slots.push(RangeSlot {
                        resource: read.request.resource,
                        tier: read.request.tier,
                        range: read.request.ranges.get(range_index),
                    });
                    self.owners.push((pending_index, range_index));
                }
            }
        }
    }
}

/// Runs until cancellation or until every [`ReadIssuer`] handle is dropped.
///
/// Each pass takes every newly submitted request, cancels pending work whose
/// key left its resource's targets, then issues one planned physical read.
/// Taking new requests between reads lets fresh mandatory work preempt the
/// rest of the optional tier.
fn issuer_loop(requests: &Receiver<IssuerMessage>, routes: &ReadRoutes, cancel: &AtomicBool) {
    let mut pending_reads = PendingReads {
        reads: Vec::new(),
        routes,
    };
    let pending = &mut pending_reads.reads;
    let mut scratch = PlanScratch::default();
    loop {
        if pending.is_empty() {
            match requests.recv() {
                Ok(IssuerMessage::Read(submitted)) => {
                    if !accept(submitted, pending, routes) {
                        return;
                    }
                }
                Ok(IssuerMessage::Wake) => {}
                Err(_) => return,
            }
        }
        loop {
            match requests.try_recv() {
                Ok(IssuerMessage::Read(submitted)) => {
                    if !accept(submitted, pending, routes) {
                        return;
                    }
                }
                Ok(IssuerMessage::Wake) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        if cancel.load(Ordering::Acquire) {
            return;
        }
        if !cancel_departed(pending, routes) {
            return;
        }
        scratch.collect(pending);
        let Some(plan) = plan_next_read(&scratch.slots) else {
            continue;
        };
        if !issue_read(&plan, pending, &mut scratch, routes, cancel) {
            return;
        }
    }
}

/// Queues one submission. An identical pending request absorbs it; a request
/// with nothing to read completes at once, unless its key has departed.
fn accept(submitted: SubmittedRead, pending: &mut Vec<PendingRead>, routes: &ReadRoutes) -> bool {
    let request = submitted.request;
    if let Some(existing) = pending
        .iter_mut()
        .find(|read| read.request.same_item(&request))
    {
        existing.request.tier = existing.request.tier.min(request.tier);
        return true;
    }
    let mut read = PendingRead {
        request,
        submitted_at: submitted.submitted_at,
        parts: Default::default(),
        unread: 0,
        held_bytes: 0,
    };
    for range_index in 0..request.ranges.len() {
        if !request.ranges.get(range_index).is_empty() {
            read.unread |= 1 << range_index;
        }
    }
    if read.unread != 0 {
        pending.push(read);
        return true;
    }
    let route = routes.route(request.resource);
    if !route.is_targeted(request.key) {
        return route.deliver(request, ReadOutcome::Cancelled);
    }
    route.deliver(request, ReadOutcome::Read(read.into_parts()))
}

/// Cancels, in pending order, every request whose key left its resource's
/// targets, dropping any bytes already read for it.
fn cancel_departed(pending: &mut Vec<PendingRead>, routes: &ReadRoutes) -> bool {
    let mut index = 0;
    while index < pending.len() {
        let route = routes.route(pending[index].request.resource);
        if route.is_targeted(pending[index].request.key) {
            index += 1;
            continue;
        }
        let read = pending.remove(index);
        if read.held_bytes != 0 {
            route.release_read_bytes(read.held_bytes);
        }
        if !route.deliver(read.request, ReadOutcome::Cancelled) {
            return false;
        }
    }
    true
}

/// One physical read for `plan`, split into per-range buffers. A request
/// completes once its last range is in hand. Gap bytes drop with the span.
fn issue_read(
    plan: &ReadPlan,
    pending: &mut Vec<PendingRead>,
    scratch: &mut PlanScratch,
    routes: &ReadRoutes,
    cancel: &AtomicBool,
) -> bool {
    let route = routes.route(plan.resource);
    let span = plan.span.clone();
    let span_bytes = span.end - span.start;
    let gap_bytes = plan.gap_bytes(&scratch.slots);
    let payload_bytes = span_bytes.saturating_sub(gap_bytes);
    route.charge_read_bytes(payload_bytes);
    let result = route.read_span(span.clone());
    let in_hand = Instant::now();
    let result = result.and_then(|bytes| {
        if bytes.len() as u64 == span_bytes {
            Ok(bytes)
        } else {
            Err(span_error(
                plan.resource,
                &span,
                format!("returned {} bytes", bytes.len()),
            ))
        }
    });
    let cancelled = cancel.load(Ordering::Acquire);
    if cancelled || result.is_err() {
        route.release_read_bytes(payload_bytes);
    }
    if cancelled {
        // The pending reads' held bytes are released as the loop exits.
        return false;
    }
    let bytes = match result {
        Ok(bytes) => bytes,
        Err(error) => return fail_members(plan, error, pending, scratch, route),
    };
    scratch.latencies.clear();
    scratch.latencies.extend(
        plan.members
            .iter()
            .map(|&slot| in_hand.duration_since(pending[scratch.owners[slot].0].submitted_at)),
    );
    route.record_read(&ReadRecord {
        span_bytes,
        ranges_served: plan.members.len(),
        gap_bytes,
        latencies: &scratch.latencies,
    });
    let ranges: Vec<Range<u64>> = plan
        .members
        .iter()
        .map(|&slot| scratch.slots[slot].range.clone())
        .collect();
    let parts = split_span(&span, bytes, &ranges);
    for (&slot, part) in plan.members.iter().zip(parts) {
        let (pending_index, range_index) = scratch.owners[slot];
        let read = &mut pending[pending_index];
        read.held_bytes += part.len() as u64;
        read.parts[range_index] = part;
        read.unread &= !(1 << range_index);
    }
    deliver_completed(plan, pending, scratch, route)
}

/// Delivers the requests this read completed, in read order. The rest keep
/// their pending order.
fn deliver_completed(
    plan: &ReadPlan,
    pending: &mut Vec<PendingRead>,
    scratch: &PlanScratch,
    route: &dyn ReadRoute,
) -> bool {
    let mut taken: Vec<Option<PendingRead>> = pending.drain(..).map(Some).collect();
    for &slot in &plan.members {
        let (pending_index, _) = scratch.owners[slot];
        if taken[pending_index]
            .as_ref()
            .is_some_and(|read| read.unread == 0)
        {
            let read = taken[pending_index].take().expect("checked above");
            let request = read.request;
            if !route.deliver(request, ReadOutcome::Read(read.into_parts())) {
                // Undelivered requests go back, to be released as the loop exits.
                pending.extend(taken.into_iter().flatten());
                return false;
            }
        }
    }
    pending.extend(taken.into_iter().flatten());
    true
}

/// A failed read fails every request it served, whole. Each gets its own
/// error because `PrlLoadError` is not `Clone`.
fn fail_members(
    plan: &ReadPlan,
    error: PrlLoadError,
    pending: &mut Vec<PendingRead>,
    scratch: &PlanScratch,
    route: &dyn ReadRoute,
) -> bool {
    let mut failed: Vec<usize> = Vec::with_capacity(plan.members.len());
    for &slot in &plan.members {
        let (pending_index, _) = scratch.owners[slot];
        if !failed.contains(&pending_index) {
            failed.push(pending_index);
        }
    }
    let shared_message = (failed.len() > 1).then(|| error.to_string());
    let mut original = Some(error);
    let mut taken: Vec<Option<PendingRead>> = pending.drain(..).map(Some).collect();
    for pending_index in failed {
        let read = taken[pending_index]
            .take()
            .expect("failed requests are distinct");
        if read.held_bytes != 0 {
            route.release_read_bytes(read.held_bytes);
        }
        let error = match &shared_message {
            Some(message) => span_error(plan.resource, &plan.span, message.clone()),
            None => original
                .take()
                .expect("a single request takes the original"),
        };
        if !route.deliver(read.request, ReadOutcome::Failed(error)) {
            // Undelivered requests go back, to be released as the loop exits.
            pending.extend(taken.into_iter().flatten());
            return false;
        }
    }
    pending.extend(taken.into_iter().flatten());
    true
}

fn span_error(resource: StreamResource, span: &Range<u64>, message: String) -> PrlLoadError {
    PrlLoadError::SectionValidation {
        section: resource.label(),
        message: format!("read of file span {}..{}: {message}", span.start, span.end),
    }
}

#[cfg(test)]
#[path = "issuer_tests.rs"]
mod tests;
