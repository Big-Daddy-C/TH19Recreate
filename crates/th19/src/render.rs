//! wgpu sprite renderer for ANM draw lists.
//!
//! Everything is drawn into a 640x480 virtual canvas (the game's coordinate space),
//! letterboxed into the window at an integer-ish scale.

use std::collections::HashMap;
use std::sync::Arc;

use th_anm::{AnmLibrary, BlendMode, DepthMode, DrawList, Vertex};
use winit::window::Window;

pub const CANVAS_W: f32 = 640.0;
pub const CANVAS_H: f32 = 480.0;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

const SHADER: &str = r#"
struct Globals { canvas: vec2<f32>, _pad: vec2<f32>, fog: vec4<f32> };
@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) fog: f32,
};

@vertex
fn vs(
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) w: f32,
    @location(4) fog: f32,
    @location(5) depth: f32,
) -> VOut {
    var o: VOut;
    // `pos` is already perspective-divided; multiply back by w so the GPU
    // interpolates UVs perspective-correctly for 3D stage quads.
    let ndc = vec2<f32>(pos.x / g.canvas.x * 2.0 - 1.0, 1.0 - pos.y / g.canvas.y * 2.0);
    o.pos = vec4<f32>(ndc * w, depth * w, w);
    o.uv = uv;
    o.color = color;
    o.fog = fog;
    return o;
}

fn shade(i: VOut, alpha_ref: f32) -> vec4<f32> {
    let c = textureSample(tex, samp, i.uv) * i.color;
    // Like Direct3D alpha testing: (nearly) transparent texels must not occlude via the depth buffer.
    if (c.a < alpha_ref) {
        discard;
    }
    return vec4<f32>(mix(c.rgb, g.fog.rgb, i.fog), c.a);
}

@fragment
fn fs(i: VOut) -> @location(0) vec4<f32> {
    return shade(i, 0.004);
}

// th19.exe enables alpha testing for all drawing with reference 1 and "greater or equal",
// i.e. only fully transparent texels are discarded, depth-writing or not.
@fragment
fn fs_depth(i: VOut) -> @location(0) vec4<f32> {
    return shade(i, 1.0 / 255.0);
}
"#;

pub struct Renderer {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipelines: HashMap<(BlendMode, DepthMode), wgpu::RenderPipeline>,
    /// Depth buffers by size.
    depth: HashMap<(u32, u32), wgpu::TextureView>,
    /// Offscreen stage image: view, bind group, size.
    scene: Option<(wgpu::TextureView, wgpu::BindGroup, u32, u32)>,
    clamp_sampler: wgpu::Sampler,
    globals: wgpu::BindGroup,
    globals_buf: wgpu::Buffer,
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
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
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
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&ubuf, 0, bytemuck::cast_slice(&[CANVAS_W, CANVAS_H, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]));
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

        let clamp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene"),
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
        let attrs =
            wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32, 4 => Float32, 5 => Float32];
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
        ]
        .into_iter()
        .flat_map(|b| [DepthMode::Off, DepthMode::Test, DepthMode::TestWrite].map(|d| (b, d)))
        {
            let (mode, depth) = mode;
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
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth == DepthMode::TestWrite),
                    depth_compare: Some(if depth == DepthMode::Off {
                        wgpu::CompareFunction::Always
                    } else {
                        wgpu::CompareFunction::LessEqual
                    }),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(if depth == DepthMode::TestWrite { "fs_depth" } else { "fs" }),
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
            pipelines.insert((mode, depth), p);
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
            depth: HashMap::new(),
            scene: None,
            clamp_sampler,
            globals,
            globals_buf: ubuf,
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

    /// Draw one frame to the window. `list[..scene_end]` is the stage scene; batches after it
    /// may sample that scene through [`th_anm::SCENE_TEXTURE`]. `background` fills the canvas
    /// (0..1 RGB); `fog` is the stage fog colour.
    pub fn render(&mut self, lib: &AnmLibrary, list: &DrawList, scene_end: usize, background: [f32; 3], fog: [f32; 3]) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };
        let view = frame.texture.create_view(&Default::default());
        let (w, h) = (self.config.width, self.config.height);
        let enc = self.draw_into(lib, list, scene_end, background, fog, &view, w, h);
        self.queue.submit([enc.finish()]);
        self.window.pre_present_notify();
        self.queue.present(frame);
    }

    /// Render one frame offscreen at `width`x`height` and return it as RGBA8 pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn screenshot(
        &mut self,
        lib: &AnmLibrary,
        list: &DrawList,
        scene_end: usize,
        background: [f32; 3],
        fog: [f32; 3],
        width: u32,
        height: u32,
    ) -> Vec<u8> {
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screenshot"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let mut enc = self.draw_into(lib, list, scene_end, background, fog, &view, width, height);
        let row = (width * 4).next_multiple_of(256);
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot"),
            size: (row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let data = slice.get_mapped_range().expect("map screenshot buffer");
        let bgra = matches!(self.config.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height as usize {
            for px in data[y * row as usize..][..width as usize * 4].chunks_exact(4) {
                let (r, b) = if bgra { (px[2], px[0]) } else { (px[0], px[2]) };
                out.extend_from_slice(&[r, px[1], b, 255]);
            }
        }
        out
    }

    fn depth_view(&mut self, w: u32, h: u32) -> wgpu::TextureView {
        if let Some(v) = self.depth.get(&(w, h)) {
            return v.clone();
        }
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let v = tex.create_view(&Default::default());
        self.depth.insert((w, h), v.clone());
        v
    }

    /// The offscreen stage image (canvas-shaped), recreated when its size changes.
    fn scene_target(&mut self, w: u32, h: u32) -> (wgpu::TextureView, wgpu::BindGroup) {
        if !matches!(&self.scene, Some((_, _, sw, sh)) if *sw == w && *sh == h) {
            let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("scene"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("scene"),
                layout: &self.tex_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.clamp_sampler) },
                ],
            });
            self.scene = Some((view, bind, w, h));
        }
        let (v, b, _, _) = self.scene.as_ref().unwrap();
        (v.clone(), b.clone())
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_into(
        &mut self,
        lib: &AnmLibrary,
        list: &DrawList,
        scene_end: usize,
        background: [f32; 3],
        fog: [f32; 3],
        view: &wgpu::TextureView,
        target_w: u32,
        target_h: u32,
    ) -> wgpu::CommandEncoder {
        self.queue.write_buffer(
            &self.globals_buf,
            0,
            bytemuck::cast_slice(&[CANVAS_W, CANVAS_H, 0.0, 0.0, fog[0], fog[1], fog[2], 1.0]),
        );
        let solid = |color: [f32; 4]| th_anm::DrawBatch {
            texture: None,
            blend: BlendMode::Replace,
            depth: DepthMode::Off,
            layer: i32::MIN,
            clip: None,
            vertices: canvas_quad(color),
        };
        let bg = solid([background[0], background[1], background[2], 1.0]);
        // The captured stage image, pasted back onto the canvas before everything else.
        let mut composite = solid([1.0; 4]);
        composite.texture = Some(th_anm::SCENE_TEXTURE);
        for v in &mut composite.vertices {
            v.uv = [v.pos[0] / CANVAS_W, v.pos[1] / CANVAS_H];
        }
        let scene_end = scene_end.min(list.len());
        let use_scene = scene_end > 0;

        // Batches in draw order; `split` is where the offscreen scene pass ends.
        let mut all: Vec<&th_anm::DrawBatch> = vec![&bg];
        all.extend(list[..scene_end].iter());
        let split = all.len();
        if use_scene {
            all.push(&composite);
        }
        all.extend(list[scene_end..].iter());

        // Upload any textures used for the first time.
        for b in &all {
            if let Some(t) = b.texture {
                if t != th_anm::SCENE_TEXTURE && !self.textures.contains_key(&t) {
                    if let Some(img) = lib.textures.get(t as usize) {
                        let bg = self.make_texture(img.width, img.height, &img.rgba);
                        self.textures.insert(t, bg);
                    }
                }
            }
        }

        let total: usize = all.iter().map(|b| b.vertices.len()).sum();
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
        let mut ranges = Vec::with_capacity(all.len());
        for b in &all {
            let data: &[u8] = bytemuck::cast_slice(&b.vertices);
            self.queue.write_buffer(&self.vbuf, offset, data);
            ranges.push((offset, b.vertices.len() as u32));
            offset += data.len() as u64;
        }

        // Letterbox the 4:3 canvas into the target.
        let (ww, wh) = (target_w as f32, target_h as f32);
        let scale = (ww / CANVAS_W).min(wh / CANVAS_H);
        let (vw, vh) = (CANVAS_W * scale, CANVAS_H * scale);
        let (ox, oy) = ((ww - vw) * 0.5, (wh - vh) * 0.5);

        let (scene_w, scene_h) = ((vw.round() as u32).max(1), (vh.round() as u32).max(1));
        let scene = if use_scene { Some(self.scene_target(scene_w, scene_h)) } else { None };
        let target_depth = self.depth_view(target_w, target_h);
        let scene_depth = if use_scene { Some(self.depth_view(scene_w, scene_h)) } else { None };

        let mut enc = self.device.create_command_encoder(&Default::default());
        // Pass 1 (optional): the stage scene into the offscreen image.
        if let (Some((scene_view, _)), Some(depth)) = (&scene, &scene_depth) {
            let geom = Geometry { ox: 0.0, oy: 0.0, scale: scene_w as f32 / CANVAS_W, w: scene_w as f32, h: scene_h as f32 };
            self.encode_pass(&mut enc, scene_view, depth, &all[..split], &ranges[..split], geom, None);
        }
        // Pass 2: everything else onto the target (starting with the scene image, if any).
        let geom = Geometry { ox, oy, scale, w: ww, h: wh };
        let (from, scene_bind) = if use_scene { (split, scene.as_ref().map(|s| &s.1)) } else { (0, None) };
        self.encode_pass(&mut enc, view, &target_depth, &all[from..], &ranges[from..], geom, scene_bind);
        enc
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_pass(
        &self,
        enc: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        batches: &[&th_anm::DrawBatch],
        ranges: &[(u64, u32)],
        g: Geometry,
        scene: Option<&wgpu::BindGroup>,
    ) {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("canvas"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let (vw, vh) = (CANVAS_W * g.scale, CANVAS_H * g.scale);
        pass.set_viewport(g.ox, g.oy, vw, vh, 0.0, 1.0);
        // Canvas rectangle -> target pixels, clamped to the target.
        let scissor = |r: [f32; 4]| {
            let x0 = (g.ox + r[0] * g.scale).clamp(0.0, g.w);
            let y0 = (g.oy + r[1] * g.scale).clamp(0.0, g.h);
            let x1 = (g.ox + (r[0] + r[2]) * g.scale).clamp(0.0, g.w);
            let y1 = (g.oy + (r[1] + r[3]) * g.scale).clamp(0.0, g.h);
            (x0 as u32, y0 as u32, ((x1 - x0) as u32).max(1), ((y1 - y0) as u32).max(1))
        };
        let full = [0.0, 0.0, CANVAS_W, CANVAS_H];
        pass.set_bind_group(0, &self.globals, &[]);
        for (b, &(off, count)) in batches.iter().zip(ranges) {
            if count == 0 {
                continue;
            }
            let (sx, sy, sw, sh) = scissor(b.clip.unwrap_or(full));
            pass.set_scissor_rect(sx, sy, sw, sh);
            let tex = match b.texture {
                Some(th_anm::SCENE_TEXTURE) => scene.unwrap_or(&self.white),
                Some(t) => self.textures.get(&t).unwrap_or(&self.white),
                None => &self.white,
            };
            pass.set_pipeline(&self.pipelines[&(b.blend, b.depth)]);
            pass.set_bind_group(1, tex, &[]);
            pass.set_vertex_buffer(0, self.vbuf.slice(off..off + count as u64 * std::mem::size_of::<Vertex>() as u64));
            pass.draw(0..count, 0..1);
        }
    }
}

/// Where the 640x480 canvas lands in a render target.
#[derive(Clone, Copy)]
struct Geometry {
    ox: f32,
    oy: f32,
    scale: f32,
    w: f32,
    h: f32,
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

fn canvas_quad(color: [f32; 4]) -> Vec<Vertex> {
    let v = |x: f32, y: f32| Vertex { pos: [x, y], uv: [0.0; 2], color, w: 1.0, fog: 0.0, depth: 0.0 };
    vec![v(0.0, 0.0), v(CANVAS_W, 0.0), v(CANVAS_W, CANVAS_H), v(0.0, 0.0), v(CANVAS_W, CANVAS_H), v(0.0, CANVAS_H)]
}
