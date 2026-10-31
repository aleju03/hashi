use std::sync::Arc;

use bytemuck::{Pod, Zeroable};

const SHADER: &str = r#"
struct U { size: vec2<f32> };
@group(0) @binding(0) var<uniform> u: U;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct V {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) shape: vec4<f32>,
    @location(3) radius: f32,
};

@vertex
fn vs(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>, @location(3) shape: vec4<f32>, @location(4) radius: f32) -> V {
    var v: V;
    v.pos = vec4<f32>(pos.x / u.size.x * 2.0 - 1.0, 1.0 - pos.y / u.size.y * 2.0, 0.0, 1.0);
    v.uv = uv;
    v.color = color;
    v.shape = shape;
    v.radius = radius;
    return v;
}

@fragment
fn fs(v: V) -> @location(0) vec4<f32> {
    var c = textureSample(tex, samp, v.uv) * v.color;
    // a rounded rectangle: shape is the point from its centre and its half size
    if (v.radius > 0.0) {
        let q = abs(v.shape.xy) - v.shape.zw + vec2<f32>(v.radius);
        let d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - v.radius;
        c = c * clamp(0.5 - d, 0.0, 1.0);
    }
    return c;
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    shape: [f32; 4],
    radius: f32,
}

pub type TexId = usize;

pub struct Tex {
    bind: wgpu::BindGroup,
}

#[derive(Clone, Copy, PartialEq)]
struct Batch {
    tex: TexId,
    add: bool,
    start: u32,
    count: u32,
}

// one frame's draws, in pixels
#[derive(Default)]
pub struct Frame {
    verts: Vec<Vertex>,
    batches: Vec<Batch>,
}

pub const WHITE: TexId = 0;

impl Frame {
    fn push(&mut self, tex: TexId, add: bool, vs: &[Vertex]) {
        let start = self.verts.len() as u32;
        self.verts.extend_from_slice(vs);
        match self.batches.last_mut() {
            Some(b) if b.tex == tex && b.add == add => b.count += vs.len() as u32,
            _ => self.batches.push(Batch { tex, add, start, count: vs.len() as u32 }),
        }
    }

    // [x0, y0, x1, y1] on screen, uv the same corners of the texture
    pub fn quad(&mut self, tex: TexId, add: bool, r: [f32; 4], uv: [f32; 4], color: [f32; 4]) {
        let v = |x: f32, y: f32, u: f32, w: f32| Vertex { pos: [x, y], uv: [u, w], color, shape: [0.0; 4], radius: 0.0 };
        let (a, b, c, d) = (v(r[0], r[1], uv[0], uv[1]), v(r[2], r[1], uv[2], uv[1]), v(r[2], r[3], uv[2], uv[3]), v(r[0], r[3], uv[0], uv[3]));
        self.push(tex, add, &[a, b, c, a, c, d]);
    }

    pub fn fill(&mut self, r: [f32; 4], color: [f32; 4]) {
        self.quad(WHITE, false, r, [0.0, 0.0, 1.0, 1.0], color);
    }

    pub fn round_rect(&mut self, r: [f32; 4], radius: f32, color: [f32; 4]) {
        let (cx, cy, hw, hh) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0, (r[2] - r[0]) / 2.0, (r[3] - r[1]) / 2.0);
        // a pixel more around it for the edge's smoothing
        let (x0, y0, x1, y1) = (r[0] - 1.0, r[1] - 1.0, r[2] + 1.0, r[3] + 1.0);
        let radius = radius.max(0.01);
        let v = |x: f32, y: f32| Vertex { pos: [x, y], uv: [0.5, 0.5], color, shape: [x - cx, y - cy, hw, hh], radius };
        let (a, b, c, d) = (v(x0, y0), v(x1, y0), v(x1, y1), v(x0, y1));
        self.push(WHITE, false, &[a, b, c, a, c, d]);
    }

    pub fn triangle(&mut self, p: [[f32; 2]; 3], color: [f32; 4]) {
        let v = |q: [f32; 2]| Vertex { pos: q, uv: [0.5, 0.5], color, shape: [0.0; 4], radius: 0.0 };
        self.push(WHITE, false, &[v(p[0]), v(p[1]), v(p[2])]);
    }
}

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    surface: Option<(wgpu::Surface<'static>, wgpu::SurfaceConfiguration)>,
    adapter: wgpu::Adapter,
    format: wgpu::TextureFormat,
    samples: u32,
    msaa: Option<(wgpu::TextureView, u32, u32)>,
    normal: wgpu::RenderPipeline,
    add: wgpu::RenderPipeline,
    tex_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
    textures: Vec<Option<Tex>>,
    vbuf: Option<(wgpu::Buffer, u64)>,
}

impl Gpu {
    // for a window, or None for drawing into an image
    pub fn new(window: Option<Arc<winit::window::Window>>, vsync: bool) -> Result<Gpu, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = match &window {
            Some(w) => Some(instance.create_surface(w.clone()).map_err(|e| e.to_string())?),
            None => None,
        };
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: surface.as_ref(),
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| e.to_string())?;
        // the screen's own values, not linear light, which is how a canvas blends
        let format = match &surface {
            Some(s) => {
                let caps = s.get_capabilities(&adapter);
                caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0])
            }
            None => wgpu::TextureFormat::Rgba8Unorm,
        };
        let samples = if adapter.get_texture_format_features(format).flags.sample_count_supported(4) { 4 } else { 1 };

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&uniform_layout), Some(&tex_layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Float32];
        let pipeline = |blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Vertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes })],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let over = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add };
        let plus = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let normal = pipeline(wgpu::BlendState { color: over, alpha: over });
        let add = pipeline(wgpu::BlendState { color: plus, alpha: plus });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        let mut gpu = Gpu {
            device,
            queue,
            surface: None,
            adapter,
            format,
            samples,
            msaa: None,
            normal,
            add,
            tex_layout,
            sampler,
            uniform,
            uniform_bind,
            textures: vec![],
            vbuf: None,
        };
        gpu.texture(1, 1, &[255, 255, 255, 255]);
        if let (Some(s), Some(w)) = (surface, window) {
            let size = w.inner_size();
            let config = s.get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1)).ok_or("This screen can't be drawn on.")?;
            gpu.surface = Some((s, wgpu::SurfaceConfiguration { format, ..config }));
            gpu.set_vsync(vsync);
        }
        Ok(gpu)
    }

    // Fifo waits for the screen; otherwise the newest frame replaces a waiting one (Mailbox), or
    // goes straight out
    pub fn set_vsync(&mut self, vsync: bool) {
        let Some((surface, config)) = &mut self.surface else { return };
        let modes = surface.get_capabilities(&self.adapter).present_modes;
        config.present_mode = if vsync {
            wgpu::PresentMode::Fifo
        } else {
            [wgpu::PresentMode::Mailbox, wgpu::PresentMode::Immediate].into_iter().find(|m| modes.contains(m)).unwrap_or(wgpu::PresentMode::Fifo)
        };
        config.desired_maximum_frame_latency = if vsync { 2 } else { 1 };
        surface.configure(&self.device, config);
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let Some((surface, config)) = &mut self.surface else { return };
        config.width = width.max(1);
        config.height = height.max(1);
        surface.configure(&self.device, config);
    }

    pub fn size(&self) -> (u32, u32) {
        self.surface.as_ref().map_or((1, 1), |(_, c)| (c.width, c.height))
    }

    // premultiplied RGBA, with every smaller level made from it
    pub fn texture(&mut self, width: u32, height: u32, rgba: &[u8]) -> TexId {
        let levels = 32 - width.max(height).max(1).leading_zeros();
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let (mut w, mut h, mut data) = (width, height, rgba.to_vec());
        for level in 0..levels {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            if level + 1 < levels {
                (data, w, h) = half(&data, w, h);
            }
        }
        let view = texture.create_view(&Default::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.tex_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) }],
        });
        let tex = Tex { bind };
        match self.textures.iter().position(Option::is_none) {
            Some(i) => {
                self.textures[i] = Some(tex);
                i
            }
            None => {
                self.textures.push(Some(tex));
                self.textures.len() - 1
            }
        }
    }

    pub fn drop_texture(&mut self, id: TexId) {
        if id != WHITE {
            if let Some(t) = self.textures.get_mut(id) {
                *t = None;
            }
        }
    }

    fn encode(&mut self, frame: &Frame, target: &wgpu::TextureView, width: u32, height: u32) -> wgpu::CommandEncoder {
        self.queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&[width as f32, height as f32, 0.0, 0.0]));
        let bytes = bytemuck::cast_slice::<Vertex, u8>(&frame.verts);
        let need = (bytes.len() as u64).max(64);
        if self.vbuf.as_ref().is_none_or(|(_, size)| *size < need) {
            let size = need.next_power_of_two();
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            self.vbuf = Some((buf, size));
        }
        let vbuf = &self.vbuf.as_ref().unwrap().0;
        if !bytes.is_empty() {
            self.queue.write_buffer(vbuf, 0, bytes);
        }
        if self.samples > 1 && self.msaa.as_ref().is_none_or(|(_, w, h)| (*w, *h) != (width, height)) {
            let t = self.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: self.samples,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            self.msaa = Some((t.create_view(&Default::default()), width, height));
        }
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let (view, resolve) = match &self.msaa {
                Some((m, _, _)) if self.samples > 1 => (m, Some(target)),
                _ => (target, None),
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: resolve,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_vertex_buffer(0, vbuf.slice(..));
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            let mut add = None;
            for b in &frame.batches {
                let Some(tex) = self.textures.get(b.tex).and_then(Option::as_ref) else { continue };
                if add != Some(b.add) {
                    pass.set_pipeline(if b.add { &self.add } else { &self.normal });
                    add = Some(b.add);
                }
                pass.set_bind_group(1, &tex.bind, &[]);
                pass.draw(b.start..b.start + b.count, 0..1);
            }
        }
        encoder
    }

    // false when the screen wants to be set up again
    pub fn present(&mut self, frame: &Frame) -> bool {
        let Some((surface, config)) = &self.surface else { return false };
        let (width, height) = (config.width, config.height);
        let texture = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return true,
            _ => return false,
        };
        let view = texture.texture.create_view(&Default::default());
        let encoder = self.encode(frame, &view, width, height);
        self.queue.submit([encoder.finish()]);
        self.queue.present(texture);
        true
    }

    pub fn reconfigure(&mut self) {
        if let Some((surface, config)) = &self.surface {
            surface.configure(&self.device, config);
        }
    }

    // for --shot, without a window
    pub fn image(&mut self, frame: &Frame, width: u32, height: u32) -> Vec<u8> {
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let mut encoder = self.encode(frame, &view, width, height);
        let row = (width * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: u64::from(row * height), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) } },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let Ok(data) = slice.get_mapped_range() else { return vec![0; (width * height * 4) as usize] };
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            out.extend_from_slice(&data[(y * row) as usize..(y * row + width * 4) as usize]);
        }
        out
    }
}

fn half(data: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let mut sum = [0u32; 4];
            let mut n = 0;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy) = ((x * 2 + dx).min(w - 1), (y * 2 + dy).min(h - 1));
                let i = ((sy * w + sx) * 4) as usize;
                for c in 0..4 {
                    sum[c] += u32::from(data[i + c]);
                }
                n += 1;
            }
            let o = ((y * nw + x) * 4) as usize;
            for c in 0..4 {
                out[o + c] = ((sum[c] + n / 2) / n) as u8;
            }
        }
    }
    (out, nw, nh)
}
