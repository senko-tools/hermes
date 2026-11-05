//! Scaling a desktop picture and converting it to NV12 on the GPU, in one pass of the Direct3D 11
//! video processor (the fixed-function hardware video players use). It replaces reading the
//! picture back, scaling it on the CPU ([`super::scale`]) and converting it to YUV on the CPU
//! for the hardware encoder ([`super::mf_encode`]).
//!
//! The output is limited-range BT.601, what OpenH264 produces too, so watchers draw both the
//! same way ([`super::view`]).

// Direct3D is a COM API: every call is `unsafe`.
#![allow(unsafe_code)]

use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_OPTIMAL_SPEED, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11Device, ID3D11Texture2D, ID3D11VideoContext,
    ID3D11VideoContext1, ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601,
    DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::core::Interface;

/// Converted pictures kept at once: the encoder may still read one while the next is written.
const POOL: usize = 4;

/// One NV12 picture for the encoder.
#[derive(Clone)]
pub struct Nv12 {
    pub texture: ID3D11Texture2D,
    pub width: u32,
    pub height: u32,
}

/// Converts pictures of one input size to NV12 pictures of one output size.
pub struct GpuConvert {
    context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    video_device: ID3D11VideoDevice,
    input_size: (u32, u32),
    output_size: (u32, u32),
    /// The input texture (the duplicator reuses one) and its view.
    input: Option<(ID3D11Texture2D, ID3D11VideoProcessorInputView)>,
    pool: Vec<(Nv12, ID3D11VideoProcessorOutputView)>,
    next: usize,
}

fn failed(what: &str) -> impl Fn(windows::core::Error) -> String + '_ {
    move |err| format!("{what}: {err}")
}

impl GpuConvert {
    /// For pictures of `input` size on `device`, scaled to `output` (both even).
    pub fn new(
        device: &ID3D11Device,
        input: (u32, u32),
        output: (u32, u32),
        fps: u32,
    ) -> Result<GpuConvert, String> {
        unsafe {
            let video_device: ID3D11VideoDevice =
                device.cast().map_err(failed("no video device"))?;
            let immediate = device
                .GetImmediateContext()
                .map_err(failed("no device context"))?;
            let context: ID3D11VideoContext =
                immediate.cast().map_err(failed("no video context"))?;
            let rate = DXGI_RATIONAL {
                Numerator: fps,
                Denominator: 1,
            };
            let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: input.0,
                InputHeight: input.1,
                OutputFrameRate: rate,
                OutputWidth: output.0,
                OutputHeight: output.1,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
            };
            let enumerator = video_device
                .CreateVideoProcessorEnumerator(&content)
                .map_err(failed("video processor"))?;
            let processor = video_device
                .CreateVideoProcessor(&enumerator, 0)
                .map_err(failed("video processor"))?;
            // Full-range RGB in, limited-range BT.601 out; no automatic "enhancements".
            if let Ok(context1) = context.cast::<ID3D11VideoContext1>() {
                context1.VideoProcessorSetStreamColorSpace1(
                    &processor,
                    0,
                    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
                );
                context1.VideoProcessorSetOutputColorSpace1(
                    &processor,
                    DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601,
                );
            }
            context.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);

            let mut pool = Vec::with_capacity(POOL);
            for _ in 0..POOL {
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: output.0,
                    Height: output.1,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_NV12,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                let mut texture = None;
                device
                    .CreateTexture2D(&desc, None, Some(&mut texture))
                    .map_err(failed("NV12 texture"))?;
                let texture = texture.ok_or("no NV12 texture")?;
                let view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                    },
                };
                let mut view = None;
                video_device
                    .CreateVideoProcessorOutputView(
                        &texture,
                        &enumerator,
                        &view_desc,
                        Some(&mut view),
                    )
                    .map_err(failed("NV12 view"))?;
                let view = view.ok_or("no NV12 view")?;
                pool.push((
                    Nv12 {
                        texture,
                        width: output.0,
                        height: output.1,
                    },
                    view,
                ));
            }
            Ok(GpuConvert {
                context,
                enumerator,
                processor,
                video_device,
                input_size: input,
                output_size: output,
                input: None,
                pool,
                next: 0,
            })
        }
    }

    pub fn input_size(&self) -> (u32, u32) {
        self.input_size
    }

    pub fn output_size(&self) -> (u32, u32) {
        self.output_size
    }

    /// Queues the conversion of `input` (of the input size) into the next pooled NV12 picture.
    /// The GPU runs it in order with the encoder's work, so nothing waits here.
    pub fn convert(&mut self, input: &ID3D11Texture2D) -> Result<Nv12, String> {
        unsafe {
            let cached = self
                .input
                .as_ref()
                .is_some_and(|(texture, _)| texture == input);
            if !cached {
                let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                    FourCC: 0,
                    ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_VPIV {
                            MipSlice: 0,
                            ArraySlice: 0,
                        },
                    },
                };
                let mut view = None;
                self.video_device
                    .CreateVideoProcessorInputView(input, &self.enumerator, &desc, Some(&mut view))
                    .map_err(failed("desktop view"))?;
                let view = view.ok_or("no desktop view")?;
                self.input = Some((input.clone(), view));
            }
            let Some((_, input_view)) = &self.input else {
                return Err("no desktop view".into());
            };
            let (picture, output_view) = &self.pool[self.next];
            self.next = (self.next + 1) % self.pool.len();
            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: std::mem::ManuallyDrop::new(Some(input_view.clone())),
                ..D3D11_VIDEO_PROCESSOR_STREAM::default()
            };
            let streams = [stream];
            let result = self
                .context
                .VideoProcessorBlt(&self.processor, output_view, 0, &streams);
            // The stream holds a reference of its own.
            let [mut stream] = streams;
            std::mem::ManuallyDrop::drop(&mut stream.pInputSurface);
            result.map_err(failed("conversion"))?;
            Ok(picture.clone())
        }
    }
}
