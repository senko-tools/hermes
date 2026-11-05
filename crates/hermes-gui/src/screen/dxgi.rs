//! Capturing a screen with DXGI Desktop Duplication (Windows 8 and later): the compositor hands
//! over each new desktop picture as a GPU texture.
//!
//! Two ways out: [`Duplicator::next`] copies it into one reused CPU-readable texture and reads it
//! back (for the software encoder; much cheaper than GDI's `BitBlt`, which reads the whole screen
//! back through the compositor on every call), and [`Duplicator::next_gpu`] keeps it on the GPU
//! (for the hardware encoder: [`super::gpu_convert`] scales and converts it there).
//!
//! The device is created on the GPU the screen is connected to, so it also works on laptops
//! with two GPUs, with video support (for conversion and encoding) where the driver has it, and
//! multithread protection (the encoder uses it too). The mouse pointer is not in the picture (as
//! with GDI).

// Direct3D and DXGI are COM APIs: every call is `unsafe`.
#![allow(unsafe_code)]

use windows::Win32::Foundation::{E_FAIL, HMODULE, LUID};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_FLAG, D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
    D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
    ID3D11Multithread, ID3D11Texture2D,
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

/// A picture of the screen on the GPU: the duplicator's own texture, overwritten by the next
/// [`Duplicator::next_gpu`].
#[derive(Clone)]
pub struct GpuShot {
    pub texture: ID3D11Texture2D,
    pub width: u32,
    pub height: u32,
}

/// Duplicates one screen.
pub struct Duplicator {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    /// The GPU the screen is connected to (a hardware encoder must be on the same one).
    luid: LUID,
    /// The device has video support (GPU conversion and hardware encoding).
    video: bool,
    /// The CPU-readable copy of the desktop, reused while the size stays.
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    /// The GPU copy of the desktop for [`Duplicator::next_gpu`], reused while the size stays.
    desktop: Option<(ID3D11Texture2D, u32, u32)>,
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
                    let luid = adapter
                        .GetDesc1()
                        .map_err(|e| failed("adapter", e))?
                        .AdapterLuid;
                    let adapter: IDXGIAdapter = adapter.cast().map_err(|e| failed("adapter", e))?;
                    // Video support where the driver offers it (not, for example, Microsoft's
                    // basic display driver); without it only the CPU path works.
                    let (device, context, video) = match create_device(
                        &adapter,
                        D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                    ) {
                        Ok((device, context)) => (device, context, true),
                        Err(_) => {
                            let (device, context) =
                                create_device(&adapter, D3D11_CREATE_DEVICE_BGRA_SUPPORT)
                                    .map_err(|e| failed("Direct3D 11 device", e))?;
                            (device, context, false)
                        }
                    };
                    // The hardware encoder uses the device from its own threads.
                    if let Ok(multithread) = context.cast::<ID3D11Multithread>() {
                        let _ = multithread.SetMultithreadProtected(true);
                    }
                    let output: IDXGIOutput1 = output.cast().map_err(|e| failed("output", e))?;
                    let duplication = output
                        .DuplicateOutput(&device)
                        .map_err(|e| failed("desktop duplication", e))?;
                    return Ok(Duplicator {
                        device,
                        context,
                        duplication,
                        luid,
                        video,
                        staging: None,
                        desktop: None,
                    });
                }
            }
        }
        Err(format!("screen {name} not found for desktop duplication"))
    }

    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    /// The GPU the screen is connected to.
    pub fn luid(&self) -> LUID {
        self.luid
    }

    /// Whether the device can convert and encode video on the GPU.
    pub fn video(&self) -> bool {
        self.video
    }

    /// The next new picture of the screen, read back into memory, waiting at most `timeout_ms`
    /// for one; `None` if the screen did not change meanwhile.
    pub fn next(&mut self, timeout_ms: u32) -> Result<Option<Frame>, Error> {
        let Some(texture) = self.acquire(timeout_ms, Target::Staging)? else {
            return Ok(None);
        };
        unsafe { self.read(&texture.texture, texture.width, texture.height) }.map(Some)
    }

    /// The next new picture of the screen, kept on the GPU, waiting at most `timeout_ms` for
    /// one; `None` if the screen did not change meanwhile.
    pub fn next_gpu(&mut self, timeout_ms: u32) -> Result<Option<GpuShot>, Error> {
        self.acquire(timeout_ms, Target::Gpu)
    }

    /// Waits for a new picture and queues its copy into the texture for `target`.
    fn acquire(&mut self, timeout_ms: u32, target: Target) -> Result<Option<GpuShot>, Error> {
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
            let copied = self.copy(&info, resource, target);
            let released = self.duplication.ReleaseFrame();
            let shot = copied?;
            released?;
            Ok(shot)
        }
    }

    /// Queues the copy of an acquired picture into the staging or GPU texture; `None` if only
    /// the pointer moved.
    unsafe fn copy(
        &mut self,
        info: &DXGI_OUTDUPL_FRAME_INFO,
        resource: Option<IDXGIResource>,
        target: Target,
    ) -> Result<Option<GpuShot>, Error> {
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
            let slot = match target {
                Target::Staging => &mut self.staging,
                Target::Gpu => &mut self.desktop,
            };
            let fits = slot
                .as_ref()
                .is_some_and(|(_, w, h)| (*w, *h) == (desc.Width, desc.Height));
            if !fits {
                let (usage, bind, cpu) = match target {
                    Target::Staging => (D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32),
                    Target::Gpu => (
                        D3D11_USAGE_DEFAULT,
                        (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
                        0,
                    ),
                };
                let copy_desc = D3D11_TEXTURE2D_DESC {
                    Width: desc.Width,
                    Height: desc.Height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: desc.Format,
                    SampleDesc: desc.SampleDesc,
                    Usage: usage,
                    BindFlags: bind,
                    CPUAccessFlags: cpu,
                    MiscFlags: 0,
                };
                let mut copy = None;
                self.device
                    .CreateTexture2D(&copy_desc, None, Some(&mut copy))?;
                let copy = copy.ok_or(Error::Other("no copy texture".into()))?;
                *slot = Some((copy, desc.Width, desc.Height));
            }
            let Some((copy, width, height)) = slot.clone() else {
                return Ok(None);
            };
            self.context.CopyResource(&copy, &texture);
            Ok(Some(GpuShot {
                texture: copy,
                width,
                height,
            }))
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

/// Where [`Duplicator::acquire`] copies a picture.
#[derive(Clone, Copy)]
enum Target {
    /// The CPU-readable texture.
    Staging,
    /// The GPU texture.
    Gpu,
}

/// A Direct3D 11 device and its immediate context on `adapter`.
fn create_device(
    adapter: &IDXGIAdapter,
    flags: D3D11_CREATE_DEVICE_FLAG,
) -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            flags,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    match (device, context) {
        (Some(device), Some(context)) => Ok((device, context)),
        _ => Err(windows::core::Error::from_hresult(E_FAIL)),
    }
}
