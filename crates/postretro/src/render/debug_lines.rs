// Immediate-mode debug-line renderer: per-frame CPU buffer of LineList segments uploaded each frame.
// See: context/lib/rendering_pipeline.md §12 Debug-Line Renderer

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

const DEBUG_LINES_SHADER_SOURCE: &str = include_str!("../shaders/debug_lines.wgsl");

const MAX_DEBUG_SEGMENTS: usize = 256 * 1024;
/// Overlay (always-on-top) segments are only used for bounding-shape AABBs,
/// which carry 12 segments each. A fraction of the depth-tested cap is plenty.
const MAX_DEBUG_OVERLAY_SEGMENTS: usize = MAX_DEBUG_SEGMENTS / 8;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DebugLineVertex {
    position: [f32; 3],
    color: [u8; 4],
}

pub struct DebugLineRenderer {
    pipeline: wgpu::RenderPipeline,
    overlay_pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    overlay_vertex_buffer: wgpu::Buffer,
    cpu_vertices: Vec<DebugLineVertex>,
    overlay_cpu_vertices: Vec<DebugLineVertex>,
    overflowed_this_frame: bool,
    overlay_overflowed_this_frame: bool,
}

/// The 12 edges of an axis-aligned box, as `(start, end)` segment pairs.
/// Ordered: 4 bottom-face edges, 4 top-face edges, 4 vertical edges.
/// Pure function — no GPU/state dependency — so the wire topology can be
/// asserted by tests without constructing a `DebugLineRenderer`.
fn aabb_edges(min: Vec3, max: Vec3) -> [(Vec3, Vec3); 12] {
    let c = [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(max.x, max.y, max.z),
        Vec3::new(min.x, max.y, max.z),
    ];
    [
        // Bottom face
        (c[0], c[1]),
        (c[1], c[2]),
        (c[2], c[3]),
        (c[3], c[0]),
        // Top face
        (c[4], c[5]),
        (c[5], c[6]),
        (c[6], c[7]),
        (c[7], c[4]),
        // Verticals
        (c[0], c[4]),
        (c[1], c[5]),
        (c[2], c[6]),
        (c[3], c[7]),
    ]
}

/// Three axis-aligned segments forming an `+` marker of total length `size`
/// centered at `center`. Pure function — no GPU/state dependency.
fn marker_segments(center: Vec3, size: f32) -> [(Vec3, Vec3); 3] {
    let h = size * 0.5;
    [
        (
            center - Vec3::new(h, 0.0, 0.0),
            center + Vec3::new(h, 0.0, 0.0),
        ),
        (
            center - Vec3::new(0.0, h, 0.0),
            center + Vec3::new(0.0, h, 0.0),
        ),
        (
            center - Vec3::new(0.0, 0.0, h),
            center + Vec3::new(0.0, 0.0, h),
        ),
    ]
}

impl DebugLineRenderer {
    pub fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        sample_count: u32,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Debug Lines Shader"),
            source: wgpu::ShaderSource::Wgsl(DEBUG_LINES_SHADER_SOURCE.into()),
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Debug Lines Pipeline Layout"),
            bind_group_layouts: &[Some(uniform_bind_group_layout)],
            immediate_size: 0,
        });

        let make_pipeline = |label: &str, depth_compare: wgpu::CompareFunction| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<DebugLineVertex>() as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute {
                                offset: 0,
                                shader_location: 0,
                                format: wgpu::VertexFormat::Float32x3,
                            },
                            wgpu::VertexAttribute {
                                offset: 12,
                                shader_location: 1,
                                format: wgpu::VertexFormat::Unorm8x4,
                            },
                        ],
                    }],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::LineList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(depth_compare),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: sample_count,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        // Depth-tested pipeline for shapes that should respect world occlusion
        // (probe markers, per-cell wires). Always-on-top pipeline for shapes
        // whose value is x-ray visibility from anywhere in the world (bounding
        // AABBs that sit at or beyond the opaque world hull).
        let pipeline = make_pipeline("Debug Lines Pipeline", wgpu::CompareFunction::LessEqual);
        let overlay_pipeline = make_pipeline(
            "Debug Lines Overlay Pipeline",
            wgpu::CompareFunction::Always,
        );

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Debug Lines Vertex Buffer"),
            size: (MAX_DEBUG_SEGMENTS * 2 * std::mem::size_of::<DebugLineVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let overlay_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Debug Lines Overlay Vertex Buffer"),
            size: (MAX_DEBUG_OVERLAY_SEGMENTS * 2 * std::mem::size_of::<DebugLineVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            overlay_pipeline,
            vertex_buffer,
            overlay_vertex_buffer,
            cpu_vertices: Vec::with_capacity(MAX_DEBUG_SEGMENTS * 2),
            overlay_cpu_vertices: Vec::with_capacity(MAX_DEBUG_OVERLAY_SEGMENTS * 2),
            overflowed_this_frame: false,
            overlay_overflowed_this_frame: false,
        }
    }

    pub fn clear(&mut self) {
        self.cpu_vertices.clear();
        self.overlay_cpu_vertices.clear();
        self.overflowed_this_frame = false;
        self.overlay_overflowed_this_frame = false;
    }

    pub fn push_line(&mut self, start: Vec3, end: Vec3, color_rgba: [u8; 4]) {
        if self.cpu_vertices.len() + 2 > MAX_DEBUG_SEGMENTS * 2 {
            if !self.overflowed_this_frame {
                log::warn!(
                    "DebugLineRenderer: segment cap {} reached; truncating",
                    MAX_DEBUG_SEGMENTS
                );
                self.overflowed_this_frame = true;
            }
            return;
        }
        self.cpu_vertices.push(DebugLineVertex {
            position: start.to_array(),
            color: color_rgba,
        });
        self.cpu_vertices.push(DebugLineVertex {
            position: end.to_array(),
            color: color_rgba,
        });
    }

    pub fn push_aabb(&mut self, min: Vec3, max: Vec3, color_rgba: [u8; 4]) {
        for (a, b) in aabb_edges(min, max) {
            self.push_line(a, b, color_rgba);
        }
    }

    pub fn push_marker(&mut self, center: Vec3, size: f32, color_rgba: [u8; 4]) {
        for (a, b) in marker_segments(center, size) {
            self.push_line(a, b, color_rgba);
        }
    }

    pub fn push_line_overlay(&mut self, start: Vec3, end: Vec3, color_rgba: [u8; 4]) {
        if self.overlay_cpu_vertices.len() + 2 > MAX_DEBUG_OVERLAY_SEGMENTS * 2 {
            if !self.overlay_overflowed_this_frame {
                log::warn!(
                    "DebugLineRenderer: overlay segment cap {} reached; truncating",
                    MAX_DEBUG_OVERLAY_SEGMENTS
                );
                self.overlay_overflowed_this_frame = true;
            }
            return;
        }
        self.overlay_cpu_vertices.push(DebugLineVertex {
            position: start.to_array(),
            color: color_rgba,
        });
        self.overlay_cpu_vertices.push(DebugLineVertex {
            position: end.to_array(),
            color: color_rgba,
        });
    }

    pub fn push_aabb_overlay(&mut self, min: Vec3, max: Vec3, color_rgba: [u8; 4]) {
        for (a, b) in aabb_edges(min, max) {
            self.push_line_overlay(a, b, color_rgba);
        }
    }

    pub fn render(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        uniform_bind_group: &wgpu::BindGroup,
    ) {
        if self.cpu_vertices.is_empty() && self.overlay_cpu_vertices.is_empty() {
            return;
        }

        if !self.cpu_vertices.is_empty() {
            queue.write_buffer(
                &self.vertex_buffer,
                0,
                bytemuck::cast_slice(&self.cpu_vertices),
            );
        }
        if !self.overlay_cpu_vertices.is_empty() {
            queue.write_buffer(
                &self.overlay_vertex_buffer,
                0,
                bytemuck::cast_slice(&self.overlay_cpu_vertices),
            );
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Debug Lines Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });

        pass.set_bind_group(0, uniform_bind_group, &[]);

        // Depth-tested first so overlay lines (drawn second) win at any pixel
        // both pipelines touch — bounding AABBs should never be visually
        // clipped by a depth-tested wire at the same pixel.
        if !self.cpu_vertices.is_empty() {
            pass.set_pipeline(&self.pipeline);
            let vertex_bytes =
                (self.cpu_vertices.len() * std::mem::size_of::<DebugLineVertex>()) as u64;
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(0..vertex_bytes));
            pass.draw(0..self.cpu_vertices.len() as u32, 0..1);
        }

        if !self.overlay_cpu_vertices.is_empty() {
            pass.set_pipeline(&self.overlay_pipeline);
            let vertex_bytes =
                (self.overlay_cpu_vertices.len() * std::mem::size_of::<DebugLineVertex>()) as u64;
            pass.set_vertex_buffer(0, self.overlay_vertex_buffer.slice(0..vertex_bytes));
            pass.draw(0..self.overlay_cpu_vertices.len() as u32, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Topology contract: an AABB emits exactly 12 edges, each corner is shared
    /// by exactly 3 edges, and every edge runs along a single axis (axis-aligned).
    #[test]
    fn aabb_edges_form_axis_aligned_box_with_twelve_edges() {
        let min = Vec3::new(-1.0, -2.0, -3.0);
        let max = Vec3::new(4.0, 5.0, 6.0);
        let edges = aabb_edges(min, max);

        assert_eq!(edges.len(), 12);

        // Every edge must vary along exactly one axis.
        for (a, b) in edges {
            let diffs = [
                (a.x - b.x).abs() > f32::EPSILON,
                (a.y - b.y).abs() > f32::EPSILON,
                (a.z - b.z).abs() > f32::EPSILON,
            ];
            let varying = diffs.iter().filter(|d| **d).count();
            assert_eq!(varying, 1, "edge {a:?} -> {b:?} is not axis-aligned");
        }

        // Each of the 8 corners must appear as an endpoint exactly 3 times
        // (3 edges per corner of a box).
        let mut endpoint_counts: std::collections::HashMap<[u32; 3], usize> =
            std::collections::HashMap::new();
        let key = |v: Vec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
        for (a, b) in edges {
            *endpoint_counts.entry(key(a)).or_default() += 1;
            *endpoint_counts.entry(key(b)).or_default() += 1;
        }
        assert_eq!(endpoint_counts.len(), 8, "expected 8 distinct corners");
        for (corner, count) in &endpoint_counts {
            assert_eq!(
                *count, 3,
                "corner {corner:?} appears {count} times, expected 3"
            );
        }
    }

    /// Topology contract: a marker emits 3 axis-aligned segments of length
    /// `size`, all sharing `center` as midpoint.
    #[test]
    fn marker_segments_form_three_axis_crosshair_at_center() {
        let center = Vec3::new(10.0, 20.0, 30.0);
        let size = 0.5;
        let segs = marker_segments(center, size);

        assert_eq!(segs.len(), 3);

        // Each segment's midpoint is the center, and each runs along a unique axis.
        let mut axes_seen = [false; 3];
        for (a, b) in segs {
            let mid = (a + b) * 0.5;
            assert!((mid - center).length() < 1e-5, "segment midpoint != center");
            assert!(
                ((b - a).length() - size).abs() < 1e-5,
                "segment length != size"
            );

            let d = b - a;
            let axis = if d.x.abs() > f32::EPSILON {
                0
            } else if d.y.abs() > f32::EPSILON {
                1
            } else {
                2
            };
            assert!(!axes_seen[axis], "duplicate axis for marker segment");
            axes_seen[axis] = true;
        }
        assert!(axes_seen.iter().all(|&s| s), "marker missed an axis");
    }
}
