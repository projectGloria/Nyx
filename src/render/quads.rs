//! Solid rectangles: cell backgrounds, selection, underline, strikethrough and the cursor.
//!
//! One instanced pipeline, four vertices generated in the shader, no geometry buffer. Instances
//! are written in two contiguous ranges so `paint` can draw the "under" set, hand over to
//! glyphon, then draw the "over" set — all inside egui's single render pass.

use bytemuck::{Pod, Zeroable};

use crate::render::metrics::CellMetrics;
use crate::term::snapshot::GridSnapshot;
use crate::theme::{Rgba, Theme};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct QuadInstance {
    /// x, y, w, h in physical pixels, relative to the framebuffer origin.
    pub rect: [f32; 4],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    screen: [f32; 2],
    _pad: [f32; 2],
}

const SHADER: &str = r#"
struct Globals {
    screen: vec2<f32>,
    pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> globals: Globals;

struct Instance {
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    // Triangle strip corners: (0,0) (1,0) (0,1) (1,1).
    let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
    let p = inst.rect.xy + corner * inst.rect.zw;
    let ndc = vec2<f32>(
        p.x / globals.screen.x * 2.0 - 1.0,
        1.0 - p.y / globals.screen.y * 2.0,
    );
    var out: VsOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.color = inst.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

pub struct QuadPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    globals: wgpu::Buffer,
}

impl QuadPipeline {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nyx.quads.shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nyx.quads.globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("nyx.quads.bgl"),
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

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nyx.quads.bg"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nyx.quads.layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nyx.quads.pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<QuadInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Self { pipeline, bind_group, globals }
    }

    pub fn set_screen(&self, queue: &wgpu::Queue, width: f32, height: f32) {
        queue.write_buffer(
            &self.globals,
            0,
            bytemuck::bytes_of(&Globals { screen: [width, height], _pad: [0.0; 2] }),
        );
    }

    /// Deliberately not `bind<'a>(&'a self, pass: &mut RenderPass<'a>)`. `&mut T` is invariant in
    /// `T`, so tying the two lifetimes together against the `RenderPass<'static>` that egui hands
    /// to `paint` would demand a `&'static self` — and the resources are borrowed from egui's
    /// callback map, which is not static. wgpu 30 refcounts these internally and does not need
    /// the tie.
    pub fn bind(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
    }
}

/// A per-pane instance buffer that grows by doubling and is never shrunk — a single `cat` of a
/// colourful file sizes it once for the life of the pane.
pub struct QuadBatch {
    buffer: Option<wgpu::Buffer>,
    capacity: usize,
    pub under: std::ops::Range<u32>,
    pub over: std::ops::Range<u32>,
}

impl Default for QuadBatch {
    fn default() -> Self {
        Self { buffer: None, capacity: 0, under: 0..0, over: 0..0 }
    }
}

impl QuadBatch {
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[QuadInstance],
        under_count: usize,
    ) {
        self.under = 0..under_count as u32;
        self.over = under_count as u32..instances.len() as u32;

        if instances.is_empty() {
            return;
        }

        if self.capacity < instances.len() {
            let capacity = instances.len().next_power_of_two();
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nyx.quads.instances"),
                size: (capacity * std::mem::size_of::<QuadInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.capacity = capacity;
        }

        if let Some(buffer) = &self.buffer {
            queue.write_buffer(buffer, 0, bytemuck::cast_slice(instances));
        }
    }

    pub fn buffer(&self) -> Option<&wgpu::Buffer> {
        self.buffer.as_ref()
    }
}

/// Build every quad for one pane.
///
/// Returns the instances with the "under" set first, and the index at which "over" begins.
pub fn build(
    snapshot: &GridSnapshot,
    metrics: &CellMetrics,
    theme: &Theme,
    origin: (f32, f32),
    srgb: bool,
    focused: bool,
) -> (Vec<QuadInstance>, usize) {
    let mut under: Vec<QuadInstance> = Vec::new();
    let mut over: Vec<QuadInstance> = Vec::new();

    let (ox, oy) = origin;
    let x0 = ox + metrics.padding;
    let y0 = oy + metrics.padding;

    let push = |v: &mut Vec<QuadInstance>, x: f32, y: f32, w: f32, h: f32, c: Rgba| {
        v.push(QuadInstance { rect: [x, y, w, h], color: c.to_target(srgb) });
    };

    for (row_idx, row) in snapshot.rows_data.iter().enumerate() {
        let y = y0 + row_idx as f32 * metrics.cell_h;

        // Backgrounds, run-length merged across adjacent runs that resolved to the same colour.
        // One quad per cell would be ~2400 instances a frame for an 80x30 grid, nearly all of
        // them the default background.
        let mut span: Option<(f32, f32, Rgba)> = None;
        for run in &row.runs {
            let rx = x0 + run.col as f32 * metrics.cell_w;
            let rw = run.cols as f32 * metrics.cell_w;
            match &mut span {
                Some((sx, sw, sc))
                    if *sc == run.attrs.bg && (*sx + *sw - rx).abs() < 0.01 =>
                {
                    *sw += rw;
                },
                _ => {
                    if let Some((sx, sw, sc)) = span.take() {
                        if sc != snapshot.default_bg {
                            push(&mut under, sx, y, sw, metrics.cell_h, sc);
                        }
                    }
                    span = Some((rx, rw, run.attrs.bg));
                },
            }
        }
        if let Some((sx, sw, sc)) = span.take() {
            if sc != snapshot.default_bg {
                push(&mut under, sx, y, sw, metrics.cell_h, sc);
            }
        }

        // Decorations sit above the glyphs so an underline is not hidden by the next row's
        // background quad.
        for run in &row.runs {
            let rx = x0 + run.col as f32 * metrics.cell_w;
            let rw = run.cols as f32 * metrics.cell_w;
            if run.attrs.underline {
                push(
                    &mut over,
                    rx,
                    y + metrics.underline_y,
                    rw,
                    metrics.underline_h,
                    run.attrs.fg,
                );
            }
            if run.attrs.strikeout {
                push(
                    &mut over,
                    rx,
                    y + metrics.baseline - metrics.font_size * 0.25,
                    rw,
                    metrics.underline_h,
                    run.attrs.fg,
                );
            }
        }
    }

    if let Some(cursor) = snapshot.cursor {
        use alacritty_terminal::vte::ansi::CursorShape;
        let cx = x0 + cursor.col as f32 * metrics.cell_w;
        let cy = y0 + cursor.row as f32 * metrics.cell_h;
        let w = metrics.cell_w;
        let h = metrics.cell_h;
        let thickness = metrics.underline_h.max(1.0);
        let color = theme.cursor_bg;

        if focused {
            match cursor.shape {
                // Already painted as the cell's own background by `snapshot::capture`, with the
                // glyph recoloured to match. Drawing a quad here too would cover that glyph.
                CursorShape::Block if cursor.drawn_inline => {},
                CursorShape::Block => push(&mut under, cx, cy, w, h, color),
                CursorShape::Underline => {
                    push(&mut over, cx, cy + h - thickness, w, thickness, color)
                },
                CursorShape::Beam => push(&mut over, cx, cy, thickness, h, color),
                CursorShape::HollowBlock => hollow(&mut over, cx, cy, w, h, thickness, color, srgb),
                CursorShape::Hidden => {},
            }
        } else if cursor.shape != CursorShape::Hidden {
            // An unfocused window shows an outline, so you can tell at a glance which pane the
            // keyboard is going to.
            hollow(&mut over, cx, cy, w, h, thickness, color, srgb);
        }
    }

    let under_count = under.len();
    under.extend_from_slice(&over);
    (under, under_count)
}

#[allow(clippy::too_many_arguments)]
fn hollow(
    v: &mut Vec<QuadInstance>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    t: f32,
    c: Rgba,
    srgb: bool,
) {
    let color = c.to_target(srgb);
    v.push(QuadInstance { rect: [x, y, w, t], color });
    v.push(QuadInstance { rect: [x, y + h - t, w, t], color });
    v.push(QuadInstance { rect: [x, y + t, t, h - 2.0 * t], color });
    v.push(QuadInstance { rect: [x + w - t, y + t, t, h - 2.0 * t], color });
}
