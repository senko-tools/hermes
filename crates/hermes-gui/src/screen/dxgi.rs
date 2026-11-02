//! Capturing a screen with DXGI Desktop Duplication (Windows 8 and later): the compositor hands
//! over each new desktop picture as a GPU texture, which is copied into one reused CPU-readable
//! texture. Much cheaper than GDI's `BitBlt`, which reads the whole screen back through the
//! compositor on every call.
//!
//! The device is created on the GPU the screen is connected to, so it also works on laptops
//! with two GPUs. The mouse pointer is not in the picture (as with GDI).

// Direct3D and DXGI are COM APIs: every call is `unsafe`.
#![allow(unsafe_code)]

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
    IDXGIAdapter, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface;

use super::codec::Frame;

/// Why [`Duplicator::next`] failed.
#[derive(Debug)]
pub enum Error {
    /// The duplication stopped (a mode change, the secure desktop, a full-screen game): open a
    /// new one.
    Lost,
    Other(String),
}

impl From<windows::core::Error> for Error {
    fn from(err: windows::core::Error) -> Error {
        if err.code() == DXGI_ERROR_ACCESS_LOST {
            Error::Lost
        } else {
            Error::Other(err.to_string())
        }
    }
}

/// Duplicates one screen.
pub struct Duplicator {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    /// The CPU-readable copy of the desktop, reused while the size stays.
    staging: Option<(ID3D11Texture2D, u32, u32)>,
}

impl Duplicator {
    /// Duplicates the screen whose system name (`\\.\DISPLAY1`) is `name`.
    pub fn open(name: &str) -> Result<Duplicator, String> {
        let failed = |what: &str, err: windows::core::Error| format!("{what}: {err}");
        unsafe {
            let factory: IDXGIFactory1 =
                CreateDXGIFactory1().map_err(|e| failed("DXGI factory", e))?;
            let mut adapter_index = 0;
            while let Ok(adapter) = factory.EnumAdapters1(adapter_index) {
                adapter_index += 1;
                let mut output_index = 0;
                while let Ok(output) = adapter.EnumOutputs(output_index) {
                    output_index += 1;
                    let Ok(desc) = output.GetDesc() else {
                        continue;
                    };
                    let len = desc
                        .DeviceName
                        .iter()
                        .position(|c| *c == 0)
                        .unwrap_or(desc.DeviceName.len());
                    if String::from_utf16_lossy(&desc.DeviceName[..len]) != name {
                        continue;
                    }
                    let adapter: IDXGIAdapter = adapter.cast().map_err(|e| failed("adapter", e))?;
                    let mut device = None;
                    let mut context = None;
                    D3D11CreateDevice(
                        &adapter,
                        D3D_DRIVER_TYPE_UNKNOWN,
                        HMODULE::default(),
                        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                        None,
                        D3D11_SDK_VERSION,
                        Some(&mut device),
                        None,
                        Some(&mut context),
                    )
                    .map_err(|e| failed("Direct3D 11 device", e))?;
                    let (Some(device), Some(context)) = (device, context) else {
                        return Err("no Direct3D 11 device".into());
                    };
                    let output: IDXGIOutput1 = output.cast().map_err(|e| failed("output", e))?;
                    let duplication = output
                        .DuplicateOutput(&device)
                        .map_err(|e| failed("desktop duplication", e))?;
                    return Ok(Duplicator {
                        device,
                        context,
                        duplication,
                        staging: None,
                    });
                }
            }
        }
        Err(format!("screen {name} not found for desktop duplication"))
    }

    /// The next new picture of the screen, waiting at most `timeout_ms` for one; `None` if the
    /// screen did not change meanwhile.
    pub fn next(&mut self, timeout_ms: u32) -> Result<Option<Frame>, Error> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        unsafe {
            if let Err(err) =
                self.duplication
                    .AcquireNextFrame(timeout_ms, &mut info, &mut resource)
            {
                return if err.code() == DXGI_ERROR_WAIT_TIMEOUT {
                    Ok(None)
                } else {
                    Err(err.into())
                };
            }
            // Always released, or the next frame never comes.
            let copied = self.copy(&info, resource);
            let released = self.duplication.ReleaseFrame();
            let staging = copied?;
            released?;
            match staging {
                Some((texture, width, height)) => self.read(&texture, width, height).map(Some),
                None => Ok(None),
            }
        }
    }

    /// Queues the copy of an acquired picture into the staging texture; `None` if only the
    /// pointer moved.
    unsafe fn copy(
        &mut self,
        info: &DXGI_OUTDUPL_FRAME_INFO,
        resource: Option<IDXGIResource>,
    ) -> Result<Option<(ID3D11Texture2D, u32, u32)>, Error> {
        if info.LastPresentTime == 0 {
            return Ok(None);
        }
        let Some(resource) = resource else {
            return Ok(None);
        };
        unsafe {
            let texture: ID3D11Texture2D = resource.cast()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
                return Err(Error::Other(format!(
                    "unsupported desktop format {:?}",
                    desc.Format
                )));
            }
            let fits = self
                .staging
                .as_ref()
                .is_some_and(|(_, w, h)| (*w, *h) == (desc.Width, desc.Height));
            if !fits {
                let staging_desc = D3D11_TEXTURE2D_DESC {
                    Width: desc.Width,
                    Height: desc.Height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: desc.Format,
                    SampleDesc: desc.SampleDesc,
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut staging = None;
                self.device
                    .CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
                let staging = staging.ok_or(Error::Other("no staging texture".into()))?;
                self.staging = Some((staging, desc.Width, desc.Height));
            }
            let Some((staging, width, height)) = self.staging.clone() else {
                return Ok(None);
            };
            self.context.CopyResource(&staging, &texture);
            Ok(Some((staging, width, height)))
        }
    }

    /// Reads the staging texture's pixels (waits for the copy).
    unsafe fn read(
        &self,
        staging: &ID3D11Texture2D,
        width: u32,
        height: u32,
    ) -> Result<Frame, Error> {
        let row = width as usize * 4;
        let mut pixels = vec![0u8; row * height as usize];
        unsafe {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let pitch = mapped.RowPitch as usize;
            let source = mapped.pData as *const u8;
            for (y, line) in pixels.chunks_exact_mut(row).enumerate() {
                line.copy_from_slice(std::slice::from_raw_parts(source.add(y * pitch), row));
            }
            self.context.Unmap(staging, 0);
        }
        Ok(Frame {
            width,
            height,
            bgra: true,
            pixels,
        })
    }
}
