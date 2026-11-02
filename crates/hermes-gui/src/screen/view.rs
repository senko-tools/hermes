//! Drawing a screen share on the GPU with iced's `shader` widget. Each share keeps one set of
//! textures (the Y, U and V planes), overwritten in place when a new picture arrives, and the
//! fragment shader turns YUV into colors. Showing pictures as images instead would create (and
//! drop) a GPU texture for every frame and convert every pixel on the CPU first.
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
    version: u64,
    planes: [wgpu::Texture; 3],
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
}

const SHADER: &str = r"
struct Params {
    scale: vec2<f32>,
    srgb: f32,
    unused: f32,
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
    let u = (textureSample(u_plane, linear_sampler, in.uv).r * 255.0 - 128.0) / 255.0;
    let v = (textureSample(v_plane, linear_sampler, in.uv).r * 255.0 - 128.0) / 255.0;
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
        let stale = self.entries.get(&share.key).is_none_or(|e| e.size != size);
        if stale {
            let entry = self.entry(device, size);
            self.entries.insert(share.key, entry);
        }
        let Some(entry) = self.entries.get_mut(&share.key) else {
            return;
        };
        if stale || entry.version != share.version {
            let (cw, ch) = picture.chroma_size();
            let planes = [
                (picture.y(), size.0, size.1),
                (picture.u(), cw, ch),
                (picture.v(), cw, ch),
            ];
            for (texture, (data, w, h)) in entry.planes.iter().zip(planes) {
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
                        bytes_per_row: Some(w),
                        rows_per_image: Some(h),
                    },
                    extent(w, h),
                );
            }
            entry.version = share.version;
        }
        // Fit the picture into the widget (letterboxed), every frame: the widget can resize.
        let (sx, sy) = fit_scale(size, bounds);
        let srgb = if self.srgb_target { 1.0f32 } else { 0.0 };
        let mut params = Vec::with_capacity(16);
        for value in [sx, sy, srgb, 0.0] {
            params.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&entry.params, 0, &params);
    }

    fn entry(&self, device: &wgpu::Device, (width, height): (u32, u32)) -> Entry {
        let plane = |w: u32, h: u32| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("hermes screen share plane"),
                size: extent(w, h),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let planes = [
            plane(width, height),
            plane(width / 2, height / 2),
            plane(width / 2, height / 2),
        ];
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
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&views[2]),
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

    #[test]
    fn new_pictures_bump_the_version() {
        let picture = |n| {
            Arc::new(Picture {
                width: 2,
                height: 2,
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
