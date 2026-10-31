// Install-time pipelines: built once per renderer on the upload thread, then shared by every level install.
// See: context/plans/in-progress/level-load-pipeline/index.md (Decisions: Pipelines once per renderer)

use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};

use super::animated_lightmap::AnimatedLightmapPipelines;
use super::billboard_direct_scatter_compose::BillboardDirectScatterComposePipelines;
use super::direct_sh_compose::DirectShComposePipelines;
use super::gpu_create::{GpuCreate, gpu_creation_counts};
use super::sh_compose::ShComposePipelines;
use super::sh_streaming::StreamingShPipelines;
use super::upload_thread::{UploadContext, UploadThread};
use crate::candidate_cull::CandidateCullPipelines;
use crate::compute_cull::ComputeCullPipelines;

/// The device-only half of every install-time pipeline family: bind-group
/// layouts and pipelines that depend only on the device, static WGSL and the
/// renderer-lifetime group-0 layout. Level halves hold clones of these
/// handles; no level install builds one.
pub(crate) struct InstallPipelines {
    pub(crate) sh_compose: ShComposePipelines,
    /// The direct-SH promotion pass and the section-45 animated-add pass.
    pub(crate) direct_sh_compose: DirectShComposePipelines,
    pub(crate) billboard_direct_scatter_compose: BillboardDirectScatterComposePipelines,
    pub(crate) animated_lightmap: AnimatedLightmapPipelines,
    pub(crate) sh_streaming: StreamingShPipelines,
    pub(crate) compute_cull: ComputeCullPipelines,
    pub(crate) candidate_cull: CandidateCullPipelines,
}

// Compile-time guard: built on the upload thread, read on the main thread
// and by later upload jobs.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<InstallPipelines>();
};

impl InstallPipelines {
    pub(crate) fn build(
        device: &wgpu::Device,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        Self {
            sh_compose: ShComposePipelines::new(device, uniform_bind_group_layout),
            direct_sh_compose: DirectShComposePipelines::new(device, uniform_bind_group_layout),
            billboard_direct_scatter_compose: BillboardDirectScatterComposePipelines::new(
                device,
                uniform_bind_group_layout,
            ),
            animated_lightmap: AnimatedLightmapPipelines::new(device, uniform_bind_group_layout),
            sh_streaming: StreamingShPipelines::new(device, uniform_bind_group_layout),
            compute_cull: ComputeCullPipelines::new(device),
            candidate_cull: CandidateCullPipelines::new(device),
        }
    }
}

/// Every install-family compute pipeline is created here, so a test can tell
/// on which thread the families were built.
pub(crate) fn new_install_compute_pipeline(
    device: &wgpu::Device,
    desc: &wgpu::ComputePipelineDescriptor,
) -> wgpu::ComputePipeline {
    #[cfg(test)]
    INSTALL_FAMILY_PIPELINES.with(|count| count.set(count.get() + 1));
    device.new_compute_pipeline(desc)
}

#[cfg(test)]
thread_local! {
    static INSTALL_FAMILY_PIPELINES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The calling thread's running total of install-family pipelines.
#[cfg(test)]
pub(crate) fn install_family_pipelines_built() -> u64 {
    INSTALL_FAMILY_PIPELINES.with(std::cell::Cell::get)
}

/// What the build job published.
struct Published {
    pipelines: Arc<InstallPipelines>,
    /// Pipelines the job created on the upload thread, by the creation
    /// chokepoint's count there. Read by tests.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by the slot's tests"))]
    pipelines_created_by_job: u64,
}

/// The renderer's one install-pipeline set, published once by the upload
/// thread's build job. Readers on any thread see it the moment it lands.
pub(crate) struct InstallPipelinesSlot {
    published: OnceLock<Result<Published, String>>,
    // Pairs with `published` only to wake the oracle's wait.
    wake_lock: Mutex<()>,
    wake: Condvar,
}

impl InstallPipelinesSlot {
    fn new() -> Self {
        Self {
            published: OnceLock::new(),
            wake_lock: Mutex::new(()),
            wake: Condvar::new(),
        }
    }

    /// The set, or `None` while it builds (or after a failed build). Never
    /// blocks. Upload jobs queued behind the build job always find it.
    pub(crate) fn try_get(&self) -> Option<Arc<InstallPipelines>> {
        match self.published.get() {
            Some(Ok(published)) => Some(Arc::clone(&published.pipelines)),
            _ => None,
        }
    }

    /// ORACLE ONLY: block the calling thread until the set is published.
    /// The main-thread `install_level_geometry` (the oracle, and today's
    /// production install) is its one caller; the staged commit path must
    /// never reach it (level-load-pipeline task 11 forbids it there by grep
    /// gate). In practice the build finished long before a first install.
    /// Panics if the build failed, as a pipeline validation error built on
    /// the main thread did before the build moved off it.
    pub(crate) fn wait_for_oracle(&self) -> Arc<InstallPipelines> {
        let mut guard = self
            .wake_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        loop {
            match self.published.get() {
                Some(Ok(published)) => return Arc::clone(&published.pipelines),
                Some(Err(error)) => panic!("install-time pipelines failed to build: {error}"),
                None => {
                    guard = self
                        .wake
                        .wait(guard)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
        }
    }

    /// Pipelines the build job created on the upload thread, once published.
    #[cfg(test)]
    pub(crate) fn pipelines_created_by_job(&self) -> Option<u64> {
        match self.published.get() {
            Some(Ok(published)) => Some(published.pipelines_created_by_job),
            _ => None,
        }
    }

    fn publish(&self, result: Result<Published, String>) {
        let _ = self.published.set(result);
        // Taking the lock orders this wake after a waiter's check.
        let _guard = self
            .wake_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.wake.notify_all();
    }
}

/// Publishes a failure if the build job is dropped unrun (its thread gone),
/// so the oracle's wait never hangs.
struct Publisher {
    slot: Arc<InstallPipelinesSlot>,
    done: bool,
}

impl Publisher {
    fn publish(mut self, result: Result<Published, String>) {
        self.done = true;
        self.slot.publish(result);
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        if !self.done {
            self.slot
                .publish(Err("the upload thread ended before the build ran".into()));
        }
    }
}

/// Queue the renderer's one install-pipeline build on its upload thread and
/// return the slot it publishes into. Upload jobs run in submission order,
/// so every later job finds the set ready without the main thread waiting.
pub(crate) fn submit_install_pipeline_build(
    upload_thread: &UploadThread,
    uniform_bind_group_layout: wgpu::BindGroupLayout,
    gate: Option<PipelineBuildGate>,
) -> Arc<InstallPipelinesSlot> {
    let slot = Arc::new(InstallPipelinesSlot::new());
    let publisher = Publisher {
        slot: Arc::clone(&slot),
        done: false,
    };
    upload_thread.submit_then(
        move |context: &UploadContext| {
            if let Some(gate) = gate {
                gate.wait_open();
            }
            let before = gpu_creation_counts();
            let pipelines = InstallPipelines::build(&context.device, &uniform_bind_group_layout);
            Ok(Published {
                pipelines: Arc::new(pipelines),
                pipelines_created_by_job: gpu_creation_counts().since(before).pipelines,
            })
        },
        move |result| {
            let result = result.map_err(|error| error.to_string());
            if let Err(error) = &result {
                log::error!("[Renderer] install-time pipeline build failed: {error}");
            }
            publisher.publish(result);
        },
    );
    slot
}

/// A test-held gate the build job waits on before building.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct PipelineBuildGate(Arc<(Mutex<bool>, Condvar)>);

#[cfg(test)]
impl PipelineBuildGate {
    fn wait_open(&self) {
        let (open, wake) = &*self.0;
        let mut open = open.lock().unwrap_or_else(PoisonError::into_inner);
        while !*open {
            open = wake.wait(open).unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn open(&self) {
        let (open, wake) = &*self.0;
        *open.lock().unwrap_or_else(PoisonError::into_inner) = true;
        wake.notify_all();
    }

    /// The handle that releases this gate, opened on `release` or drop.
    pub(crate) fn release_handle(&self) -> PipelineBuildRelease {
        PipelineBuildRelease(self.clone())
    }
}

/// Production builds carry no gate: this type has no values.
#[cfg(not(test))]
pub(crate) enum PipelineBuildGate {}

#[cfg(not(test))]
impl PipelineBuildGate {
    fn wait_open(&self) {
        match *self {}
    }
}

/// Releases a held pipeline build; dropping it releases too, so a failed
/// test never leaves the upload thread parked.
#[cfg(test)]
pub(crate) struct PipelineBuildRelease(PipelineBuildGate);

#[cfg(test)]
impl PipelineBuildRelease {
    pub(crate) fn release(self) {}
}

#[cfg(test)]
impl Drop for PipelineBuildRelease {
    fn drop(&mut self) {
        self.0.open();
    }
}

#[cfg(test)]
impl super::Renderer {
    /// Block until this renderer's install pipelines are published. Tests
    /// that construct a family directly read the set through this.
    pub(crate) fn wait_install_pipelines_for_test(&self) -> Arc<InstallPipelines> {
        self.full().install_pipelines.wait_for_oracle()
    }
}

#[cfg(test)]
#[path = "install_pipelines_tests.rs"]
mod tests;
