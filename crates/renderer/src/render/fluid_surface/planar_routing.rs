// Planar routing: the planar layer, its group 5, and each fluid draw's split by plane identity.
// See: context/lib/rendering_pipeline.md §7.3a

use postretro_render_cpu::fluid_surface::reflection::{
    FLUID_REFLECTION_NO_PROBE, fill_probe_indices,
};
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;
use postretro_render_cpu::fluid_surface::{FluidMaterialRange, FluidSurfaceMesh};
use postretro_render_cpu::planar_reflection::{
    PLANE_HEIGHT_TOLERANCE, PLANE_UP_NORMAL_MIN, PlanarCandidates, planar_preset,
};

use super::*;
use crate::render::planar_reflection::PLANAR_LAYER_FORMAT;

/// One run of a material range's triangles: `height` is the plane of a run of
/// horizontal, up-facing, probe-bound triangles at one height, else `None`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PlanarIndexRange {
    pub start: u32,
    pub count: u32,
    pub height: Option<f32>,
}

fn same_plane(a: Option<f32>, b: Option<f32>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= PLANE_HEIGHT_TOLERANCE,
        (None, None) => true,
        _ => false,
    }
}

/// Split every material range of `mesh` into runs by plane, in index order,
/// never across a range. A triangle has a plane only when all three corners
/// face up, sit within the height tolerance of its first corner and are bound
/// to a probe, so a plane run always takes the reflecting gate. Install-time.
pub(super) fn split_planar_ranges(
    mesh: &FluidSurfaceMesh,
    volumes: &[EnvironmentVolumeRecord],
    probes: Option<&ReflectionProbesSection>,
) -> Vec<PlanarIndexRange> {
    let mut probe = vec![FLUID_REFLECTION_NO_PROBE; mesh.vertices.len()];
    fill_probe_indices(volumes, &mesh.face_drift_maps, probes, &mut probe);
    let mut parts: Vec<PlanarIndexRange> = Vec::new();
    for volume in &mesh.volumes {
        for material in &volume.material_ranges {
            let first_part = parts.len();
            let end = material.index_start + material.index_count;
            for start in (material.index_start..end).step_by(3) {
                let corners = [0, 1, 2].map(|k| mesh.indices[(start + k) as usize] as usize);
                let h = mesh.vertices[corners[0]].position[1];
                let height = corners
                    .iter()
                    .all(|&corner| {
                        let vertex = &mesh.vertices[corner];
                        vertex.normal[1] > PLANE_UP_NORMAL_MIN
                            && (vertex.position[1] - h).abs() <= PLANE_HEIGHT_TOLERANCE
                            && probe[corner] != FLUID_REFLECTION_NO_PROBE
                    })
                    .then_some(h);
                match parts[first_part..].last() {
                    Some(last) if same_plane(last.height, height) => {
                        parts.last_mut().expect("a run exists").count += 3;
                    }
                    _ => parts.push(PlanarIndexRange {
                        start,
                        count: 3,
                        height,
                    }),
                }
            }
        }
    }
    parts
}

/// Whether `volumes` hold a candidate plane by the selection's install rule:
/// [`PlanarCandidates::from_volumes`] over every volume, so probe-bound flat
/// faces grouped by height, planes under the area floor dropped. A level of
/// puddles alone has none. The renderer cannot see fluid reflectivity, so it
/// counts every volume; the app's verdict, which can, narrows it
/// ([`FluidSurfacePass::set_planar_app_candidate`]). Install-time.
pub(super) fn has_candidate_plane(
    volumes: &[EnvironmentVolumeRecord],
    probes: Option<&ReflectionProbesSection>,
) -> bool {
    let every_volume: Vec<u32> = (0..volumes.len() as u32).collect();
    !PlanarCandidates::from_volumes(volumes, &every_volume, probes)
        .planes
        .is_empty()
}

/// Which group-5 bind group a pass currently holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BoundSceneInputs {
    Ssr,
    Planar,
}

/// The planar layer at its extent, with its own depth: as the SSR target's,
/// the depth orders fluid against fluid, nearest wins, and scene depth never
/// occludes.
struct PlanarLayer {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    extent: (u32, u32),
}

impl PlanarLayer {
    fn new(device: &wgpu::Device, extent: (u32, u32)) -> Self {
        let size = wgpu::Extent3d {
            width: extent.0,
            height: extent.1,
            depth_or_array_layers: 1,
        };
        let copy = if cfg!(test) {
            wgpu::TextureUsages::COPY_SRC
        } else {
            wgpu::TextureUsages::empty()
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Planar Reflection Layer"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: PLANAR_LAYER_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | copy,
            view_formats: &[],
        });
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Planar Reflection Layer Depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        Self {
            view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
            _texture: texture,
            depth_view: depth.create_view(&wgpu::TextureViewDescriptor::default()),
            _depth: depth,
            extent,
        }
    }
}

/// The planar layer and the group 5 over it. Both exist only while a level
/// with a candidate plane is installed at a planar tier; `route` is the plane
/// the planar pass wrote this frame, which selected-plane draws read.
#[derive(Default)]
pub(super) struct PlanarRouting {
    layer: Option<PlanarLayer>,
    /// A second `FluidSceneInputs` over the scene depth and the planar layer,
    /// with its own uniform. Its layout's entries equal the SSR group's, so
    /// WebGPU's group equivalence binds it under the fluid pipeline layout.
    group: Option<FluidSceneInputs>,
    route: Option<f32>,
    /// The main group's fog weight and style, mirrored so a rebuilt group
    /// starts in step.
    fog_weight: Option<f32>,
    style: Option<FluidReflectionStyle>,
    /// The app's candidate verdict for the installed level, which reads the
    /// fluids' reflectivity: `None` until the app says (capture, renderer
    /// tests), when the install rule alone decides. The app sets it at every
    /// level install, before the fluid install, and at every fluid refresh,
    /// so it outlives the fluid clear.
    app_candidate: Option<bool>,
    #[cfg(test)]
    layer_builds: u32,
}

impl PlanarRouting {
    pub(super) fn set_fog_weight(&mut self, weight: f32) {
        self.fog_weight = Some(weight);
        if let Some(group) = self.group.as_mut() {
            group.set_fog_weight(weight);
        }
    }

    pub(super) fn set_reflection_style(&mut self, style: FluidReflectionStyle) {
        self.style = Some(style);
        if let Some(group) = self.group.as_mut() {
            group.set_reflection_style(style);
        }
    }
}

impl FluidSurfacePass {
    /// The fluid vertex stream alone (buffer 0: position, normal, UV), as the
    /// planar resolve reads it: no drift, which tilts nothing, and no probe
    /// index, since planar reads no probe resource.
    pub(in crate::render) fn planar_resolve_vertex_stream()
    -> [Option<wgpu::VertexBufferLayout<'static>>; 1] {
        let [vertex, _drift, _probe] = fluid_vertex_buffer_layouts();
        [vertex]
    }

    /// Whether the installed level holds a candidate plane, the plane
    /// selection could pick: the install rule over its volumes
    /// ([`has_candidate_plane`]), narrowed by the app's verdict when it gave
    /// one. Both build from `PlanarCandidates::from_volumes`, and the app's
    /// reflecting volumes are a subset of every volume, so with the app
    /// driving this is the app's candidate exactly.
    pub(in crate::render) fn has_planar_candidate(&self) -> bool {
        self.installed
            .as_ref()
            .is_some_and(|installed| installed.planar_candidate)
            && self.planar.app_candidate.unwrap_or(true)
    }

    /// Store the app's candidate verdict; the caller re-keys the layer.
    pub(in crate::render) fn set_planar_app_candidate(&mut self, candidate: bool) {
        self.planar.app_candidate = Some(candidate);
    }

    /// The planar layer's extent; `None` without one.
    pub(in crate::render) fn planar_layer_extent(&self) -> Option<(u32, u32)> {
        self.planar.layer.as_ref().map(|layer| layer.extent)
    }

    /// Create, resize or release the planar layer to match `wanted`, and
    /// rebuild its group 5 over the scene depth and the new layer; `rebuild`
    /// forces both after the scene targets were recreated. The extent it
    /// already has builds nothing. Never called per frame.
    pub(in crate::render) fn set_planar_layer(
        &mut self,
        device: &wgpu::Device,
        wanted: Option<(u32, u32)>,
        rebuild: bool,
    ) {
        if wanted == self.planar_layer_extent() && !rebuild {
            return;
        }
        let released = self.planar.layer.take();
        self.planar.route = None;
        let Some(extent) = wanted else {
            self.planar.group = None;
            return;
        };
        let layer = PlanarLayer::new(device, extent);
        let scene_depth = self.scene_inputs.bound_scene_depth().clone();
        if let Some(group) = self.planar.group.as_mut() {
            group.rebuild(device, &scene_depth, &layer.view);
        } else {
            let mut group = FluidSceneInputs::new(device, &scene_depth, &layer.view);
            if let Some(weight) = self.planar.fog_weight {
                group.set_fog_weight(weight);
            }
            if let Some(style) = self.planar.style {
                group.set_reflection_style(style);
            }
            self.planar.group = Some(group);
        }
        let scene = scene_depth.texture().size();
        if let Some(group) = self.planar.group.as_mut() {
            group.set_ssr_scale([
                extent.0 as f32 / scene.width.max(1) as f32,
                extent.1 as f32 / scene.height.max(1) as f32,
            ]);
        }
        self.planar.layer = Some(layer);
        #[cfg(test)]
        {
            self.planar.layer_builds += 1;
        }
        // The old layer drops only once the group holds the new view, as a
        // superseded SSR target drops only after group 5 holds its successor.
        drop(released);
    }

    /// Set the plane selected-plane draws read this frame: the planar pass
    /// sets it after writing the layer, and clears it when it writes none.
    pub(in crate::render) fn set_planar_route(&mut self, height: Option<f32>) {
        self.planar.route = height.filter(|_| self.planar.group.is_some());
    }

    /// The plane routed this frame at `quality`: none below very high, without
    /// a planar layer, or on a frame the planar pass wrote nothing.
    pub(super) fn planar_route(&self, quality: FluidReflectionQuality) -> Option<f32> {
        self.planar
            .route
            .filter(|_| planar_preset(quality).is_some() && self.planar.group.is_some())
    }

    /// Write the planar group's uniform where it changed.
    pub(in crate::render) fn upload_planar_inputs(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
    ) {
        if let Some(group) = self.planar.group.as_mut() {
            group.upload(queue);
        }
    }

    /// The planar layer and its depth, while a layer exists.
    pub(in crate::render) fn planar_layer_attachments(
        &self,
    ) -> Option<(&wgpu::TextureView, &wgpu::TextureView)> {
        self.planar
            .layer
            .as_ref()
            .map(|layer| (&layer.view, &layer.depth_view))
    }

    /// One material range of the fluid pass: whole when nothing is routed,
    /// else each run bound to the planar group on the routed plane and to the
    /// SSR group elsewhere, rebinding only where the group changes.
    pub(super) fn draw_routed_range(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        installed: &InstalledFluidSurfaces,
        range: &FluidMaterialRange,
        element: u32,
        route: Option<f32>,
        bound: &mut BoundSceneInputs,
    ) {
        // A non-reflecting draw after a planar run may keep the planar group 5
        // bound: its plain entry reads only the fog weight and scene depth,
        // which both groups hold identically.
        let (Some(height), Some(planar)) = (route, self.planar.group.as_ref()) else {
            pass.draw_indexed(
                range.index_start..range.index_start + range.index_count,
                0,
                element..element + 1,
            );
            return;
        };
        for part in parts_of(installed, range) {
            let wanted = if same_plane(part.height, Some(height)) {
                BoundSceneInputs::Planar
            } else {
                BoundSceneInputs::Ssr
            };
            if *bound != wanted {
                let group = match wanted {
                    BoundSceneInputs::Planar => planar.bind_group(),
                    BoundSceneInputs::Ssr => self.scene_inputs.bind_group(),
                };
                pass.set_bind_group(FLUID_SCENE_INPUTS_GROUP, group, &[]);
                *bound = wanted;
            }
            pass.draw_indexed(part.start..part.start + part.count, 0, element..element + 1);
        }
    }

    /// One material range of the SSR pass: whole when nothing is routed, else
    /// only its runs off the routed plane, contiguous runs merged.
    pub(super) fn draw_ssr_range(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        installed: &InstalledFluidSurfaces,
        range: &FluidMaterialRange,
        element: u32,
        route: Option<f32>,
    ) {
        let Some(height) = route else {
            pass.draw_indexed(
                range.index_start..range.index_start + range.index_count,
                0,
                element..element + 1,
            );
            return;
        };
        let mut run: Option<(u32, u32)> = None;
        for part in parts_of(installed, range) {
            if same_plane(part.height, Some(height)) {
                if let Some((start, end)) = run.take() {
                    pass.draw_indexed(start..end, 0, element..element + 1);
                }
                continue;
            }
            run = match run {
                Some((start, end)) if end == part.start => Some((start, part.start + part.count)),
                Some((start, end)) => {
                    pass.draw_indexed(start..end, 0, element..element + 1);
                    Some((part.start, part.start + part.count))
                }
                None => Some((part.start, part.start + part.count)),
            };
        }
        if let Some((start, end)) = run {
            pass.draw_indexed(start..end, 0, element..element + 1);
        }
    }

    /// Whether draw slot `slot` holds any run off `route`, the SSR pass's
    /// share of it; every slot does when nothing is routed.
    pub(super) fn slot_reflects_off_route(
        &self,
        installed: &InstalledFluidSurfaces,
        slot: u32,
        route: Option<f32>,
    ) -> bool {
        let Some(height) = route else {
            return true;
        };
        installed.volumes[slot as usize]
            .material_ranges
            .iter()
            .any(|range| {
                parts_of(installed, range)
                    .iter()
                    .any(|part| !same_plane(part.height, Some(height)))
            })
    }

    /// Whether this frame's reflecting draws hold a run on the plane at
    /// `height`: the planar pass writes nothing without one.
    pub(in crate::render) fn has_planar_parts(
        &self,
        light_term_mask: LightTermMask,
        quality: FluidReflectionQuality,
        height: f32,
    ) -> bool {
        self.installed.as_ref().is_some_and(|installed| {
            installed
                .slots
                .frame_draws(light_term_mask.bits(), quality)
                .filter(|(_, variant)| variant.reflections())
                .any(|(slot, _)| {
                    installed.volumes[slot as usize]
                        .material_ranges
                        .iter()
                        .any(|range| {
                            parts_of(installed, range)
                                .iter()
                                .any(|part| same_plane(part.height, Some(height)))
                        })
                })
        })
    }

    /// Draw this frame's reflecting runs on the plane at `height` with the
    /// pipeline and groups the caller set: the planar mask and resolve raster
    /// the selected plane's own fluid geometry, from the fluid vertex stream.
    /// With `materials`, each range binds its world material at
    /// `material_group` from the existing material bind groups, rebinding
    /// only where it changes, as the reflection pass does: the resolve reads
    /// the face texture's texel grid.
    pub(in crate::render) fn record_planar_parts(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        light_term_mask: LightTermMask,
        quality: FluidReflectionQuality,
        height: f32,
        materials: Option<(u32, &[GpuTexture])>,
    ) {
        let Some(installed) = self.installed.as_ref() else {
            return;
        };
        pass.set_vertex_buffer(0, installed.vertex_buffer.slice(..));
        pass.set_index_buffer(installed.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        let mut bound_material = None;
        for (slot, variant) in installed.slots.frame_draws(light_term_mask.bits(), quality) {
            if !variant.reflections() {
                continue;
            }
            for range in &installed.volumes[slot as usize].material_ranges {
                if let Some((group, materials)) = materials
                    && bound_material != Some(range.material_index)
                {
                    let material = materials
                        .get(range.material_index)
                        .or_else(|| materials.first())
                        .expect("renderer always keeps a placeholder material bind group");
                    pass.set_bind_group(group, &material.bind_group, &[]);
                    bound_material = Some(range.material_index);
                }
                for part in parts_of(installed, range) {
                    if same_plane(part.height, Some(height)) {
                        pass.draw_indexed(part.start..part.start + part.count, 0, 0..1);
                    }
                }
            }
        }
    }

    /// The planar layer's view; tests read it back.
    #[cfg(test)]
    pub(crate) fn planar_layer_view(&self) -> Option<&wgpu::TextureView> {
        self.planar.layer.as_ref().map(|layer| &layer.view)
    }

    /// The planar group, when it exists; tests read its uniform.
    #[cfg(test)]
    pub(crate) fn planar_group(&self) -> Option<&FluidSceneInputs> {
        self.planar.group.as_ref()
    }

    /// Planar layer creations since full init.
    #[cfg(test)]
    pub(crate) fn planar_layer_builds(&self) -> u32 {
        self.planar.layer_builds
    }
}

/// The runs of `range`, in index order: runs never cross a range, so they are
/// a contiguous slice of the installed runs.
fn parts_of<'a>(
    installed: &'a InstalledFluidSurfaces,
    range: &FluidMaterialRange,
) -> &'a [PlanarIndexRange] {
    let parts = &installed.planar_parts;
    let end = range.index_start + range.index_count;
    let first = parts.partition_point(|part| part.start < range.index_start);
    let last = parts.partition_point(|part| part.start < end);
    &parts[first..last.max(first)]
}

#[cfg(test)]
#[path = "planar_routing_tests.rs"]
mod tests;
