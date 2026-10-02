//! wgpu sprite renderer for ANM draw lists.
//!
//! Everything is drawn into a 640x480 virtual canvas (the game's coordinate space),
//! letterboxed into the window at an integer-ish scale.

use std::collections::HashMap;
use std::sync::Arc;

use th_anm::{AnmLibrary, BlendMode, DrawList, Vertex};
use winit::window::Window;

pub const CANVAS_W: f32 = 640.0;
pub const CANVAS_H: f32 = 480.0;

const SHADER: &str = r#"
struct Globals { canvas: vec2<f32>, _pad: vec2<f32> };
@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> VOut {
    var o: VOut;
    o.pos = vec4<f32>(pos.x / g.canvas.x * 2.0 - 1.0, 1.0 - pos.y / g.canvas.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    o.color = color;
    return o;
}

@fragment
fn fs(i: VOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, i.uv) * i.color;
}
"#;

pub struct Renderer {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipelines: HashMap<BlendMode, wgpu::RenderPipeline>,
    globals: wgpu::BindGroup,
    tex_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: wgpu::BindGroup,
    textures: HashMap<u32, wgpu::BindGroup>,
    vbuf: wgpu::Buffer,
    vbuf_cap: u64,
}

fn blend_state(mode: BlendMode) -> wgpu::BlendState {
    use wgpu::{BlendComponent as C, BlendFactor as F, BlendOperation as O};
    let c = |src, dst, operation| C { src_factor: src, dst_factor: dst, operation };
    let color = match mode {
        BlendMode::Normal => c(F::SrcAlpha, F::OneMinusSrcAlpha, O::Add),
        BlendMode::Add => c(F::SrcAlpha, F::One, O::Add),
        BlendMode::Subtract => c(F::SrcAlpha, F::One, O::ReverseSubtract),
        BlendMode::Replace => c(F::One, F::Zero, O::Add),
        BlendMode::Screen => c(F::OneMinusDst, F::OneMinusSrc, O::Add),
        BlendMode::Multiply => c(F::Dst, F::Zero, O::Add),
        BlendMode::Weird6 => c(F::OneMinusSrc, F::OneMinusSrcAlpha, O::Add),
        BlendMode::Behind => c(F::DstAlpha, F::OneMinusDstAlpha, O::Add),
        BlendMode::Min => c(F::One, F::One, O::Min),
        BlendMode::Max => c(F::One, F::One, O::Max),
    };
    let alpha = match mode {
        BlendMode::Min | BlendMode::Max => c(F::One, F::One, O::Max),
        _ => c(F::One, F::OneMinusSrcAlpha, O::Add),
    };
    wgpu::BlendState { color, alpha }
}

impl Renderer {
    pub fn new(window: Arc<Window>) -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(window.clone()).expect("create surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("no suitable GPU adapter");
        log::info!("GPU: {:?}", adapter.get_info());
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("request device");

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface unsupported by adapter");
        // ZUN's Direct3D 9 renderer blends in gamma space, so prefer a non-sRGB target.
        let caps = surface.get_capabilities(&adapter);
        if let Some(f) = caps.formats.iter().find(|f| !f.is_srgb()) {
            config.format = *f;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);

        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
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
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&ubuf, 0, bytemuck::cast_slice(&[CANVAS_W, CANVAS_H, 0.0, 0.0]));
        let globals = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sprite"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprite"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprite"),
            bind_group_layouts: &[Some(&globals_layout), Some(&tex_layout)],
            immediate_size: 0,
        });
        let attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];
        let mut pipelines = HashMap::new();
        for mode in [
            BlendMode::Normal,
            BlendMode::Add,
            BlendMode::Subtract,
            BlendMode::Replace,
            BlendMode::Screen,
            BlendMode::Multiply,
            BlendMode::Weird6,
            BlendMode::Behind,
            BlendMode::Min,
            BlendMode::Max,
        ] {
            let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("sprite"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attrs,
                    })],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(blend_state(mode)),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
            pipelines.insert(mode, p);
        }

        let vbuf_cap = 1 << 20;
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vertices"),
            size: vbuf_cap,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let white = make_texture(&device, &queue, &tex_layout, &sampler, 1, 1, &[255, 255, 255, 255]);
        Self {
            window,
            surface,
            device,
            queue,
            config,
            pipelines,
            globals,
            white,
            tex_layout,
            sampler,
            textures: HashMap::new(),
            vbuf,
            vbuf_cap,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    fn make_texture(&self, width: u32, height: u32, rgba: &[u8]) -> wgpu::BindGroup {
        make_texture(&self.device, &self.queue, &self.tex_layout, &self.sampler, width, height, rgba)
    }

    /// Drop GPU copies of textures (e.g. when switching to another set of files).
    pub fn forget_textures(&mut self) {
        self.textures.clear();
    }

    pub fn render(&mut self, lib: &AnmLibrary, list: &DrawList) {
        // Upload any textures used for the first time.
        for b in list {
            if let Some(t) = b.texture {
                if !self.textures.contains_key(&t) {
                    if let Some(img) = lib.textures.get(t as usize) {
                        let bg = self.make_texture(img.width, img.height, &img.rgba);
                        self.textures.insert(t, bg);
                    }
                }
            }
        }

        let total: usize = list.iter().map(|b| b.vertices.len()).sum();
        let bytes = (total * std::mem::size_of::<Vertex>()) as u64;
        if bytes > self.vbuf_cap {
            self.vbuf_cap = bytes.next_power_of_two();
            self.vbuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vertices"),
                size: self.vbuf_cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        let mut offset = 0u64;
        let mut ranges = Vec::with_capacity(list.len());
        for b in list {
            let data: &[u8] = bytemuck::cast_slice(&b.vertices);
            self.queue.write_buffer(&self.vbuf, offset, data);
            ranges.push((offset, b.vertices.len() as u32));
            offset += data.len() as u64;
        }

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };
        let view = frame.texture.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            // Letterbox the 4:3 canvas.
            let (ww, wh) = (self.config.width as f32, self.config.height as f32);
            let scale = (ww / CANVAS_W).min(wh / CANVAS_H);
            let (vw, vh) = (CANVAS_W * scale, CANVAS_H * scale);
            pass.set_viewport((ww - vw) * 0.5, (wh - vh) * 0.5, vw, vh, 0.0, 1.0);
            pass.set_scissor_rect(
                ((ww - vw) * 0.5) as u32,
                ((wh - vh) * 0.5) as u32,
                (vw as u32).max(1),
                (vh as u32).max(1),
            );

            pass.set_bind_group(0, &self.globals, &[]);
            for (b, &(off, count)) in list.iter().zip(&ranges) {
                let tex = b.texture.and_then(|t| self.textures.get(&t)).unwrap_or(&self.white);
                pass.set_pipeline(&self.pipelines[&b.blend]);
                pass.set_bind_group(1, tex, &[]);
                pass.set_vertex_buffer(0, self.vbuf.slice(off..off + count as u64 * std::mem::size_of::<Vertex>() as u64));
                pass.draw(0..count, 0..1);
            }
        }
        self.queue.submit([enc.finish()]);
        self.window.pre_present_notify();
        self.queue.present(frame);
    }
}

fn make_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> wgpu::BindGroup {
    let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
        size,
    );
    let view = tex.create_view(&Default::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}
