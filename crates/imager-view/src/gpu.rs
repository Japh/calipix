//! The wgpu side of the viewer: pipelines, tile textures and drawing.
//!
//! Each tile is one texture per channel holding the raw samples (`R8Uint`,
//! `R16Uint`, `R16Sint` or `R32Float`). A tile is drawn as a full-target
//! triangle restricted by the scissor rectangle to the tile's part of the
//! screen. The fragment shader works from the device pixel position alone:
//! it maps the pixel centre to a texel of the tile, reads the texel of each
//! channel with `textureLoad` (there is no sampler and so no smoothing),
//! maps it to a display level as described in [`crate::mapping`] and adds
//! up the channels' lookup-table colours.
//!
//! Which pixels belong to which tile is decided on the CPU, in double
//! precision, and enforced by the scissor rectangles, which partition the
//! screen ([`crate::geometry::ViewTransform::pixels_showing`]). The shader
//! clamps its texel to the tile, so single-precision error in the pixel
//! position can at worst pick a neighbouring texel where a pixel centre
//! sits on a texel boundary; it can never leave a gap between tiles.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;
use imager_core::{Buffer, DType};

use crate::mapping::{
    DisplayPlan, MAX_GPU_CHANNELS, SampleKind, float_params, level_table, level_wgsl, table_rows,
};

/// The pixel format of offscreen renders.
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Per-tile shader constants. Must match `Params` in the shader.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    /// A device pixel centre the tile coordinates are measured from.
    base: [f32; 2],
    /// The tile-local texel coordinate at `base`.
    texel0: [f32; 2],
    /// Tile size in texels.
    size: [i32; 2],
    texels_per_px: f32,
    n_channels: u32,
    table_rows: u32,
    _pad: [u32; 3],
    /// Float mapping per channel: `(min, scale, unused, unused)`.
    chan: [[f32; 4]; MAX_GPU_CHANNELS],
}

const PARAMS_SIZE: u64 = std::mem::size_of::<Params>() as u64;

/// The shader for samples of `kind` with `channels` channels drawn. There
/// is one shader per channel count rather than a run-time test of the
/// count: Microsoft's software rasteriser was seen to skip texture reads
/// placed inside such a test for pixels on the edge of a scissor rectangle.
fn shader_source(kind: SampleKind, channels: usize) -> String {
    let img_t = match kind {
        SampleKind::Uint => "u32",
        SampleKind::Sint => "i32",
        SampleKind::Float => "f32",
    };
    let mut src = format!(
        "struct Params {{
    base: vec2<f32>,
    texel0: vec2<f32>,
    size: vec2<i32>,
    texels_per_px: f32,
    n_channels: u32,
    table_rows: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
    chan: array<vec4<f32>, {MAX_GPU_CHANNELS}>,
}};
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var levels: texture_2d<u32>;
@group(0) @binding(2) var lut: texture_2d<u32>;
"
    );
    for c in 0..MAX_GPU_CHANNELS {
        src += &format!(
            "@group(0) @binding({}) var img{c}: texture_2d<{img_t}>;\n",
            c + 3
        );
    }
    src += "
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the whole target; the scissor does the rest.
    let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

";
    src += level_wgsl(kind);
    src += "

fn colour(c: u32, l: u32) -> vec3<u32> {
    return textureLoad(lut, vec2<i32>(i32(l), i32(c)), 0).rgb;
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let t = params.texel0 + (frag.xy - params.base) * params.texels_per_px;
    let p = clamp(vec2<i32>(floor(t)), vec2<i32>(0), params.size - vec2<i32>(1));
    var sum = vec3<u32>(0u);
";
    for c in 0..channels.min(MAX_GPU_CHANNELS) {
        src += &format!(
            "    sum += colour({c}u, level({c}u, textureLoad(img{c}, p, 0).r));
"
        );
    }
    src += "    return vec4<f32>(vec3<f32>(min(sum, vec3<u32>(255u))) / 255.0, 1.0);
}
";
    if kind == SampleKind::Uint {
        // A finished RGBA picture (CPU-rendered) copied texel for texel.
        src += "
@fragment
fn fs_picture(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let t = params.texel0 + (frag.xy - params.base) * params.texels_per_px;
    let p = clamp(vec2<i32>(floor(t)), vec2<i32>(0), params.size - vec2<i32>(1));
    return vec4<f32>(vec3<f32>(textureLoad(img0, p, 0).rgb) / 255.0, 1.0);
}
";
    }
    src
}

fn kind_index(kind: SampleKind) -> usize {
    match kind {
        SampleKind::Uint => 0,
        SampleKind::Sint => 1,
        SampleKind::Float => 2,
    }
}

/// Sample kind index, channels drawn, picture mode and target format.
type PipelineKey = (usize, usize, bool, wgpu::TextureFormat);

const KINDS: [SampleKind; 3] = [SampleKind::Uint, SampleKind::Sint, SampleKind::Float];

struct Inner {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_name: String,
    backend: String,
    software: bool,
    target_format: wgpu::TextureFormat,
    max_texture_side: u32,
    uniform_align: u64,
    layouts: [wgpu::BindGroupLayout; 3],
    pipelines: Mutex<HashMap<PipelineKey, wgpu::RenderPipeline>>,
    /// 1x1 textures bound in place of channels that are not drawn.
    blanks: [wgpu::TextureView; 3],
}

/// The viewer's GPU resources: device, queue and render pipelines. Create
/// one per application from the renderer egui draws with, and pass it to
/// every [`ImageView`](crate::ImageView). Cloning is cheap.
#[derive(Clone)]
pub struct ViewerGpu(Arc<Inner>);

impl std::fmt::Debug for ViewerGpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewerGpu")
            .field("adapter", &self.0.adapter_name)
            .field("backend", &self.0.backend)
            .field("software", &self.0.software)
            .field("target_format", &self.0.target_format)
            .field("max_texture_side", &self.0.max_texture_side)
            .finish()
    }
}

impl ViewerGpu {
    /// Resources for drawing inside egui, from the render state eframe
    /// provides (`CreationContext::wgpu_render_state`).
    pub fn from_render_state(state: &egui_wgpu::RenderState) -> ViewerGpu {
        ViewerGpu::new(
            &state.device,
            &state.queue,
            &state.adapter.get_info(),
            state.target_format,
        )
    }

    /// Resources for a device, drawing to targets of `target_format` (the
    /// surface format inside egui; offscreen renders always use
    /// [`OFFSCREEN_FORMAT`]).
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        adapter: &wgpu::AdapterInfo,
        target_format: wgpu::TextureFormat,
    ) -> ViewerGpu {
        let texture_entry = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layouts = KINDS.map(|kind| {
            let sample_type = match kind {
                SampleKind::Uint => wgpu::TextureSampleType::Uint,
                SampleKind::Sint => wgpu::TextureSampleType::Sint,
                // `R32Float` can only be filtered with an optional feature;
                // it is only ever read with `textureLoad`.
                SampleKind::Float => wgpu::TextureSampleType::Float { filterable: false },
            };
            let mut entries = vec![
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(PARAMS_SIZE),
                    },
                    count: None,
                },
                texture_entry(1, wgpu::TextureSampleType::Uint),
                texture_entry(2, wgpu::TextureSampleType::Uint),
            ];
            entries.extend((0..MAX_GPU_CHANNELS as u32).map(|c| texture_entry(c + 3, sample_type)));
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("imager-view tile layout"),
                entries: &entries,
            })
        });
        let blanks = [
            wgpu::TextureFormat::R8Uint,
            wgpu::TextureFormat::R16Sint,
            wgpu::TextureFormat::R32Float,
        ]
        .map(|format| {
            create_texture(device, 1, 1, format)
                .create_view(&wgpu::TextureViewDescriptor::default())
        });
        let limits = device.limits();
        ViewerGpu(Arc::new(Inner {
            device: device.clone(),
            queue: queue.clone(),
            adapter_name: adapter.name.clone(),
            backend: format!("{:?}", adapter.backend),
            software: adapter.device_type == wgpu::DeviceType::Cpu,
            target_format,
            max_texture_side: limits.max_texture_dimension_2d,
            uniform_align: limits.min_uniform_buffer_offset_alignment.max(1) as u64,
            layouts,
            pipelines: Mutex::new(HashMap::new()),
            blanks,
        }))
    }

    /// True if the adapter is a software renderer (for example Microsoft's
    /// WARP). It draws correctly but far slower than the CPU renderer, so
    /// views fall back to the CPU path on it.
    pub fn is_software(&self) -> bool {
        self.0.software
    }

    /// The adapter's name, for diagnostics.
    pub fn adapter_name(&self) -> &str {
        &self.0.adapter_name
    }

    /// The graphics backend in use ("Vulkan", "Dx12", "Gl", ...).
    pub fn backend(&self) -> &str {
        &self.0.backend
    }

    /// Largest texture side the device accepts; tiles are never larger.
    pub fn max_texture_side(&self) -> u32 {
        self.0.max_texture_side
    }

    pub(crate) fn target_format(&self) -> wgpu::TextureFormat {
        self.0.target_format
    }

    fn pipeline(
        &self,
        kind: SampleKind,
        channels: usize,
        picture: bool,
        format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        let k = kind_index(kind);
        let channels = channels.min(MAX_GPU_CHANNELS);
        let mut pipelines = self.0.pipelines.lock().unwrap_or_else(|e| e.into_inner());
        pipelines
            .entry((k, channels, picture, format))
            .or_insert_with(|| {
                let device = &self.0.device;
                let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("imager-view tile shader"),
                    source: wgpu::ShaderSource::Wgsl(shader_source(kind, channels).into()),
                });
                let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("imager-view tile pipeline layout"),
                    bind_group_layouts: &[Some(&self.0.layouts[k])],
                    immediate_size: 0,
                });
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("imager-view tile pipeline"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some(if picture { "fs_picture" } else { "fs_main" }),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            })
            .clone()
    }

    /// Uploads a tile's samples as a texture. The pixel type must have a
    /// GPU path ([`SampleKind::of`]); `None` otherwise.
    pub(crate) fn upload_tile(
        &self,
        samples: &Buffer,
        width: usize,
        height: usize,
    ) -> Option<wgpu::Texture> {
        let (format, bytes): (_, &[u8]) = match samples {
            Buffer::U8(s) => (wgpu::TextureFormat::R8Uint, s),
            Buffer::U16(s) => (wgpu::TextureFormat::R16Uint, bytemuck::cast_slice(s)),
            Buffer::I16(s) => (wgpu::TextureFormat::R16Sint, bytemuck::cast_slice(s)),
            Buffer::F32(s) => (wgpu::TextureFormat::R32Float, bytemuck::cast_slice(s)),
            _ => return None,
        };
        let side = self.0.max_texture_side as usize;
        if width == 0 || height == 0 || width > side || height > side {
            return None;
        }
        let texture = create_texture(&self.0.device, width as u32, height as u32, format);
        write_texture(
            &self.0.queue,
            &texture,
            0,
            width as u32,
            height as u32,
            bytes,
        );
        Some(texture)
    }

    /// Uploads a finished RGBA picture for [`GpuFrame::picture`], reusing
    /// `existing` if it has the right size. `None` if the picture is larger
    /// than the device's textures can be.
    pub(crate) fn upload_picture(
        &self,
        picture: &imager_render::Rgba8,
        existing: Option<wgpu::Texture>,
    ) -> Option<wgpu::Texture> {
        let (width, height) = (picture.width as u32, picture.height as u32);
        let side = self.0.max_texture_side;
        if width == 0 || height == 0 || width > side || height > side {
            return None;
        }
        let texture = existing
            .filter(|t| t.width() == width && t.height() == height)
            .unwrap_or_else(|| {
                create_texture(
                    &self.0.device,
                    width,
                    height,
                    wgpu::TextureFormat::Rgba8Uint,
                )
            });
        write_texture(&self.0.queue, &texture, 0, width, height, &picture.data);
        Some(texture)
    }

    /// Creates the per-view textures holding the level tables and lookup
    /// tables for images of `dtype`.
    pub(crate) fn create_display(&self, dtype: DType) -> DisplayTextures {
        let rows = table_rows(dtype);
        let levels = create_texture(
            &self.0.device,
            256,
            (rows * MAX_GPU_CHANNELS as u32).max(1),
            wgpu::TextureFormat::R8Uint,
        );
        let lut = create_texture(
            &self.0.device,
            256,
            MAX_GPU_CHANNELS as u32,
            wgpu::TextureFormat::Rgba8Uint,
        );
        DisplayTextures {
            dtype,
            levels_view: levels.create_view(&wgpu::TextureViewDescriptor::default()),
            lut_view: lut.create_view(&wgpu::TextureViewDescriptor::default()),
            levels,
            lut,
        }
    }

    /// Writes the level tables and lookup tables for `plan`, which must be
    /// drawable ([`crate::mapping::gpu_can_draw`]).
    pub(crate) fn write_display(&self, display: &DisplayTextures, plan: &DisplayPlan) {
        let rows = table_rows(plan.dtype);
        for (c, channel) in plan.channels.iter().take(MAX_GPU_CHANNELS).enumerate() {
            let rgba: Vec<u8> = channel
                .lut
                .table
                .iter()
                .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
                .collect();
            write_texture(&self.0.queue, &display.lut, c as u32, 256, 1, &rgba);
            if rows > 0
                && let Ok(table) = level_table(plan.dtype, plan.rgb, channel.range)
            {
                write_texture(
                    &self.0.queue,
                    &display.levels,
                    c as u32 * rows,
                    256,
                    rows,
                    &table,
                );
            }
        }
    }
}

fn create_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("imager-view"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// Writes `height` full-width rows starting at row `y`.
fn write_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    y: u32,
    width: u32,
    height: u32,
    bytes: &[u8],
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x: 0, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes.len() as u32 / height.max(1)),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

/// Level tables and lookup tables of one view.
pub(crate) struct DisplayTextures {
    pub dtype: DType,
    levels: wgpu::Texture,
    lut: wgpu::Texture,
    levels_view: wgpu::TextureView,
    lut_view: wgpu::TextureView,
}

/// One tile to draw.
pub(crate) struct TileDraw {
    /// The tile's texture for each drawn channel.
    pub textures: Vec<wgpu::Texture>,
    /// Device pixels the tile may touch: `[x0, y0, x1, y1]`, end exclusive.
    pub scissor: [i32; 4],
    /// Device pixel (top-left corner) the coordinates are measured from.
    pub base: [i32; 2],
    /// Tile-local texel coordinate of the centre of `base`.
    pub texel0: [f64; 2],
    /// Tile size in texels.
    pub size: [u32; 2],
    pub texels_per_px: f64,
}

struct Prepared {
    pipeline: wgpu::RenderPipeline,
    bind_groups: Vec<wgpu::BindGroup>,
    stride: u32,
}

/// Everything needed to draw one view once: used as an egui paint callback
/// and for offscreen renders.
pub(crate) struct GpuFrame {
    gpu: ViewerGpu,
    kind: SampleKind,
    /// Draw a finished RGBA picture instead of mapping samples.
    picture: bool,
    format: wgpu::TextureFormat,
    levels: wgpu::TextureView,
    lut: wgpu::TextureView,
    table_rows: u32,
    chan: [[f32; 4]; MAX_GPU_CHANNELS],
    n_channels: u32,
    /// The view's rectangle in device pixels; nothing is drawn outside it.
    bounds: [i32; 4],
    draws: Vec<TileDraw>,
    prepared: Mutex<Option<Prepared>>,
}

impl GpuFrame {
    /// A frame drawing `draws` with the display state last written to
    /// `display` for `plan`.
    pub fn new(
        gpu: &ViewerGpu,
        display: &DisplayTextures,
        plan: &DisplayPlan,
        format: wgpu::TextureFormat,
        bounds: [i32; 4],
        draws: Vec<TileDraw>,
    ) -> Option<GpuFrame> {
        let kind = SampleKind::of(plan.dtype)?;
        let mut chan = [[0.0; 4]; MAX_GPU_CHANNELS];
        if kind == SampleKind::Float {
            for (slot, channel) in chan.iter_mut().zip(&plan.channels) {
                let p = float_params(channel.range)?;
                *slot = [p.min, p.scale, 0.0, 0.0];
            }
        }
        Some(GpuFrame {
            gpu: gpu.clone(),
            kind,
            picture: false,
            format,
            levels: display.levels_view.clone(),
            lut: display.lut_view.clone(),
            table_rows: table_rows(plan.dtype),
            chan,
            n_channels: plan.channels.len().min(MAX_GPU_CHANNELS) as u32,
            bounds,
            draws,
            prepared: Mutex::new(None),
        })
    }

    /// A frame that copies a finished RGBA picture (see
    /// [`ViewerGpu::upload_picture`]) to the device pixels of `bounds`, one
    /// texel per pixel.
    pub fn picture(
        gpu: &ViewerGpu,
        picture: &wgpu::Texture,
        format: wgpu::TextureFormat,
        bounds: [i32; 4],
    ) -> GpuFrame {
        let blank = &gpu.0.blanks[kind_index(SampleKind::Uint)];
        GpuFrame {
            gpu: gpu.clone(),
            kind: SampleKind::Uint,
            picture: true,
            format,
            levels: blank.clone(),
            lut: blank.clone(),
            table_rows: 0,
            chan: [[0.0; 4]; MAX_GPU_CHANNELS],
            n_channels: 1,
            bounds,
            draws: vec![TileDraw {
                textures: vec![picture.clone()],
                scissor: bounds,
                base: [bounds[0], bounds[1]],
                texel0: [0.5, 0.5],
                size: [picture.width(), picture.height()],
                texels_per_px: 1.0,
            }],
            prepared: Mutex::new(None),
        }
    }

    fn prepare_draws(&self) {
        let inner = &self.gpu.0;
        let stride = PARAMS_SIZE.div_ceil(inner.uniform_align) * inner.uniform_align;
        let mut data = vec![0u8; stride as usize * self.draws.len().max(1)];
        for (i, draw) in self.draws.iter().enumerate() {
            let params = Params {
                base: [draw.base[0] as f32 + 0.5, draw.base[1] as f32 + 0.5],
                texel0: [draw.texel0[0] as f32, draw.texel0[1] as f32],
                size: [draw.size[0] as i32, draw.size[1] as i32],
                texels_per_px: draw.texels_per_px as f32,
                n_channels: self.n_channels,
                table_rows: self.table_rows,
                _pad: [0; 3],
                chan: self.chan,
            };
            let at = i * stride as usize;
            data[at..at + PARAMS_SIZE as usize].copy_from_slice(bytemuck::bytes_of(&params));
        }
        let uniforms = inner
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("imager-view tile params"),
                contents: &data,
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let k = kind_index(self.kind);
        let bind_groups = self
            .draws
            .iter()
            .map(|draw| {
                let views: Vec<wgpu::TextureView> = draw
                    .textures
                    .iter()
                    .take(MAX_GPU_CHANNELS)
                    .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
                    .collect();
                let mut entries = vec![
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &uniforms,
                            offset: 0,
                            size: NonZeroU64::new(PARAMS_SIZE),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&self.levels),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&self.lut),
                    },
                ];
                for c in 0..MAX_GPU_CHANNELS {
                    entries.push(wgpu::BindGroupEntry {
                        binding: c as u32 + 3,
                        resource: wgpu::BindingResource::TextureView(
                            views.get(c).unwrap_or(&inner.blanks[k]),
                        ),
                    });
                }
                inner.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("imager-view tile"),
                    layout: &inner.layouts[k],
                    entries: &entries,
                })
            })
            .collect();
        let prepared = Prepared {
            pipeline: self.gpu.pipeline(
                self.kind,
                self.n_channels as usize,
                self.picture,
                self.format,
            ),
            bind_groups,
            stride: stride as u32,
        };
        *self.prepared.lock().unwrap_or_else(|e| e.into_inner()) = Some(prepared);
    }

    /// Issues the draw calls into `pass`, whose target is `target` pixels
    /// in size, touching only pixels inside `clip`.
    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, target: [u32; 2], clip: [i32; 4]) {
        let prepared = self.prepared.lock().unwrap_or_else(|e| e.into_inner());
        let Some(prepared) = prepared.as_ref() else {
            return;
        };
        pass.set_viewport(0.0, 0.0, target[0] as f32, target[1] as f32, 0.0, 1.0);
        pass.set_pipeline(&prepared.pipeline);
        for (i, (draw, bind_group)) in self.draws.iter().zip(&prepared.bind_groups).enumerate() {
            let x0 = draw.scissor[0].max(clip[0]).max(self.bounds[0]).max(0);
            let y0 = draw.scissor[1].max(clip[1]).max(self.bounds[1]).max(0);
            let x1 = draw.scissor[2]
                .min(clip[2])
                .min(self.bounds[2])
                .min(target[0] as i32);
            let y1 = draw.scissor[3]
                .min(clip[3])
                .min(self.bounds[3])
                .min(target[1] as i32);
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            pass.set_scissor_rect(x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32);
            pass.set_bind_group(0, bind_group, &[i as u32 * prepared.stride]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Draws the frame into a new `width` x `height` target cleared to
    /// `background` and reads it back as RGBA bytes, row-major.
    pub fn render_offscreen(&self, width: u32, height: u32, background: [u8; 3]) -> Vec<u8> {
        debug_assert_eq!(self.format, OFFSCREEN_FORMAT);
        let inner = &self.gpu.0;
        let device = &inner.device;
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("imager-view offscreen target"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        // Rows of a texture-to-buffer copy must be padded to this alignment.
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded = (width * 4).div_ceil(align) * align;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("imager-view readback"),
            size: padded as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.prepare_draws();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let clear = |c: u8| c as f64 / 255.0;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("imager-view offscreen"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear(background[0]),
                            g: clear(background[1]),
                            b: clear(background[2]),
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.draw(
                &mut pass,
                [width, height],
                [0, 0, width as i32, height as i32],
            );
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            size,
        );
        inner.queue.submit([encoder.finish()]);

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        let mapped = device.poll(wgpu::PollType::wait_indefinitely()).is_ok()
            && matches!(rx.recv(), Ok(Ok(())));
        if mapped && let Ok(data) = slice.get_mapped_range() {
            for row in data.chunks(padded as usize) {
                out.extend_from_slice(&row[..(width * 4) as usize]);
            }
        }
        readback.unmap();
        out
    }
}

impl egui_wgpu::CallbackTrait for GpuFrame {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        self.prepare_draws();
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        let clip = info.clip_rect_in_pixels();
        self.draw(
            pass,
            info.screen_size_px,
            [
                clip.left_px,
                clip.top_px,
                clip.left_px + clip.width_px,
                clip.top_px + clip.height_px,
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_match_the_shader_layout() {
        // vec2 + vec2 + vec2 + f32 + 4 * u32 = 48 bytes, then 7 vec4.
        assert_eq!(PARAMS_SIZE, 48 + 16 * MAX_GPU_CHANNELS as u64);
        assert_eq!(std::mem::offset_of!(Params, chan), 48);
    }

    #[test]
    fn shaders_have_a_slot_per_channel() {
        for kind in KINDS {
            let src = shader_source(kind, 3);
            assert!(src.contains(&format!("var img{}:", MAX_GPU_CHANNELS - 1)));
            assert_eq!(src.matches("sum += colour(").count(), 3);
            assert_eq!(shader_source(kind, 0).matches("sum += colour(").count(), 0);
            assert!(src.contains("fn level("));
            assert_eq!(src.contains("fn fs_picture("), kind == SampleKind::Uint);
        }
    }
}
