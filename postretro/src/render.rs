// Wireframe renderer: GPU init, buffer upload, pipeline, and draw.
// See: context/lib/rendering_pipeline.md

use std::sync::Arc;

use anyhow::{Context, Result};
use glam::Mat4;
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::visibility::{DrawRange, VisibleFaces};

// --- WGSL Shaders ---

const SHADER_SOURCE: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) base_uv: vec2<f32>,
    @location(2) vertex_color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) vert_color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.vert_color = in.vertex_color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.vert_color;
}
"#;

// --- Cluster color palette ---

/// 16 distinct, saturated colors for cluster visualization on dark backgrounds.
const CLUSTER_PALETTE: [[f32; 3]; 16] = [
    [1.0, 0.0, 0.0],  // Red
    [0.0, 1.0, 0.0],  // Green
    [0.3, 0.5, 1.0],  // Blue (brightened for dark bg)
    [1.0, 1.0, 0.0],  // Yellow
    [0.0, 1.0, 1.0],  // Cyan
    [1.0, 0.0, 1.0],  // Magenta
    [1.0, 0.5, 0.0],  // Orange
    [0.5, 1.0, 0.0],  // Lime
    [1.0, 0.4, 0.7],  // Pink
    [0.0, 0.8, 0.6],  // Teal
    [0.7, 0.3, 1.0],  // Purple
    [1.0, 0.84, 0.0], // Gold
    [1.0, 0.5, 0.31], // Coral
    [0.4, 0.75, 1.0], // Sky
    [0.2, 1.0, 0.6],  // Mint
    [1.0, 1.0, 1.0],  // White
];

/// Default wireframe color when no cluster coloring is available (BSP mode).
const DEFAULT_WIREFRAME_COLOR: [f32; 3] = [0.0, 1.0, 1.0]; // Cyan

// --- Wireframe mode ---

/// How wireframe is rendered: native PolygonMode::Line or emulated via LineList topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireframeMode {
    /// GPU supports PolygonMode::Line with triangle topology.
    PolygonModeLine,
    /// Fallback: edge-based line list built from face edges.
    LineList,
}

/// Geometry data the renderer needs from any level format.
/// Both BSP and PRL loaders produce this shape of data.
pub struct LevelGeometry<'a> {
    pub vertices: &'a [crate::bsp::TexturedVertex],
    pub indices: &'a [u32],
    /// Per-face index offset and count, used by LineList wireframe fallback.
    pub face_ranges: Vec<(u32, u32)>,
    /// Per-face cluster index for cluster-colored wireframe (PRL levels).
    /// When `Some`, each entry corresponds to the face at the same index in `face_ranges`.
    /// When `None`, all faces use the default wireframe color (BSP levels).
    pub face_cluster_indices: Option<Vec<u32>>,
}

// --- Renderer ---

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    is_surface_configured: bool,

    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,

    wireframe_mode: WireframeMode,
    has_geometry: bool,
}

impl Renderer {
    /// Create the renderer, taking ownership of all GPU state.
    ///
    /// `geometry` is `None` when no map file was loaded (renders clear color only).
    /// `force_line_list` overrides polygon mode detection and uses the LineList fallback.
    pub fn new(
        window: &Arc<Window>,
        geometry: Option<&LevelGeometry>,
        force_line_list: bool,
    ) -> Result<Self> {
        let size = window.inner_size();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let surface = instance
            .create_surface(window.clone())
            .context("failed to create wgpu surface")?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .context("no suitable GPU adapter found")?;

        log::info!("[Renderer] GPU adapter: {}", adapter.get_info().name);

        let supports_polygon_mode_line = adapter
            .features()
            .contains(wgpu::Features::POLYGON_MODE_LINE);

        let wireframe_mode = if supports_polygon_mode_line && !force_line_list {
            log::info!("[Renderer] Using PolygonMode::Line for wireframe");
            WireframeMode::PolygonModeLine
        } else {
            if force_line_list {
                log::info!("[Renderer] Forced LineList wireframe mode via CLI flag");
            } else {
                log::info!("[Renderer] POLYGON_MODE_LINE not supported, falling back to LineList");
            }
            WireframeMode::LineList
        };

        let required_features = if wireframe_mode == WireframeMode::PolygonModeLine {
            wgpu::Features::POLYGON_MODE_LINE
        } else {
            wgpu::Features::empty()
        };

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Postretro Device"),
            required_features,
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .context("failed to create GPU device")?;

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: surface_caps.alpha_modes[0],
            desired_maximum_frame_latency: 2,
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);

        let has_geometry =
            geometry.is_some_and(|g| !g.vertices.is_empty() && !g.indices.is_empty());

        let (vertex_data, index_data, index_count) = if let Some(geom) =
            geometry.filter(|g| !g.vertices.is_empty() && !g.indices.is_empty())
        {
            let colored_verts = build_wireframe_vertices(
                geom.vertices,
                geom.indices,
                &geom.face_ranges,
                &geom.face_cluster_indices,
            );

            let indices = match wireframe_mode {
                WireframeMode::PolygonModeLine => geom.indices.to_vec(),
                WireframeMode::LineList => {
                    build_line_list_indices_from_ranges(geom.indices, &geom.face_ranges)
                }
            };

            let count = indices.len() as u32;
            (
                cast_textured_vertices_to_bytes(&colored_verts),
                bytemuck_cast_slice_u32(&indices),
                count,
            )
        } else {
            // Empty placeholder buffers (wgpu requires non-zero size for some backends).
            (
                vec![0u8; 36], // one dummy vertex (9 floats: pos + uv + color)
                vec![0u8; 4],  // one dummy index
                0u32,
            )
        };

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("BSP Vertex Buffer"),
            contents: &vertex_data,
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("BSP Index Buffer"),
            contents: &index_data,
            usage: wgpu::BufferUsages::INDEX,
        });

        let view_proj = build_default_view_projection(
            surface_config.width as f32 / surface_config.height as f32,
        );
        let uniform_data = view_proj.to_cols_array();

        let uniform_bytes = cast_f32_slice_to_bytes(&uniform_data);
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ViewProj Uniform Buffer"),
            contents: &uniform_bytes,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ViewProj Bind Group Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ViewProj Bind Group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Wireframe Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Wireframe Shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SOURCE.into()),
        });

        let (topology, polygon_mode) = match wireframe_mode {
            WireframeMode::PolygonModeLine => (
                wgpu::PrimitiveTopology::TriangleList,
                wgpu::PolygonMode::Line,
            ),
            WireframeMode::LineList => (wgpu::PrimitiveTopology::LineList, wgpu::PolygonMode::Fill),
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Wireframe Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: crate::bsp::TexturedVertex::STRIDE as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        // position: vec3<f32> at offset 0
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        // base_uv: vec2<f32> at offset 12
                        wgpu::VertexAttribute {
                            offset: 12,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        // vertex_color: vec4<f32> at offset 20
                        wgpu::VertexAttribute {
                            offset: 20,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Float32x4,
                        },
                    ],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology,
                polygon_mode,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None, // No culling for wireframe.
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        if has_geometry {
            log::info!(
                "[Renderer] Pipeline ready: {} indices, {:?} mode",
                index_count,
                wireframe_mode,
            );
        } else {
            log::info!("[Renderer] Pipeline ready (no geometry loaded)");
        }

        Ok(Self {
            device,
            queue,
            surface,
            surface_config,
            is_surface_configured: true,
            pipeline,
            vertex_buffer,
            index_buffer,
            index_count,
            uniform_buffer,
            uniform_bind_group,
            wireframe_mode,
            has_geometry,
        })
    }

    /// Handle window resize. Reconfigures the surface.
    /// The caller is responsible for updating the view-projection matrix via
    /// `update_view_projection` after calling this (the camera owns aspect ratio).
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        self.is_surface_configured = true;
    }

    pub fn update_view_projection(&self, view_proj: Mat4) {
        let data = view_proj.to_cols_array();
        self.queue
            .write_buffer(&self.uniform_buffer, 0, &cast_f32_slice_to_bytes(&data));
    }

    pub fn is_ready(&self) -> bool {
        self.is_surface_configured
    }

    /// Render a frame with visibility-based culling.
    ///
    /// When `visible` is `VisibleFaces::Culled`, issues one `draw_indexed` call per visible
    /// face range. When `DrawAll`, draws everything in a single call (fallback for missing PVS).
    pub fn render_frame(&self, visible: &VisibleFaces) -> Result<()> {
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(tex) => tex,
            wgpu::CurrentSurfaceTexture::Suboptimal(tex) => {
                self.surface.configure(&self.device, &self.surface_config);
                tex
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.surface_config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                anyhow::bail!("surface lost");
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                anyhow::bail!("surface validation error");
            }
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Frame Encoder"),
            });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Wireframe Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.05,
                            b: 0.08,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });

            if self.has_geometry && self.index_count > 0 {
                render_pass.set_pipeline(&self.pipeline);
                render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

                // LineList mode uses a rebuilt index buffer with different offsets,
                // so face-level draw ranges from PVS don't apply. Fall back to draw-all.
                let effective_visible = if self.wireframe_mode == WireframeMode::LineList {
                    &VisibleFaces::DrawAll
                } else {
                    visible
                };

                match effective_visible {
                    VisibleFaces::DrawAll => {
                        render_pass.draw_indexed(0..self.index_count, 0, 0..1);
                    }
                    VisibleFaces::Culled(ranges) => {
                        self.draw_ranges(&mut render_pass, ranges);
                    }
                }
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }

    fn draw_ranges<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>, ranges: &[DrawRange]) {
        for range in ranges {
            let start = range.index_offset;
            let end = start + range.index_count;
            render_pass.draw_indexed(start..end, 0, 0..1);
        }
    }
}

// --- Hardcoded view-projection ---

/// Camera at (0, 200, 500) looking at origin.
fn build_default_view_projection(aspect: f32) -> Mat4 {
    let eye = glam::Vec3::new(0.0, 200.0, 500.0);
    let center = glam::Vec3::ZERO;
    let up = glam::Vec3::Y;

    let view = Mat4::look_at_rh(eye, center, up);
    let projection = Mat4::perspective_rh(
        std::f32::consts::FRAC_PI_2, // 90 degree FOV
        aspect,
        0.1,
        4096.0,
    );

    projection * view
}

// --- Line-list fallback ---

/// Build a line-list index buffer from triangle indices grouped by face.
///
/// For each face, extracts the unique edges from its fan triangulation and emits them
/// as line segments. This avoids drawing interior fan edges that don't correspond to
/// actual BSP face edges.
///
/// `face_ranges` is a slice of (index_offset, index_count) pairs.
fn build_line_list_indices_from_ranges(
    tri_indices: &[u32],
    face_ranges: &[(u32, u32)],
) -> Vec<u32> {
    let mut line_indices = Vec::with_capacity(tri_indices.len() * 2);

    for &(index_offset, index_count) in face_ranges {
        let offset = index_offset as usize;
        let count = index_count as usize;

        if count < 3 {
            continue;
        }

        // The fan triangulation uses vertex 0 as the hub. The face's actual polygon
        // edges are: v0-v1, v1-v2, v2-v3, ..., v(N-1)-v0. We reconstruct these from
        // the triangle indices.
        //
        // First triangle gives us the hub vertex (index 0 in the face).
        let hub = tri_indices[offset];

        // Collect the ring of vertices from the fan. In fan order:
        // triangle i has indices: (hub, hub+i+1, hub+i+2).
        // The ring is: tri_indices[offset+1], tri_indices[offset+2], ..., last tri's third vertex.
        let num_tris = count / 3;
        let mut ring = Vec::with_capacity(num_tris + 1);
        for t in 0..num_tris {
            let base = offset + t * 3;
            if t == 0 {
                ring.push(tri_indices[base + 1]);
            }
            ring.push(tri_indices[base + 2]);
        }

        line_indices.push(hub);
        line_indices.push(ring[0]);

        for i in 0..ring.len() - 1 {
            line_indices.push(ring[i]);
            line_indices.push(ring[i + 1]);
        }

        if let Some(&last) = ring.last() {
            line_indices.push(last);
            line_indices.push(hub);
        }
    }

    line_indices
}

// --- Per-vertex color assignment ---

/// Build wireframe-colored vertices from the textured vertex buffer.
///
/// Copies the input vertices and overwrites vertex_color for wireframe visualization.
/// When `face_cluster_indices` is `Some`, each face's vertices are colored by
/// cluster index using the palette. When `None`, all vertices use the default
/// wireframe color (BSP fallback).
fn build_wireframe_vertices(
    vertices: &[crate::bsp::TexturedVertex],
    indices: &[u32],
    face_ranges: &[(u32, u32)],
    face_cluster_indices: &Option<Vec<u32>>,
) -> Vec<crate::bsp::TexturedVertex> {
    let mut colored: Vec<crate::bsp::TexturedVertex> = vertices.to_vec();

    // Set default wireframe color for all vertices.
    for v in colored.iter_mut() {
        v.vertex_color = [
            DEFAULT_WIREFRAME_COLOR[0],
            DEFAULT_WIREFRAME_COLOR[1],
            DEFAULT_WIREFRAME_COLOR[2],
            1.0,
        ];
    }

    // If cluster indices are available, overwrite colors per face.
    if let Some(cluster_indices) = face_cluster_indices {
        for (face_idx, &(index_offset, index_count)) in face_ranges.iter().enumerate() {
            let cluster_idx = cluster_indices.get(face_idx).copied().unwrap_or(0) as usize;
            let color = CLUSTER_PALETTE[cluster_idx % CLUSTER_PALETTE.len()];

            let start = index_offset as usize;
            let end = start + index_count as usize;
            for &vert_idx in indices.get(start..end).unwrap_or(&[]) {
                if let Some(v) = colored.get_mut(vert_idx as usize) {
                    v.vertex_color = [color[0], color[1], color[2], 1.0];
                }
            }
        }
    }

    colored
}

// --- Byte casting helpers ---

fn cast_f32_slice_to_bytes(data: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(data.len() * 4);
    for &val in data {
        bytes.extend_from_slice(&val.to_ne_bytes());
    }
    bytes
}

fn cast_textured_vertices_to_bytes(data: &[crate::bsp::TexturedVertex]) -> Vec<u8> {
    let byte_len = data.len() * crate::bsp::TexturedVertex::STRIDE;
    let mut bytes = Vec::with_capacity(byte_len);
    for vertex in data {
        for &c in &vertex.position {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.base_uv {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.vertex_color {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
    }
    bytes
}

fn bytemuck_cast_slice_u32(data: &[u32]) -> Vec<u8> {
    let byte_len = std::mem::size_of_val(data);
    let mut bytes = Vec::with_capacity(byte_len);
    for &val in data {
        bytes.extend_from_slice(&val.to_ne_bytes());
    }
    bytes
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_line_list_from_single_triangle() {
        let tri_indices = vec![0, 1, 2];
        let face_ranges = vec![(0u32, 3u32)];

        let lines = build_line_list_indices_from_ranges(&tri_indices, &face_ranges);
        assert_eq!(lines, vec![0, 1, 1, 2, 2, 0]);
    }

    #[test]
    fn build_line_list_from_quad() {
        let tri_indices = vec![0, 1, 2, 0, 2, 3];
        let face_ranges = vec![(0, 6)];

        let lines = build_line_list_indices_from_ranges(&tri_indices, &face_ranges);
        assert_eq!(lines, vec![0, 1, 1, 2, 2, 3, 3, 0]);
    }

    #[test]
    fn build_line_list_from_pentagon() {
        let tri_indices = vec![0, 1, 2, 0, 2, 3, 0, 3, 4];
        let face_ranges = vec![(0, 9)];

        let lines = build_line_list_indices_from_ranges(&tri_indices, &face_ranges);
        assert_eq!(lines, vec![0, 1, 1, 2, 2, 3, 3, 4, 4, 0]);
    }

    #[test]
    fn build_line_list_multiple_faces() {
        let tri_indices = vec![0, 1, 2, 10, 11, 12];
        let face_ranges = vec![(0, 3), (3, 3)];

        let lines = build_line_list_indices_from_ranges(&tri_indices, &face_ranges);
        assert_eq!(lines, vec![0, 1, 1, 2, 2, 0, 10, 11, 11, 12, 12, 10]);
    }

    #[test]
    fn build_line_list_degenerate_face_skipped() {
        let tri_indices = vec![0, 1];
        let face_ranges = vec![(0, 2)];

        let lines = build_line_list_indices_from_ranges(&tri_indices, &face_ranges);
        assert!(lines.is_empty());
    }

    #[test]
    fn default_view_projection_is_finite() {
        let vp = build_default_view_projection(16.0 / 9.0);
        let cols = vp.to_cols_array();
        for (i, val) in cols.iter().enumerate() {
            assert!(val.is_finite(), "view_proj[{i}] is not finite: {val}");
        }
    }

    #[test]
    fn cast_textured_vertices_roundtrips() {
        let input = vec![
            crate::bsp::TexturedVertex {
                position: [1.0, 2.0, 3.0],
                base_uv: [0.5, 0.75],
                vertex_color: [1.0, 1.0, 1.0, 1.0],
            },
            crate::bsp::TexturedVertex {
                position: [4.0, 5.0, 6.0],
                base_uv: [0.25, 0.125],
                vertex_color: [0.5, 0.5, 0.5, 1.0],
            },
        ];
        let bytes = cast_textured_vertices_to_bytes(&input);
        // 2 vertices * 9 floats * 4 bytes = 72 bytes
        assert_eq!(bytes.len(), 72);

        // Read back.
        let mut output = Vec::new();
        for chunk in bytes.chunks_exact(4) {
            output.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        // First vertex: pos(1,2,3), uv(0.5,0.75), color(1,1,1,1)
        assert_eq!(
            output,
            vec![
                1.0, 2.0, 3.0, 0.5, 0.75, 1.0, 1.0, 1.0, 1.0, 4.0, 5.0, 6.0, 0.25, 0.125, 0.5,
                0.5, 0.5, 1.0
            ]
        );
    }

    #[test]
    fn byte_cast_u32_roundtrips() {
        let input = vec![100u32, 200, 300];
        let bytes = bytemuck_cast_slice_u32(&input);
        assert_eq!(bytes.len(), 12); // 3 * 4 bytes

        let mut output = Vec::new();
        for chunk in bytes.chunks_exact(4) {
            output.push(u32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        assert_eq!(output, vec![100, 200, 300]);
    }
}
