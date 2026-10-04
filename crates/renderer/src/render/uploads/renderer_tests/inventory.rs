//! Real-map writer inventory through the production renderer.
use super::*;

#[path = "inventory/fixture.rs"]
mod fixture;
use fixture::{add_sdf_inventory_fixture, promotion_receiver_pose};

// Regression: a world-less Surface Depth change left bytes pending at reload commit.
#[test]
#[cfg(debug_assertions)]
fn frontend_surface_depth_uploads_flush_before_the_next_manifest_commit() {
    use postretro_render_cpu::surface_depth::SurfaceDepthQuality;
    let Some(mut renderer) = renderer() else {
        return;
    };
    assert!(
        !renderer.full().gpu_textures.is_empty(),
        "world-less renderer retains a placeholder material"
    );
    let mut font = postretro_ui::text::build_font_system();
    for quality in [SurfaceDepthQuality::Off, SurfaceDepthQuality::On] {
        // Frontend polls reload before applying this frame's option changes.
        renderer.set_presentation_templates(vec![]);
        renderer.set_bloom_render_profile(renderer.bloom_render_profile());
        let before = renderer.queue.counts();
        renderer.set_surface_depth_quality(quality);
        assert_eq!(renderer.surface_depth_quality(), quality);
        assert_eq!(
            renderer.queue.counts().writes - before.writes,
            renderer.full().gpu_textures.len() as u64,
            "the actual option setter stages every retained material"
        );
        for templates in [true, false] {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if templates {
                    renderer.set_presentation_templates(vec![]);
                } else {
                    renderer.set_bloom_render_profile(renderer.bloom_render_profile());
                }
            }));
            assert!(
                result.is_err(),
                "reload must reject unsubmitted option bytes"
            );
        }
        // Actual world-less scene submission completes the option's frame.
        let encoder = record_window(&mut renderer, &mut font);
        renderer.submit_windowed_frame(encoder);
        assert_submit(&renderer, before, 1);
        renderer.set_presentation_templates(vec![]);
        renderer.set_bloom_render_profile(renderer.bloom_render_profile());
        renderer.queue.assert_empty("next frontend manifest commit");
    }
    eprintln!("[UploadProof] frontend Surface Depth/reload boundary: 2 adapter cases ran");
}

/// Generated PRL is deliberately ignored: run after compiling campaign-test with
/// POSTRETRO_SH_STREAMING=off POSTRETRO_LIGHTMAP_STREAMING=all-resident.
/// This covers the legacy real-map paths; streamed compose inventory requires
/// the separate residency integration fixtures.
#[test]
#[ignore = "requires compiled campaign-test.prl and all-resident loader modes"]
fn real_map_steady_state_stages_binary_prewrites_and_window_only_writers() {
    use postretro_render_cpu::mesh_instances::{MeshInstanceInput, MeshPaletteCacheKey};
    use postretro_ui::descriptor::{
        AnchoredTree, CaptureMode, ColorValue, PanelWidget, RingWidget, ScalarValue, TextWidget,
        Widget,
    };
    use postretro_ui::layout::Anchor;
    let Some(mut renderer) = renderer() else {
        return;
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
    let map = root.join("maps/campaign-test.prl");
    assert!(
        map.is_file(),
        "build campaign-test.prl before running this ignored proof"
    );
    let mut world = postretro_level_loader::load_prl(map.to_str().unwrap()).unwrap();
    add_sdf_inventory_fixture(&mut world);
    assert!(
        world.sh_stream_manifest().is_none(),
        "run with POSTRETRO_SH_STREAMING=off"
    );
    assert!(
        world.lightmap_stream_manifest().is_none(),
        "run with POSTRETRO_LIGHTMAP_STREAMING=all-resident"
    );
    let materials: Vec<_> = world
        .texture_names
        .iter()
        .map(|_| postretro_render_data::material::Material::Default)
        .collect();
    renderer.install_textures(
        &world.texture_names,
        &world.texture_cache_keys,
        &root.join("cache"),
        &materials,
    );
    renderer.normalize_world_uvs(&mut world);
    let lighting = world.take_gpu_lighting_payloads();
    renderer.install_level_geometry(
        &crate::render::level_world_to_geometry(&world, &materials),
        lighting,
    );
    assert!(
        renderer.full().sdf_atlas_resources.present,
        "normal install must upload the valid SDF fixture"
    );
    assert!(
        renderer.animated_compose_descriptor_count() > 0,
        "fixture must contain baked animated lights"
    );
    assert!(!world.fog_volumes.is_empty(), "fixture must contain fog");
    assert!(
        !world.kinematic_geometry.movers.is_empty(),
        "fixture must contain movers"
    );
    let model = "models/pose-modifier-fixture/joint_zones.gltf";
    assert!(
        renderer
            .load_skinned_model(model, &root, &root.join("cache"))
            .is_some()
    );
    renderer.register_smoke_collection(
        "upload-inventory",
        "smoke_puff",
        &root.join("textures"),
        &root.join("cache"),
        crate::render::SpriteCollectionRegistration {
            baked_sidecar_eligible: false,
            spec_intensity: 0.0,
            spec_exponent: 16.0,
            lifetime: 1.0,
            emissive: 0.0,
        },
    );
    let (promotion_selection, receiver_position, eye) = promotion_receiver_pose(&renderer);
    renderer.set_mesh_draws(&[false, true].map(|is_viewmodel| MeshInstanceInput {
        model: postretro_model::ModelHandle::from(model),
        transform: if is_viewmodel {
            Mat4::IDENTITY
        } else {
            Mat4::from_translation(receiver_position)
        },
        shadow_bias_scale: 1.0,
        phase_seed: u32::from(is_viewmodel),
        palette_cache_key: MeshPaletteCacheKey::Entity(u32::from(is_viewmodel)),
        sample: postretro_model::sample_params::MeshSampleParams::rest(),
        pose_inputs: None,
        capture: None,
        resample: true,
        forward_visible: true,
        dynamic_shadow_visible: true,
        is_viewmodel,
    }));
    let movers: Vec<_> = world
        .kinematic_geometry
        .movers
        .iter()
        .map(
            |mover| crate::render::kinematic_brush::KinematicMoverInstance {
                mover_id: mover.mover_id,
                transform: Mat4::from_translation(mover.origin),
            },
        )
        .collect();
    let particles = [0.0f32, 0.0, 0.0, 0.25, 1.0, 0.0, 1.0, 0.0];
    let fog: Vec<_> = world
        .fog_volumes
        .iter()
        .map(|volume| postretro_render_cpu::fog_volume::FogVolume {
            min: volume.min,
            density: volume.density,
            max_v: volume.max,
            edge_softness: volume.edge_softness,
            center: volume.center,
            half_diag: volume.half_diag,
            inv_half_ext: volume.inv_half_ext,
            shape_mode: volume.shape_mode,
            tint: volume.tint,
            saturation: volume.saturation,
            radial_falloff: volume.radial_falloff,
            glow: volume.glow,
            plane_offset: 0,
            plane_count: volume.plane_count,
            min_brightness: volume.min_brightness,
            light_range: volume.light_range,
            anisotropy: volume.anisotropy,
            ambient_scatter: volume.ambient_scatter,
        })
        .collect();
    let planes: Vec<_> = world
        .fog_volumes
        .iter()
        .map(|volume| volume.planes.clone())
        .collect();
    let aabbs: Vec<_> = world
        .fog_volumes
        .iter()
        .map(|volume| (Vec3::from_array(volume.min), Vec3::from_array(volume.max)))
        .collect();
    let point = postretro_render_cpu::fog_volume::FogPointLight {
        position: [0.0; 3],
        range: 4.0,
        color: [1.0; 3],
        _pad: 0.0,
    };
    let roots = [
        Widget::Text(TextWidget {
            content: "UPLOAD".into(),
            font_size: 48.0,
            color: ColorValue::Literal([1.0; 4]),
            font: None,
            bind: None,
            style_ranges: None,
            id: None,
            focus_neighbors: Default::default(),
            visible_when: None,
            role: None,
        }),
        Widget::Panel(PanelWidget {
            fill: ColorValue::Literal([0.2, 0.1, 0.3, 0.4]),
            border: None,
            id: None,
            focus_neighbors: Default::default(),
            bind: None,
            style_ranges: None,
            visible_when: None,
            role: None,
        }),
        Widget::Ring(RingWidget {
            diameter: 320.0,
            radius: ScalarValue::Literal(120.0),
            radius_range: None,
            thickness: ScalarValue::Literal(16.0),
            start_angle: None,
            sweep: None,
            fill: ColorValue::Literal([1.0; 4]),
            track: None,
            id: None,
            visible_when: None,
            role: None,
        }),
    ];
    renderer.set_ui_snapshot(postretro_ui::UiReadSnapshot::with_trees(
        roots
            .into_iter()
            .enumerate()
            .map(|(index, root)| {
                let tree = AnchoredTree {
                    anchor: Anchor::TopLeft,
                    offset: [0.0, 0.0],
                    root,
                    capture_mode: CaptureMode::Passthrough,
                    initial_focus: None,
                    text_entry_target: None,
                    accessible_name: None,
                    role: None,
                };
                postretro_ui::UiTreeEntry {
                    name: format!("upload-{index}"),
                    tier: postretro_ui::modal_stack::ScopeTier::Engine,
                    capture_mode: tree.capture_mode,
                    descriptor: tree,
                    on_commit: None,
                }
            })
            .collect(),
        Default::default(),
        Default::default(),
        1.0,
        None,
    ));
    let mut font = postretro_ui::text::build_font_system();
    let view = Mat4::look_at_rh(eye, receiver_position, Vec3::Y);
    let vp = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.1, 4096.0) * view;
    // Warm retained meshes, glyphs, target pipelines and pool before counting.
    let visible = VisibleCells::Culled((0..world.cells.len() as u32).collect());
    let light_reachable = vec![true; world.cells.len()];
    let reachable_aabbs: Vec<_> = world
        .cells
        .iter()
        .map(|cell| (cell.bounds_min, cell.bounds_max))
        .collect();
    let mut baseline = Vec::new();
    for frame in 0..4 {
        if frame == 2 {
            baseline = renderer
                .queue
                .writer_counts()
                .iter()
                .map(|writer| (writer.site.file(), writer.site.line(), writer.writes))
                .collect();
        }
        let before = renderer.queue.counts();
        renderer.full_mut().mesh_draws[1].resample = false;
        let full = renderer.full();
        let mut lights = postretro_lighting::pack_lights(&full.level_lights);
        lights.resize(
            lights.len() + full.animated_baked_light_count * postretro_lighting::GPU_LIGHT_SIZE,
            0,
        );
        let record_count = full.level_lights.len() + full.animated_baked_light_count;
        let influence = vec![0u8; record_count * 16];
        let descriptors =
            vec![0u8; record_count * postretro_render_cpu::sh_volume::ANIMATION_DESCRIPTOR_SIZE];
        let samples =
            vec![
                0u8;
                record_count * postretro_render_cpu::sh_volume::SCRIPTED_FLOATS_PER_LIGHT * 4
            ];
        let brightness = vec![1.0; full.level_lights.len()];
        let window_brightness = vec![1.0; full.animated_baked_light_count];
        assert!(renderer.upload_light_bridge_snapshot(
            &lights,
            &influence,
            &descriptors,
            &samples,
            &brightness,
            &window_brightness,
            &[]
        ));
        renderer.upload_fog_volumes(bytemuck::cast_slice(&fog), &planes, u32::MAX);
        renderer.set_fog_aabbs(&aabbs);
        renderer.upload_fog_points(bytemuck::bytes_of(&point));
        renderer.set_animated_light_active(0, false);
        renderer.set_animated_light_active(0, true);
        renderer.update_per_frame_uniforms(vp, eye, frame as f32 + 1.0);
        #[cfg(feature = "dev-tools")]
        {
            renderer.set_direct_sh_debug_override(crate::render::DirectShDebugOverride {
                enabled: frame % 2 != 0,
                selection_index: 0,
                weight: 0.5,
            });
            renderer.set_animated_direct_sh_debug_override(
                crate::render::AnimatedDirectShDebugOverride {
                    enabled: frame % 2 != 0,
                    light_index: 0,
                },
            );
        }
        renderer.update_viewmodel_view_projection(1.0, view);
        renderer.set_kinematic_mover_draws(&movers, &movers);
        let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("real map offscreen window branch"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: renderer.surface_config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let output_view = output.create_view(&Default::default());
        let mut encoder = renderer.device.create_command_encoder(&Default::default());
        renderer
            .record_scene_passes(
                &mut encoder,
                Some(&mut font),
                Some(&output_view),
                if frame % 2 == 0 {
                    camera()
                } else {
                    CameraCullVisibility {
                        cells: &visible,
                        path: VisibilityPath::PrlPortal {
                            walk_reach: world.cells.len() as u32,
                        },
                    }
                },
                &light_reachable,
                &reachable_aabbs,
                &[],
                regions(),
                None,
                vp,
                &[("upload-inventory", bytemuck::cast_slice(&particles))],
                &[],
                frame as f64 + 1.0,
                CLEAR,
                true,
            )
            .unwrap();
        assert!(
            renderer.full().promoted_baked_records.iter().any(|record| {
                record.source.selected_static_index() == Some(promotion_selection)
            }),
            "frame {frame} must promote the authored light overlapping the world mesh"
        );
        renderer.submit_windowed_frame(encoder);
        assert_submit(&renderer, before, 1);
    }
    for writer in renderer.queue.writer_counts().iter() {
        let delta = writer.writes
            - baseline
                .iter()
                .find(|(path, line, _)| *path == writer.site.file() && *line == writer.site.line())
                .map_or(0, |(_, _, count)| *count);
        if delta > 0 {
            eprintln!(
                "[UploadInventory] {}:{} staged={delta}",
                writer.site.file(),
                writer.site.line()
            );
        }
    }
    let counts = renderer.queue.counts();
    eprintln!(
        "[UploadInventoryTotals] writes={} copies={} bytes={} batches={} max_batch_bytes={} created={} live={}",
        counts.writes,
        counts.copies,
        counts.bytes,
        counts.batches,
        renderer.queue.window_max_batch_bytes.get(),
        renderer.queue.pool_counts().1,
        renderer.queue.pool_counts().2
    );
    let expected = [
        (
            "renderer_lighting.rs",
            include_str!("../../renderer_lighting.rs"),
            "&full.lights_buffer",
            0,
        ),
        (
            "renderer_lighting.rs",
            include_str!("../../renderer_lighting.rs"),
            "&full.influence_buffer",
            0,
        ),
        (
            "renderer_lighting.rs",
            include_str!("../../renderer_lighting.rs"),
            "&full.sh_volume_resources.scripted_light_descriptors",
            0,
        ),
        (
            "renderer_lighting.rs",
            include_str!("../../renderer_lighting.rs"),
            "&full.sh_volume_resources.animation.anim_samples",
            0,
        ),
        (
            "renderer_frame.rs",
            include_str!("../../renderer_frame.rs"),
            "&full.uniform_buffer",
            0,
        ),
        (
            "direct_sh_resources.rs",
            include_str!("../../direct_sh_resources.rs"),
            "&self.dynamic_direct_params_buffer",
            0, // write_dynamic_direct_params; occurrence 1 is installation-only.
        ),
        (
            "sh_volume.rs",
            include_str!("../../sh_volume.rs"),
            "&self.descriptors",
            0,
        ),
        (
            "mesh_pass.rs",
            include_str!("../../mesh_pass.rs"),
            "&self.viewmodel_uniform_buffer",
            0,
        ),
        (
            "mesh_pass.rs",
            include_str!("../../mesh_pass.rs"),
            "instance_buffer,",
            0,
        ),
        (
            "mesh_pass.rs",
            include_str!("../../mesh_pass.rs"),
            "bytemuck::cast_slice(scratch)",
            0,
        ),
        (
            "mesh_pass.rs",
            include_str!("../../mesh_pass.rs"),
            "bytemuck::cast_slice(cached)",
            0,
        ),
        (
            "mesh_pass.rs",
            include_str!("../../mesh_pass.rs"),
            "&self.light_params_buffer",
            0,
        ),
        (
            "renderer_light_slots.rs",
            include_str!("../../renderer_light_slots.rs"),
            "&full.lights_buffer",
            1,
        ),
        (
            "renderer_light_slots.rs",
            include_str!("../../renderer_light_slots.rs"),
            "&full.influence_buffer",
            0,
        ),
        (
            "renderer_light_slots.rs",
            include_str!("../../renderer_light_slots.rs"),
            "&full.promoted_static_weight_buffer",
            0,
        ),
        (
            "renderer_light_slots.rs",
            include_str!("../../renderer_light_slots.rs"),
            "&full.uniform_buffer",
            1,
        ),
        (
            "renderer_light_slots.rs",
            include_str!("../../renderer_light_slots.rs"),
            "&full.spot_shadow_pool.matrices_buffer",
            0,
        ),
        (
            "renderer_light_slots.rs",
            include_str!("../../renderer_light_slots.rs"),
            "&full.shadow_vs_uniform_buffer",
            0,
        ),
        (
            "renderer_light_slots.rs",
            include_str!("../../renderer_light_slots.rs"),
            "&full.cube_shadow_vs_uniform_buffer",
            0,
        ),
        (
            "compute_cull.rs",
            include_str!("../../../compute_cull.rs"),
            "&self.visible_cells_buffer",
            0,
        ),
        (
            "compute_cull.rs",
            include_str!("../../../compute_cull.rs"),
            "&self.uniform_buffer",
            0,
        ),
        (
            "candidate_cull.rs",
            include_str!("../../../candidate_cull.rs"),
            "&self.uniform_buffer",
            0,
        ),
        (
            "candidate_cull.rs",
            include_str!("../../../candidate_cull.rs"),
            "&self.params_buffer",
            0,
        ),
        (
            "candidate_cull.rs",
            include_str!("../../../candidate_cull.rs"),
            "&self.candidate_buffer",
            0,
        ),
        (
            "fog_pass.rs",
            include_str!("../../fog_pass.rs"),
            "&self.volumes_buffer",
            0,
        ),
        (
            "fog_pass.rs",
            include_str!("../../fog_pass.rs"),
            "&self.fog_planes_buffer",
            0,
        ),
        (
            "fog_pass.rs",
            include_str!("../../fog_pass.rs"),
            "&self.params_buffer",
            0,
        ),
        (
            "fog_pass.rs",
            include_str!("../../fog_pass.rs"),
            "&self.fog_points_buffer",
            0,
        ),
        (
            "fog_pass.rs",
            include_str!("../../fog_pass.rs"),
            "&self.spots_buffer",
            0,
        ),
        (
            "kinematic_brush.rs",
            include_str!("../../kinematic_brush.rs"),
            "&self.instance_buffer",
            0,
        ),
        (
            "kinematic_brush.rs",
            include_str!("../../kinematic_brush.rs"),
            "&self.light_params_buffer",
            0,
        ),
        (
            "smoke.rs",
            include_str!("../../smoke.rs"),
            "placement.offset as u64",
            0,
        ),
        (
            "ui/mod.rs",
            include_str!("../../ui/mod.rs"),
            "&self.uniform_buffer",
            0,
        ),
        (
            "ui/mod.rs",
            include_str!("../../ui/mod.rs"),
            "&self.instance_buffer",
            0,
        ),
        (
            "ui/mod.rs",
            include_str!("../../ui/mod.rs"),
            "&self.ring_instance_buffer",
            0,
        ),
        (
            "screen_effects.rs",
            include_str!("../../screen_effects.rs"),
            "&self.effect_buffer",
            0,
        ),
        (
            "sdf_shadow.rs",
            include_str!("../../sdf_shadow.rs"),
            "&self.params_buffer",
            0,
        ),
        (
            "animated_lightmap.rs",
            include_str!("../../animated_lightmap.rs"),
            "&state.dispatch_tiles_buffer",
            0,
        ),
    ];
    for (file, source, target, occurrence) in expected {
        require_site(&renderer, &baseline, file, source, target, occurrence);
    }
    // Direct compose's always-active mask, plus dev-tools diff-gated overrides.
    require_site(
        &renderer,
        &baseline,
        "direct_sh_compose.rs",
        include_str!("../../direct_sh_compose.rs"),
        "&pipeline.light_term_mask_buffer",
        0,
    );
    #[cfg(feature = "dev-tools")]
    for target in [
        "&pipeline.debug_override_buffer",
        "&animated_add.animated_light_scale_buffer",
    ] {
        require_site(
            &renderer,
            &baseline,
            "direct_sh_compose.rs",
            include_str!("../../direct_sh_compose.rs"),
            target,
            0,
        );
    }
    eprintln!("[UploadProof] real-map legacy inventory: 2 steady-state adapter cases ran");
}

/// Source only resolves a stable target anchor to a call-site line; execution
/// counters, rather than a text search, prove that the real writer staged.
pub(super) fn require_site(
    renderer: &Renderer,
    baseline: &[(&str, u32, u64)],
    file: &str,
    source: &str,
    target: &str,
    occurrence: usize,
) {
    let mut offset = 0;
    let mut lines = Vec::new();
    while let Some(relative) = source[offset..].find("queue.write_buffer(") {
        let start = offset + relative;
        let mut depth = 1;
        let args_start = start + "queue.write_buffer(".len();
        let end = source[args_start..]
            .char_indices()
            .find_map(|(index, ch)| {
                if ch == '(' {
                    depth += 1;
                }
                if ch == ')' {
                    depth -= 1;
                }
                (depth == 0).then_some(args_start + index)
            })
            .unwrap();
        if source[args_start..end].contains(target) {
            lines.push(
                source[..start]
                    .bytes()
                    .filter(|byte| *byte == b'\n')
                    .count() as u32
                    + 1,
            );
        }
        offset = end + 1;
    }
    let line = *lines
        .get(occurrence)
        .unwrap_or_else(|| panic!("missing writer anchor {file}:{target} occurrence {occurrence}"));
    let staged: u64 = renderer
        .queue
        .writer_counts()
        .iter()
        .filter(|writer| writer.is_at(file, line))
        .map(|writer| {
            writer.writes
                - baseline
                    .iter()
                    .find(|(path, old_line, _)| *path == writer.site.file() && *old_line == line)
                    .map_or(0, |(_, _, count)| *count)
        })
        .sum();
    assert!(
        staged > 0,
        "real-map steady frames did not stage {file}:{line} target {target}; fixture/feature coverage is incomplete"
    );
}
