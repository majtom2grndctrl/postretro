//! The level's one read issuer and decode pool, and the retirement of a replaced level's I/O.
//! See: context/lib/rendering_pipeline.md §4

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;

use crate::lightmap_streaming::controller::LIGHTMAP_QUEUE_CAPACITY;
use crate::lightmap_streaming::route::{LightmapCompletion, LightmapRouteLedger};
use crate::lightmap_streaming::source::LightmapBlockSource;
use crate::session::sh_async_workers::ShWorkerRetirement;
use crate::sh_streaming::controller::MAX_STREAM_PERMITS;
use crate::streaming::decode_pool::{DecodePool, DecodeQueue, decode_pool_size};
use crate::streaming::issuer::{ReadIssuer, ReadRoute, ReadRoutes};
use crate::streaming::issuer_busy::IssuerBusyLog;
use crate::streaming::request::StreamResource;

/// Jobs the decode pool can hold at once: every read either resource can
/// have outstanding.
const DECODE_QUEUE_CAPACITY: usize = MAX_STREAM_PERMITS + LIGHTMAP_QUEUE_CAPACITY;

/// The level's decode pool before its issuer: routes are built over its
/// queue, then [`LevelReadIssuer::spawn`] takes both. The pool lives and
/// retires with the issuer, so it starts whenever an issuer does, whichever
/// resources it serves: every route decodes.
pub(crate) fn spawn_decode_pool() -> std::io::Result<(DecodePool, DecodeQueue)> {
    let pool = DecodePool::spawn(
        decode_pool_size(crate::parallelism::available_parallelism()),
        DECODE_QUEUE_CAPACITY,
    )?;
    let queue = pool.queue();
    Ok((pool, queue))
}

/// Owns the one issuer thread every streamed resource of a level reads
/// through, and the one decode pool both resources decode on. The issuer
/// only reads; its routes hand read bytes to the pool. SH's workers hold a
/// clone of the issuer handle to submit; lightmap submits through
/// [`Self::issuer`].
///
/// Teardown order: every clone must be dropped before this owner joins,
/// because the thread exits only when its last handle is gone. The session
/// drops SH (and its clone) before this owner; [`Self::begin_retirement`]
/// never joins.
#[derive(Debug)]
pub(crate) struct LevelReadIssuer {
    issuer: Option<ReadIssuer>,
    handle: Option<JoinHandle<()>>,
    decode: Option<DecodePool>,
}

impl LevelReadIssuer {
    /// Spawns one issuer over the routes present, beside `decode`, the pool
    /// their routes were built over. Its queue covers every submission the
    /// routed resources can have outstanding, so a submission never finds it
    /// full. `busy`, when present, receives the issuer's busy time per
    /// physical read.
    pub(crate) fn spawn(
        decode: DecodePool,
        sh: Option<Box<dyn ReadRoute>>,
        lightmap: Option<Box<dyn ReadRoute>>,
        busy: Option<Arc<IssuerBusyLog>>,
    ) -> std::io::Result<Self> {
        let mut routes = ReadRoutes::default().with_busy_log(busy);
        let mut queue_capacity = 0;
        if let Some(route) = sh {
            routes = routes.with(StreamResource::Sh, route);
            queue_capacity += MAX_STREAM_PERMITS;
        }
        if let Some(route) = lightmap {
            routes = routes.with(StreamResource::LightmapBlock, route);
            queue_capacity += LIGHTMAP_QUEUE_CAPACITY;
        }
        let (issuer, handle) = ReadIssuer::spawn(routes, queue_capacity)?;
        Ok(Self {
            issuer: Some(issuer),
            handle: Some(handle),
            decode: Some(decode),
        })
    }

    pub(crate) fn issuer(&self) -> &ReadIssuer {
        self.issuer
            .as_ref()
            .expect("the issuer handle lives until retirement")
    }

    /// Stops the issuer before its next read and drops this handle, and
    /// retires the decode pool: queued decodes are released, a running one
    /// releases rather than delivers. Waits on neither an in-flight
    /// positional read nor a running decode; the caller polls the returned
    /// threads and joins them only once finished.
    pub(crate) fn begin_retirement(mut self) -> Vec<JoinHandle<()>> {
        self.cancel_and_release();
        let mut threads = vec![self.handle.take().expect("spawned issuer has a thread")];
        if let Some(decode) = self.decode.take() {
            threads.extend(decode.begin_retirement());
        }
        threads
    }

    fn cancel_and_release(&mut self) {
        if let Some(issuer) = self.issuer.take() {
            issuer.cancel();
        }
    }
}

impl Drop for LevelReadIssuer {
    fn drop(&mut self) {
        self.cancel_and_release();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        drop(self.decode.take());
    }
}

/// A replaced level's cancelled I/O: SH worker threads, the issuer and decode
/// pool threads, the lightmap source its route and decodes may still read
/// through, and the completion queues they may still deliver into. The frame
/// path polls it and never joins a live positional read or decode; teardown
/// joins.
#[derive(Debug, Default)]
pub(crate) struct StreamingRetirement {
    sh: Vec<ShWorkerRetirement>,
    /// Issuer and decode pool threads.
    threads: Vec<JoinHandle<()>>,
    retained_lightmap: Vec<Arc<dyn LightmapBlockSource>>,
    /// A kept lightmap session's old completion queue, with the ledger its
    /// deliveries were charged to.
    lightmap_completions: Vec<(Receiver<LightmapCompletion>, Arc<LightmapRouteLedger>)>,
}

impl StreamingRetirement {
    pub(crate) fn add_sh(&mut self, retirement: ShWorkerRetirement) {
        self.sh.push(retirement);
    }

    /// The threads of a retiring [`LevelReadIssuer`].
    pub(crate) fn add_threads(&mut self, threads: Vec<JoinHandle<()>>) {
        self.threads.extend(threads);
    }

    pub(crate) fn retain_lightmap(&mut self, source: Arc<dyn LightmapBlockSource>) {
        self.retained_lightmap.push(source);
    }

    pub(crate) fn drain_lightmap_completions(
        &mut self,
        completions: Receiver<LightmapCompletion>,
        ledger: Arc<LightmapRouteLedger>,
    ) {
        self.lightmap_completions.push((completions, ledger));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.sh.is_empty()
            && self.threads.is_empty()
            && self.retained_lightmap.is_empty()
            && self.lightmap_completions.is_empty()
    }

    /// Joins and releases everything once every thread has finished. Every
    /// poll first empties the old lightmap completion queues: a retiring
    /// issuer blocked delivering into a full one could otherwise never see
    /// its cancel and finish.
    pub(crate) fn try_finish(&mut self) -> bool {
        self.sh.retain_mut(|retirement| !retirement.try_finish());
        self.release_lightmap_completions();
        if !self.sh.is_empty() || !self.threads.iter().all(JoinHandle::is_finished) {
            return false;
        }
        self.join_and_release();
        true
    }

    /// Releases every delivery waiting in the old lightmap completion
    /// queues, keeping the kept session's in-memory byte figure exact.
    fn release_lightmap_completions(&self) {
        for (completions, ledger) in &self.lightmap_completions {
            while let Ok(completion) = completions.try_recv() {
                ledger.release(completion.charged_bytes);
            }
        }
    }

    /// Releases the old completion queues, then joins the issuer and decode
    /// threads, then drops what they could still reach. A delivery into a
    /// dropped queue fails, wakes a blocked issuer, and releases its own
    /// bytes, so neither the join nor the byte figure waits on the frame.
    fn join_and_release(&mut self) {
        self.release_lightmap_completions();
        self.lightmap_completions.clear();
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
        self.retained_lightmap.clear();
    }
}

impl Drop for StreamingRetirement {
    fn drop(&mut self) {
        self.sh.clear();
        self.join_and_release();
    }
}
