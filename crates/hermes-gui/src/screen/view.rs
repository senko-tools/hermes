//! Drawing a screen share on the GPU with iced's `shader` widget. Each share keeps one set of
//! textures (the Y, U and V planes for I420; the Y and the interleaved U/V plane for NV12),
//! overwritten in place when a new picture arrives, and the fragment shader turns YUV into
//! colors. Showing pictures as images instead would create (and drop) a GPU texture for every
//! frame and convert every pixel on the CPU first.
//!
//! Without a GPU (iced's software renderer) shares are not drawn.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use iced::widget::shader::{self, Viewport};
use iced::{Rectangle, mouse, wgpu};

use super::codec::Picture;

/// The latest picture of one share and how many came before it.
#[derive(Debug, Clone)]
pub struct Share {
    /// Identifies the share's textures on the GPU.
    pub key: u64,
    pub picture: Arc<Picture>,
    /// Counts pictures, so the GPU copy is only refreshed for a new one.
    pub version: u64,
}

impl Share {
    pub fn new(id: &impl Hash, picture: Arc<Picture>) -> Share {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        id.hash(&mut hasher);
        Share {
            key: hasher.finish(),
            picture,
            version: 0,
        }
    }

    pub fn update(&mut self, picture: Arc<Picture>) {
        self.picture = picture;
        self.version += 1;
    }
}

impl<Message> shader::Program<Message> for Share {
    type State = ();
    type Primitive = Primitive;

    fn draw(&self, _state: &(), _cursor: mouse::Cursor, _bounds: Rectangle) -> Primitive {
        Primitive(self.clone())
    }
}

#[derive(Debug)]
pub struct Primitive(Share);

impl shader::Primitive for Primitive {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        _viewport: &Viewport,
    ) {
        pipeline.prepare(device, queue, &self.0, bounds.size());
    }

    fn draw(&self, pipeline: &Pipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some(entry) = pipeline.entries.get(&self.0.key) else {
            return true;
        };
        render_pass.set_pipeline(&pipeline.pipeline);
        render_pass.set_bind_group(0, &entry.bind_group, &[]);
        render_pass.draw(0..4, 0..1);
        true
    }
}

/// The GPU side of one share.
struct Entry {
    size: (u32, u32),
    nv12: bool,
    version: u64,
    /// Y, U, V (I420) or Y, UV (NV12).
    planes: Vec<wgpu::Texture>,
    params: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

pub struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The target stores linear colors (an sRGB format): the shader decodes its output.
    srgb_target: bool,
    entries: HashMap<u64, Entry>,
    /// Shares drawn since the last trim; the others' textures are dropped.
    used: HashSet<u64>,
    /// New pictures uploaded and the time it took, logged every [`STATS_EVERY`] while shares
    /// are drawn: the last step between a decoded picture and the screen.
    uploads: u32,
    upload_us: u64,
    stats_since: std::time::Instant,
}

/// How often the drawing numbers are logged.
const STATS_EVERY: std::time::Duration = std::time::Duration::from_secs(10);

const SHADER: &str = r"
struct Params {
    scale: vec2<f32>,
    srgb: f32,
    // 1: U and V are interleaved in `u_plane` (NV12).
    nv12: f32,
};

@group(0) @binding(0) var y_plane: texture_2d<f32>;
@group(0) @binding(1) var u_plane: texture_2d<f32>;
@group(0) @binding(2) var v_plane: texture_2d<f32>;
@group(0) @binding(3) var linear_sampler: sampler;
@group(0) @binding(4) var<uniform> params: Params;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// A quad (triangle strip) centered in the widget, scaled to keep the picture's aspect ratio.
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Varyings {
    let corner = vec2<f32>(f32(index & 1u), f32(index >> 1u));
    var out: Varyings;
    out.position = vec4<f32>(
        (corner.x * 2.0 - 1.0) * params.scale.x,
        (1.0 - corner.y * 2.0) * params.scale.y,
        0.0,
        1.0,
    );
    out.uv = corner;
    return out;
}

// Limited-range BT.601, as OpenH264 encodes.
@fragment
fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
    let y = (textureSample(y_plane, linear_sampler, in.uv).r * 255.0 - 16.0) * 1.164 / 255.0;
    let chroma = textureSample(u_plane, linear_sampler, in.uv);
    var raw_v = textureSample(v_plane, linear_sampler, in.uv).r;
    if params.nv12 > 0.5 {
        raw_v = chroma.g;
    }
    let u = (chroma.r * 255.0 - 128.0) / 255.0;
    let v = (raw_v * 255.0 - 128.0) / 255.0;
    var rgb = clamp(
        vec3<f32>(y + 1.596 * v, y - 0.391 * u - 0.813 * v, y + 2.018 * u),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    if params.srgb > 0.5 {
        rgb = pow(rgb, vec3<f32>(2.2));
    }
    return vec4<f32>(rgb, 1.0);
}
";

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Pipeline {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hermes screen share"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let plane = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hermes screen share"),
            entries: &[
                plane(0),
                plane(1),
                plane(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hermes screen share"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hermes screen share"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hermes screen share"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        Pipeline {
            pipeline,
            layout,
            sampler,
            srgb_target: format.is_srgb(),
            entries: HashMap::new(),
            used: HashSet::new(),
            uploads: 0,
            upload_us: 0,
            stats_since: std::time::Instant::now(),
        }
    }

    fn trim(&mut self) {
        let used = std::mem::take(&mut self.used);
        self.entries.retain(|key, _| used.contains(key));
    }
}

impl Pipeline {
    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        share: &Share,
        bounds: iced::Size,
    ) {
        self.used.insert(share.key);
        let picture = &share.picture;
        let size = (picture.width, picture.height);
        let stale = self
            .entries
            .get(&share.key)
            .is_none_or(|e| e.size != size || e.nv12 != picture.nv12);
        if stale {
            let entry = self.entry(device, size, picture.nv12);
            self.entries.insert(share.key, entry);
        }
        let shares = self.entries.len();
        let Some(entry) = self.entries.get_mut(&share.key) else {
            return;
        };
        if stale || entry.version != share.version {
            let begin = std::time::Instant::now();
            let (cw, ch) = picture.chroma_size();
            // Bytes per row and rows of each plane (an NV12 chroma row has two bytes a sample).
            let planes = if picture.nv12 {
                vec![(picture.y(), size.0, size.1), (picture.u(), cw * 2, ch)]
            } else {
                vec![
                    (picture.y(), size.0, size.1),
                    (picture.u(), cw, ch),
                    (picture.v(), cw, ch),
                ]
            };
            for (texture, (data, row, h)) in entry.planes.iter().zip(planes) {
                let w = texture.width();
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(h),
                    },
                    extent(w, h),
                );
            }
            entry.version = share.version;
            self.uploads += 1;
            self.upload_us += u64::try_from(begin.elapsed().as_micros()).unwrap_or(u64::MAX);
        }
        let elapsed = self.stats_since.elapsed();
        if elapsed >= STATS_EVERY {
            let secs = elapsed.as_secs_f64();
            tracing::info!(
                "screen shares drawn: {:.1} new pictures/s over {} share(s), {:.2} ms to upload each",
                f64::from(self.uploads) / secs,
                shares,
                self.upload_us as f64 / 1000.0 / f64::from(self.uploads.max(1)),
            );
            self.uploads = 0;
            self.upload_us = 0;
            self.stats_since = std::time::Instant::now();
        }
        // Fit the picture into the widget (letterboxed), every frame: the widget can resize.
        let (sx, sy) = fit_scale(size, bounds);
        let srgb = if self.srgb_target { 1.0f32 } else { 0.0 };
        let nv12 = if picture.nv12 { 1.0f32 } else { 0.0 };
        let mut params = Vec::with_capacity(16);
        for value in [sx, sy, srgb, nv12] {
            params.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&entry.params, 0, &params);
    }

    fn entry(&self, device: &wgpu::Device, (width, height): (u32, u32), nv12: bool) -> Entry {
        let plane = |w: u32, h: u32, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("hermes screen share plane"),
                size: extent(w, h),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let (r8, rg8) = (wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm);
        let planes = if nv12 {
            vec![plane(width, height, r8), plane(width / 2, height / 2, rg8)]
        } else {
            vec![
                plane(width, height, r8),
                plane(width / 2, height / 2, r8),
                plane(width / 2, height / 2, r8),
            ]
        };
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hermes screen share params"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let views: Vec<wgpu::TextureView> = planes
            .iter()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hermes screen share"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&views[1]),
                },
                // NV12 has no V plane: the U/V one is bound again (the shader ignores it).
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&views[views.len() - 1]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: params.as_entire_binding(),
                },
            ],
        });
        Entry {
            size: (width, height),
            nv12,
            version: u64::MAX,
            planes,
            params,
            bind_group,
        }
    }
}

fn extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    }
}

/// How much of the widget's width and height the picture covers when it fits inside keeping its
/// aspect ratio (1.0 on the side it touches).
pub fn fit_scale((width, height): (u32, u32), bounds: iced::Size) -> (f32, f32) {
    if width == 0 || height == 0 || bounds.width <= 0.0 || bounds.height <= 0.0 {
        return (0.0, 0.0);
    }
    let picture = width as f32 / height as f32;
    let widget = bounds.width / bounds.height;
    if picture > widget {
        (1.0, widget / picture)
    } else {
        (picture / widget, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_are_letterboxed() {
        let wide = iced::Size::new(1600.0, 900.0);
        assert_eq!(fit_scale((1280, 720), wide), (1.0, 1.0));
        assert_eq!(fit_scale((1280, 1440), wide), (0.5, 1.0), "pillarboxed");
        let (sx, sy) = fit_scale((3200, 900), wide);
        assert_eq!(sx, 1.0);
        assert!((sy - 0.5).abs() < 1e-6, "letterboxed");
        assert_eq!(fit_scale((0, 0), wide), (0.0, 0.0));
    }

    /// Draws one flat-colored picture as I420 and as NV12 with the real pipeline into a small
    /// texture and reads the pixels back: both must give the same, expected color. Skipped
    /// where there is no GPU (or software adapter) at all.
    #[test]
    fn i420_and_nv12_pictures_draw_the_same_colors() {
        use iced::futures::executor::block_on;
        use iced::widget::shader::{Pipeline as _, Primitive as _};

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Ok(adapter) = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            force_fallback_adapter: false,
            ..wgpu::RequestAdapterOptions::default()
        })) else {
            println!("no GPU here: skipped");
            return;
        };
        let (device, queue) =
            block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut pipeline = Pipeline::new(&device, &queue, format);

        let (y, u, v) = (100u8, 90u8, 170u8);
        let i420 = Picture {
            width: 4,
            height: 4,
            nv12: false,
            planes: [vec![y; 16], vec![u; 4], vec![v; 4]].concat(),
        };
        let nv12 = Picture {
            width: 4,
            height: 4,
            nv12: true,
            planes: [vec![y; 16], [u, v].repeat(4)].concat(),
        };
        let draw = |pipeline: &mut Pipeline, picture: Picture, id: u64| -> [u8; 4] {
            let share = Share::new(&id, Arc::new(picture));
            pipeline.prepare(&device, &queue, &share, iced::Size::new(4.0, 4.0));
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: extent(4, 4),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = target.create_view(&wgpu::TextureViewDescriptor::default());
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 256 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..wgpu::RenderPassDescriptor::default()
                });
                Primitive(share).draw(pipeline, &mut pass);
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
                        bytes_per_row: Some(256),
                        rows_per_image: Some(4),
                    },
                },
                extent(4, 4),
            );
            queue.submit([encoder.finish()]);
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, |r| r.unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let data = readback.slice(..).get_mapped_range();
            // A pixel inside (row 1, column 1).
            let at = 256 + 4;
            [data[at], data[at + 1], data[at + 2], data[at + 3]]
        };
        let a = draw(&mut pipeline, i420, 1);
        let b = draw(&mut pipeline, nv12, 2);
        let close =
            |p: [u8; 4], q: [u8; 4], d: u8| p.iter().zip(q).all(|(a, b)| a.abs_diff(b) <= d);
        assert!(close(a, b, 1), "I420 {a:?} vs NV12 {b:?}");
        // Limited-range BT.601: (100, 90, 170) is about (165, 78, 21).
        assert!(close(a, [165, 78, 21, 255], 2), "{a:?}");
    }

    #[test]
    fn new_pictures_bump_the_version() {
        let picture = |n| {
            Arc::new(Picture {
                width: 2,
                height: 2,
                nv12: false,
                planes: vec![n; 6],
            })
        };
        let mut share = Share::new(&("hermes.example", 7u64), picture(1));
        let key = share.key;
        assert_eq!(share.version, 0);
        share.update(picture(2));
        assert_eq!((share.key, share.version), (key, 1));
        assert_ne!(Share::new(&("hermes.example", 8u64), picture(1)).key, key);
    }
}
