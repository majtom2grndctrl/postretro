// GPU creation counts: per-thread totals across the oracle install and the upload thread.
// See: context/lib/testing_guide.md §3 (GPU tests)

use super::*;
use crate::render::gpu_test_harness::{gpu_or_skip, offscreen_renderer_or_skip};
use crate::render::renderer_textures::tests::{chain, install_one_triangle, write_prm};
use crate::render::upload_thread::tests::wait_for;
use crate::render::upload_thread::{UploadContext, UploadThread};
use crate::render::{Material, TextureCacheKeysSection};
use postretro_level_format::prm::PrmFormat;

#[test]
fn since_is_per_kind_and_saturates() {
    let earlier = GpuCreationCounts {
        buffers: 3,
        textures: 1,
        bind_groups: 5,
        pipelines: 0,
    };
    let later = GpuCreationCounts {
        buffers: 7,
        textures: 0,
        bind_groups: 5,
        pipelines: 2,
    };
    let delta = later.since(earlier);
    assert_eq!(
        delta,
        GpuCreationCounts {
            buffers: 4,
            textures: 0,
            bind_groups: 0,
            pipelines: 2,
        }
    );
    assert_eq!(delta.total(), 6);
}

// The count source must read nonzero across the main-thread oracle's install,
// so a chokepoint that counts nothing fails here rather than passing a
// "commit creates nothing" row vacuously.
#[test]
fn creation_counter_counts_oracle_install() {
    let test = "creation_counter_counts_oracle_install";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let key = [0x11u8; 32];
    write_prm(
        dir.path(),
        key,
        [
            Some(chain(PrmFormat::Rgba8UnormSrgb, 4, 4, 0x5A)),
            None,
            None,
            None,
        ],
    );
    let names = vec!["baked".to_string()];
    let keys = TextureCacheKeysSection { keys: vec![key] };
    let materials = vec![Material::Default];

    let before = gpu_creation_counts();
    renderer.install_textures(&names, &keys, dir.path(), &materials);
    install_one_triangle(&mut renderer, &materials);
    let install = gpu_creation_counts().since(before);

    assert!(install.buffers > 0, "{install:?}");
    assert!(install.textures > 0, "{install:?}");
    assert!(install.bind_groups > 0, "{install:?}");
    assert!(install.pipelines > 0, "{install:?}");
}

#[test]
fn upload_thread_creations_count_on_the_upload_thread() {
    let test = "upload_thread_creations_count_on_the_upload_thread";
    let Some(ctx) = gpu_or_skip(test) else {
        return;
    };
    let thread = UploadThread::spawn(ctx.device.clone(), ctx.queue.clone()).unwrap();
    let before = gpu_creation_counts();
    let job = thread.submit(|context: &UploadContext| {
        let fresh = gpu_creation_counts();
        let _buffer = context.device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("upload thread counted buffer"),
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok((fresh, gpu_creation_counts()))
    });
    let (fresh, after_create) = wait_for(job).expect("the counting job reports");
    assert_eq!(
        fresh,
        GpuCreationCounts::default(),
        "a new upload thread starts at zero"
    );
    assert_eq!(
        after_create,
        GpuCreationCounts {
            buffers: 1,
            ..Default::default()
        },
        "the job reads its own creation on its own thread"
    );
    assert_eq!(
        gpu_creation_counts(),
        before,
        "an upload-thread creation leaves the test thread's counts unchanged"
    );
}
