#[test]
fn planar_shaders_preserve_ip_boundaries() {
    for shader in [
        include_str!("../../shaders/planar_mask.wgsl"),
        include_str!("../../shaders/planar_resolve.wgsl"),
    ] {
        let lower = shader.to_lowercase();
        assert!(!lower.contains("fresnel"));
        assert!(!lower.contains("fluid_probe"));
        assert!(!lower.contains("dot("));
    }
}

use super::*;
use crate::render::fluid_reflection_frame_test_fixtures as fixture;
use crate::render::fluid_surface_frame_test::look_draw;
use glam::{UVec2, Vec4};
use postretro_level_format::cell_draw_index::{CellDrawIndexSection, Span};
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::planar_reflection::PlanarCandidates;

fn planar_fixture() -> Option<(Renderer, fixture::View, PlanarCandidates, MirrorCamera)> {
    let mut renderer = fixture::ssr_renderer()?;
    let quad = fixture::upright(
        [-1.4, -0.2],
        [0.0, 1.5],
        4.0,
        fixture::standing_quad().base_uv,
    );
    fixture::install_world_quads(&mut renderer, &[quad, fixture::back_wall()]);
    renderer.full_mut().cell_draw_index = Some(CellDrawIndexSection {
        cell_count: 1,
        span_count: 1,
        cell_span_offset: vec![0, 1],
        spans: vec![Span {
            leaf_start: 0,
            leaf_count: 1,
        }],
    });
    renderer.install_fluid_surfaces(
        &[fixture::base_pool()],
        Some(&fixture::green_probe(&[(0, 0)])),
    );
    renderer.set_fluid_surface_draws(&[look_draw(0, fixture::mirror())]);
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    let candidates = PlanarCandidates::from_volumes(&[fixture::base_pool()], &[0]);
    let view = fixture::base_view();
    let projection =
        glam::camera::rh::proj::directx::perspective(60.0_f32.to_radians(), 1.0, 0.1, 100.0);
    let main_view = projection.inverse() * view.view_proj;
    let mirror = MirrorCamera::new(view.eye, main_view, projection, 0.0, None).unwrap();
    renderer.set_planar_reflection_settings(PlanarReflectionSettings {
        on: true,
        scale: 1.0,
        ..Default::default()
    });
    renderer.set_planar_reflection_view(
        Some(mirror),
        Some(&candidates.planes[0]),
        &VisibleCells::Culled(vec![0]),
    );
    Some((renderer, view, candidates, mirror))
}

fn read_planar(renderer: &Renderer) -> Vec<Vec4> {
    use postretro_render_cpu::sh_compose::f16_bits_to_f32;
    let texture = renderer
        .full()
        .planar_reflection
        .targets
        .as_ref()
        .unwrap()
        .resolved
        .texture();
    let size = texture.size();
    let row = size.width * 8;
    let padded = row.div_ceil(256) * 256;
    let buffer = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Planar Test Readback"),
        size: u64::from(padded * size.height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(size.height),
            },
        },
        size,
    );
    renderer
        .queue
        .submit_unbatched([encoder.finish()], "planar test readback");
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).unwrap();
    });
    renderer
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    rx.recv().unwrap().unwrap();
    let mapped = buffer.slice(..).get_mapped_range().unwrap();
    let mut pixels = Vec::new();
    for y in 0..size.height {
        let start = (y * padded) as usize;
        for texel in mapped[start..start + row as usize].as_chunks::<8>().0 {
            let f = |i| f16_bits_to_f32(u16::from_le_bytes([texel[i], texel[i + 1]]));
            pixels.push(Vec4::new(f(0), f(2), f(4), f(6)));
        }
    }
    drop(mapped);
    buffer.unmap();
    pixels
}

#[test]
fn planar_gpu_mask_rejects_dry_occluded_and_beyond_r_pixels() {
    let Some((mut renderer, view, _, _)) = planar_fixture() else {
        return;
    };
    let mut settings = renderer.planar_reflection_settings();
    settings.r = Some(6.0);
    settings.band_r = 1.0;
    renderer.set_planar_reflection_settings(settings);
    fixture::capture_checked(&mut renderer, view);
    let reflected = read_planar(&renderer);
    let scene = fixture::read_scene(&renderer);
    let mut dry = 0;
    let mut far = 0;
    let mut wet = 0;
    let mut occluded = 0;
    for y in 0..fixture::FRAME {
        for x in 0..fixture::FRAME {
            let pixel = UVec2::new(x, y);
            let sample = reflected[(y * fixture::FRAME + x) as usize];
            let water = fixture::on_plane(view, pixel.as_vec2() + 0.5, 0.0);
            match water {
                Some(water) if fixture::on_pool(water, 0.01) => {
                    let (_, water_depth) = fixture::project(view, water);
                    if water_depth > scene.depth_at(pixel) + 0.00001 {
                        assert_eq!(sample.w, 0.0);
                        occluded += 1;
                    } else if water.distance(view.eye) >= 6.02 {
                        assert_eq!(sample.w, 0.0);
                        far += 1;
                    } else if sample.w > 0.0 {
                        wet += 1;
                    }
                }
                Some(water) if fixture::on_pool(water, -0.03) => {}
                _ => {
                    assert_eq!(sample.w, 0.0, "dry pixel {pixel:?}");
                    dry += 1;
                }
            }
        }
    }
    assert!(
        dry > 20 && far > 20 && wet > 20 && occluded > 20,
        "dry={dry}, far={far}, wet={wet}, occluded={occluded}"
    );
}

#[test]
fn planar_gpu_asymmetric_world_lands_on_mirrored_pixel_and_ssr_records_zero() {
    let Some((mut renderer, view, _, _)) = planar_fixture() else {
        return;
    };
    let before = renderer.full().fluid_surface.reflection_pass_records();
    fixture::capture_checked(&mut renderer, view);
    assert_eq!(
        renderer.full().fluid_surface.reflection_pass_records(),
        before
    );
    let reflected = read_planar(&renderer);
    let (at, _) = fixture::project(view, Vec3::new(-0.8, -0.75, 4.0));
    let pixel = at.as_uvec2();
    let sample = reflected[(pixel.y * fixture::FRAME + pixel.x) as usize];
    assert!(
        sample.w > 0.95 && sample.x > 0.01 && sample.z > 0.01 && sample.y < 0.01,
        "expected magenta at {pixel:?}, got {sample:?}"
    );
    let other = UVec2::new(fixture::FRAME - 1 - pixel.x, pixel.y);
    let opposite = reflected[(other.y * fixture::FRAME + other.x) as usize];
    assert!(
        opposite.x < 0.01,
        "asymmetric reflection was flipped twice: {opposite:?}"
    );
    // Low keeps the planar override even though SSR has no screen target there.
    renderer.set_reflection_quality(FluidReflectionQuality::Low);
    fixture::capture_checked(&mut renderer, view);
    assert_eq!(
        renderer.full().fluid_surface.reflection_pass_records(),
        before
    );
    assert!(read_planar(&renderer)[(pixel.y * fixture::FRAME + pixel.x) as usize].w > 0.95);
}

#[test]
fn planar_gpu_no_selected_plane_clears_previous_confidence() {
    let Some((mut renderer, view, _, _)) = planar_fixture() else {
        return;
    };
    fixture::capture_checked(&mut renderer, view);
    assert!(read_planar(&renderer).iter().any(|p| p.w > 0.0));
    renderer.set_planar_reflection_view(None, None, &VisibleCells::Culled(Vec::new()));
    fixture::capture_checked(&mut renderer, view);
    assert!(read_planar(&renderer).iter().all(|p| *p == Vec4::ZERO));
}

// Regression: a fluid reload could change mask geometry while keeping its old cache key.
#[test]
fn planar_gpu_changed_faces_with_same_plane_bounds_refresh_the_water_mask() {
    let Some((mut renderer, view, candidates, camera)) = planar_fixture() else {
        return;
    };
    fixture::capture_checked(&mut renderer, view);
    let before = read_planar(&renderer);
    let mut replacement = candidates.planes[0].clone();
    replacement.faces[0].vertices.remove(1);
    renderer.set_planar_reflection_view(
        Some(camera),
        Some(&replacement),
        &VisibleCells::Culled(vec![0]),
    );
    fixture::capture_checked(&mut renderer, view);
    let after = read_planar(&renderer);
    assert!(
        before
            .iter()
            .zip(&after)
            .any(|(old, new)| old.w > 0.0 && new.w == 0.0)
    );
    assert!(after.iter().any(|pixel| pixel.w > 0.0));
}

#[test]
fn planar_shaders_validate_without_an_adapter() {
    for source in [
        include_str!("../../shaders/planar_mask.wgsl"),
        include_str!("../../shaders/planar_resolve.wgsl"),
    ] {
        let module = naga::front::wgsl::parse_str(source).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }
}

#[test]
fn planar_gpu_d_distance_fades_and_disabling_restores_ssr_bytes() {
    let Some((mut renderer, view, _, _)) = planar_fixture() else {
        return;
    };
    fixture::capture_checked(&mut renderer, view);
    let mut settings = renderer.planar_reflection_settings();
    settings.d = Some(3.0);
    settings.band_d = 1.0;
    renderer.set_planar_reflection_settings(settings);
    fixture::capture_checked(&mut renderer, view);
    assert!(
        read_planar(&renderer).iter().all(|p| *p == Vec4::ZERO),
        "all reflected geometry is beyond D=3m"
    );
    settings.on = false;
    renderer.set_planar_reflection_settings(settings);
    let ssr = fixture::capture_checked(&mut renderer, view);
    settings.on = true;
    renderer.set_planar_reflection_settings(settings);
    renderer.set_planar_reflection_view(None, None, &VisibleCells::Culled(Vec::new()));
    fixture::capture_checked(&mut renderer, view);
    settings.on = false;
    renderer.set_planar_reflection_settings(settings);
    assert_eq!(fixture::capture_checked(&mut renderer, view), ssr);
}

#[test]
fn planar_gpu_other_water_plane_remains_probe_only() {
    let Some((mut renderer, view, candidates, camera)) = planar_fixture() else {
        return;
    };
    renderer.set_planar_reflection_settings(PlanarReflectionSettings::default());
    renderer.install_fluid_surfaces(
        &[
            fixture::base_pool(),
            fixture::pool(1, 1.0, &fixture::POOL_X),
        ],
        Some(&fixture::green_probe(&[(0, 0), (1, 0)])),
    );
    renderer.set_fluid_surface_draws(&[look_draw(1, fixture::mirror())]);
    renderer.set_reflection_quality(FluidReflectionQuality::Low);
    let probe_only = fixture::capture_checked(&mut renderer, view);
    renderer.set_planar_reflection_settings(PlanarReflectionSettings {
        on: true,
        scale: 1.0,
        ..Default::default()
    });
    renderer.set_planar_reflection_view(
        Some(camera),
        Some(&candidates.planes[0]),
        &VisibleCells::Culled(vec![0]),
    );
    let planar = fixture::capture_checked(&mut renderer, view);
    assert!(read_planar(&renderer).iter().any(|p| p.w > 0.0));
    assert_eq!(
        planar, probe_only,
        "an upper water plane must not sample the selected lower plane's screen layer"
    );
}
