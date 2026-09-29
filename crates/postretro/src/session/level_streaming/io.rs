//! The level's one read issuer and the retirement of a replaced level's I/O.
//! See: context/lib/rendering_pipeline.md §4

use std::sync::Arc;
use std::thread::JoinHandle;

use crate::lightmap_streaming::controller::MAX_LIGHTMAP_PERMITS;
use crate::lightmap_streaming::source::LightmapBlockSource;
use crate::session::sh_async_workers::ShWorkerRetirement;
use crate::sh_streaming::controller::MAX_STREAM_PERMITS;
use crate::streaming::issuer::{ReadIssuer, ReadRoute, ReadRoutes};
use crate::streaming::request::StreamResource;

/// Owns the one issuer thread every streamed resource of a level reads
/// through. SH's workers hold a clone of the handle to submit; lightmap
/// submits through [`Self::issuer`].
///
/// Teardown order: every clone must be dropped before this owner joins,
/// because the thread exits only when its last handle is gone. The session
/// drops SH (and its clone) before this owner; [`Self::begin_retirement`]
/// never joins.
#[derive(Debug)]
pub(crate) struct LevelReadIssuer {
    issuer: Option<ReadIssuer>,
    handle: Option<JoinHandle<()>>,
}

impl LevelReadIssuer {
    /// Spawns one issuer over the routes present. Its queue covers every
    /// routed resource's permits, so a submission never finds it full.
    pub(crate) fn spawn(
        sh: Option<Box<dyn ReadRoute>>,
        lightmap: Option<Box<dyn ReadRoute>>,
    ) -> std::io::Result<Self> {
        let mut routes = ReadRoutes::default();
        let mut queue_capacity = 0;
        if let Some(route) = sh {
            routes = routes.with(StreamResource::Sh, route);
            queue_capacity += MAX_STREAM_PERMITS;
        }
        if let Some(route) = lightmap {
            routes = routes.with(StreamResource::LightmapBlock, route);
            queue_capacity += MAX_LIGHTMAP_PERMITS;
        }
        let (issuer, handle) = ReadIssuer::spawn(routes, queue_capacity)?;
        Ok(Self {
            issuer: Some(issuer),
            handle: Some(handle),
        })
    }

    pub(crate) fn issuer(&self) -> &ReadIssuer {
        self.issuer
            .as_ref()
            .expect("the issuer handle lives until retirement")
    }

    /// Stops the thread before its next read and drops this handle, without
    /// waiting on an in-flight positional read. The caller polls the returned
    /// thread and joins it only once finished.
    pub(crate) fn begin_retirement(mut self) -> JoinHandle<()> {
        self.cancel_and_release();
        self.handle.take().expect("spawned issuer has a thread")
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
    }
}

/// A replaced level's cancelled I/O: SH worker threads, the issuer thread,
/// and the lightmap source its route may still read through. The frame path
/// polls it and never joins a live positional read; teardown joins.
#[derive(Debug, Default)]
pub(crate) struct StreamingRetirement {
    sh: Vec<ShWorkerRetirement>,
    issuers: Vec<JoinHandle<()>>,
    retained_lightmap: Vec<Arc<dyn LightmapBlockSource>>,
}

impl StreamingRetirement {
    pub(crate) fn add_sh(&mut self, retirement: ShWorkerRetirement) {
        self.sh.push(retirement);
    }

    pub(crate) fn add_issuer(&mut self, handle: JoinHandle<()>) {
        self.issuers.push(handle);
    }

    pub(crate) fn retain_lightmap(&mut self, source: Arc<dyn LightmapBlockSource>) {
        self.retained_lightmap.push(source);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.sh.is_empty() && self.issuers.is_empty() && self.retained_lightmap.is_empty()
    }

    /// Joins and releases everything once every thread has finished.
    pub(crate) fn try_finish(&mut self) -> bool {
        self.sh.retain_mut(|retirement| !retirement.try_finish());
        if !self.sh.is_empty() || !self.issuers.iter().all(JoinHandle::is_finished) {
            return false;
        }
        for handle in self.issuers.drain(..) {
            let _ = handle.join();
        }
        self.retained_lightmap.clear();
        true
    }
}

impl Drop for StreamingRetirement {
    fn drop(&mut self) {
        self.sh.clear();
        for handle in self.issuers.drain(..) {
            let _ = handle.join();
        }
        self.retained_lightmap.clear();
    }
}
