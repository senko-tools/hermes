//! Draws the mouse pointer ([`super::cursor`]) over a screen picture on the GPU: one textured
//! quad the size of the pointer, placed with the viewport, so the picture never leaves the GPU.

// Direct3D is a COM API: every call is `unsafe`.
#![allow(unsafe_code)]

use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{ID3DBlob, ID3DInclude};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_BLEND, D3D11_BLEND_DESC, D3D11_BLEND_INV_DEST_COLOR,
    D3D11_BLEND_INV_SRC_ALPHA, D3D11_BLEND_INV_SRC_COLOR, D3D11_BLEND_ONE, D3D11_BLEND_OP_ADD,
    D3D11_BLEND_ZERO, D3D11_COLOR_WRITE_ENABLE_ALL, D3D11_COMPARISON_NEVER, D3D11_CULL_NONE,
    D3D11_FILL_SOLID, D3D11_FILTER_MIN_MAG_MIP_POINT, D3D11_RASTERIZER_DESC,
    D3D11_RENDER_TARGET_BLEND_DESC, D3D11_SAMPLER_DESC, D3D11_SUBRESOURCE_DATA,
    D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_TEXTURE2D_DESC, D3D11_USAGE_IMMUTABLE, D3D11_VIEWPORT,
    ID3D11BlendState, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11PixelShader,
    ID3D11RasterizerState, ID3D11RenderTargetView, ID3D11SamplerState, ID3D11ShaderResourceView,
    ID3D11Texture2D, ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::core::{Interface, PCSTR, s};

use super::cursor::{Pointer, Shape};

/// A quad covering the viewport, from the vertex index (a 4-vertex strip, no vertex buffer),
/// sampled 1:1 from the pointer picture.
const SHADER: &str = r"
struct V { float4 pos : SV_Position; float2 uv : TEXCOORD0; };
V vs(uint id : SV_VertexID) {
    float2 uv = float2(id & 1, id >> 1);
    V v;
    v.pos = float4(uv.x * 2 - 1, 1 - uv.y * 2, 0, 1);
    v.uv = uv;
    return v;
}
Texture2D picture : register(t0);
SamplerState nearest : register(s0);
float4 ps(V v) : SV_Target { return picture.Sample(nearest, v.uv); }
";

/// The uploaded pointer pictures.
struct Uploaded {
    width: u32,
    height: u32,
    color: ID3D11ShaderResourceView,
    invert: Option<ID3D11ShaderResourceView>,
}

pub struct CursorDrawer {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    rasterizer: ID3D11RasterizerState,
    /// Premultiplied alpha over the screen.
    over: ID3D11BlendState,
    /// `screen * (1 - src) + (1 - screen) * src`: inverts where the source is white.
    invert: ID3D11BlendState,
    uploaded: Option<Uploaded>,
    /// The render target view of the last target texture (the same one every frame).
    target: Option<(ID3D11Texture2D, ID3D11RenderTargetView)>,
}

fn compile(entry: PCSTR, profile: PCSTR) -> Result<ID3DBlob, String> {
    let mut code = None;
    let mut errors = None;
    let result = unsafe {
        D3DCompile(
            SHADER.as_ptr().cast(),
            SHADER.len(),
            PCSTR::null(),
            None,
            None::<&ID3DInclude>,
            entry,
            profile,
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(err) = result {
        let detail = errors
            .map(|e: ID3DBlob| unsafe {
                String::from_utf8_lossy(std::slice::from_raw_parts(
                    e.GetBufferPointer().cast::<u8>(),
                    e.GetBufferSize(),
                ))
                .into_owned()
            })
            .unwrap_or_default();
        return Err(format!("pointer shader: {err} {detail}"));
    }
    code.ok_or_else(|| "pointer shader: no code".into())
}

fn bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    }
}

fn blend(
    device: &ID3D11Device,
    src: D3D11_BLEND,
    dest: D3D11_BLEND,
) -> Result<ID3D11BlendState, String> {
    let mut desc = D3D11_BLEND_DESC::default();
    desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
        BlendEnable: true.into(),
        SrcBlend: src,
        DestBlend: dest,
        BlendOp: D3D11_BLEND_OP_ADD,
        // The screen's alpha stays.
        SrcBlendAlpha: D3D11_BLEND_ZERO,
        DestBlendAlpha: D3D11_BLEND_ONE,
        BlendOpAlpha: D3D11_BLEND_OP_ADD,
        RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
    };
    let mut state = None;
    unsafe { device.CreateBlendState(&desc, Some(&mut state)) }
        .map_err(|e| format!("pointer blend state: {e}"))?;
    state.ok_or_else(|| "pointer blend state".into())
}

fn upload(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<ID3D11ShaderResourceView, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_IMMUTABLE,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let data = D3D11_SUBRESOURCE_DATA {
        pSysMem: pixels.as_ptr().cast(),
        SysMemPitch: width * 4,
        SysMemSlicePitch: 0,
    };
    let failed = |e: windows::core::Error| format!("pointer texture: {e}");
    unsafe {
        let mut texture = None;
        device
            .CreateTexture2D(&desc, Some(&data), Some(&mut texture))
            .map_err(failed)?;
        let texture = texture.ok_or("pointer texture")?;
        let mut view = None;
        device
            .CreateShaderResourceView(&texture, None, Some(&mut view))
            .map_err(failed)?;
        view.ok_or_else(|| "pointer texture view".into())
    }
}

impl CursorDrawer {
    pub fn new(device: &ID3D11Device) -> Result<CursorDrawer, String> {
        let vs_code = compile(s!("vs"), s!("vs_4_0"))?;
        let ps_code = compile(s!("ps"), s!("ps_4_0"))?;
        let failed = |what: &str, e: windows::core::Error| format!("pointer {what}: {e}");
        unsafe {
            let mut vs = None;
            device
                .CreateVertexShader(bytes(&vs_code), None, Some(&mut vs))
                .map_err(|e| failed("vertex shader", e))?;
            let mut ps = None;
            device
                .CreatePixelShader(bytes(&ps_code), None, Some(&mut ps))
                .map_err(|e| failed("pixel shader", e))?;
            let sampler_desc = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                ComparisonFunc: D3D11_COMPARISON_NEVER,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            let mut sampler = None;
            device
                .CreateSamplerState(&sampler_desc, Some(&mut sampler))
                .map_err(|e| failed("sampler", e))?;
            let rasterizer_desc = D3D11_RASTERIZER_DESC {
                FillMode: D3D11_FILL_SOLID,
                CullMode: D3D11_CULL_NONE,
                DepthClipEnable: true.into(),
                ..Default::default()
            };
            let mut rasterizer = None;
            device
                .CreateRasterizerState(&rasterizer_desc, Some(&mut rasterizer))
                .map_err(|e| failed("rasterizer", e))?;
            Ok(CursorDrawer {
                vs: vs.ok_or("pointer vertex shader")?,
                ps: ps.ok_or("pointer pixel shader")?,
                sampler: sampler.ok_or("pointer sampler")?,
                rasterizer: rasterizer.ok_or("pointer rasterizer")?,
                over: blend(device, D3D11_BLEND_ONE, D3D11_BLEND_INV_SRC_ALPHA)?,
                invert: blend(
                    device,
                    D3D11_BLEND_INV_DEST_COLOR,
                    D3D11_BLEND_INV_SRC_COLOR,
                )?,
                uploaded: None,
                target: None,
            })
        }
    }

    /// Draws `pointer` (if visible and known) over `target`, a render-target texture.
    pub fn draw(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        target: &ID3D11Texture2D,
        pointer: &mut Pointer,
    ) -> Result<(), String> {
        if pointer.shape_changed {
            pointer.shape_changed = false;
            self.uploaded = match &pointer.shape {
                Some(shape) => Some(Self::upload(device, shape)?),
                None => None,
            };
        }
        let Some(uploaded) = &self.uploaded else {
            return Ok(());
        };
        if !pointer.visible {
            return Ok(());
        }
        if self.target.as_ref().is_none_or(|(t, _)| t != target) {
            let mut view = None;
            unsafe { device.CreateRenderTargetView(target, None, Some(&mut view)) }
                .map_err(|e| format!("pointer render target: {e}"))?;
            self.target = Some((target.clone(), view.ok_or("pointer render target")?));
        }
        let Some((_, view)) = &self.target else {
            return Ok(());
        };
        let viewport = D3D11_VIEWPORT {
            TopLeftX: pointer.position.0 as f32,
            TopLeftY: pointer.position.1 as f32,
            Width: uploaded.width as f32,
            Height: uploaded.height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        // The encoder uses the same context from its own threads.
        let multithread = context.cast::<ID3D11Multithread>().ok();
        unsafe {
            if let Some(m) = &multithread {
                m.Enter();
            }
            context.OMSetRenderTargets(Some(&[Some(view.clone())]), None);
            context.RSSetViewports(Some(&[viewport]));
            context.RSSetState(&self.rasterizer);
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            context.VSSetShader(&self.vs, None);
            context.PSSetShader(&self.ps, None);
            context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            context.PSSetShaderResources(0, Some(&[Some(uploaded.color.clone())]));
            context.OMSetBlendState(&self.over, None, u32::MAX);
            context.Draw(4, 0);
            if let Some(invert) = &uploaded.invert {
                context.PSSetShaderResources(0, Some(&[Some(invert.clone())]));
                context.OMSetBlendState(&self.invert, None, u32::MAX);
                context.Draw(4, 0);
            }
            // Unbind, so the texture can be read (converted, copied) next.
            context.PSSetShaderResources(0, Some(&[None]));
            context.OMSetRenderTargets(None, None);
            context.OMSetBlendState(None, None, u32::MAX);
            if let Some(m) = &multithread {
                m.Leave();
            }
        }
        Ok(())
    }

    fn upload(device: &ID3D11Device, shape: &Shape) -> Result<Uploaded, String> {
        Ok(Uploaded {
            width: shape.width,
            height: shape.height,
            color: upload(device, shape.width, shape.height, &shape.color)?,
            invert: match &shape.invert {
                Some(pixels) => Some(upload(device, shape.width, shape.height, pixels)?),
                None => None,
            },
        })
    }
}
