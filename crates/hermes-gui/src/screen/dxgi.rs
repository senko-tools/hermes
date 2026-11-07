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
//! multithread protection (the encoder uses it too).
//!
//! The mouse pointer is not in the duplicated picture: its position and shape come separately
//! and are drawn over a copy of it on the GPU ([`super::cursor_gpu`]), so a pointer that moves on
//! a still screen also makes a new picture.

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
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
    DXGI_OUTDUPL_POINTER_SHAPE_INFO, IDXGIAdapter, IDXGIFactory1, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface;

use super::codec::Frame;
use super::cursor::{Pointer, Shape};
use super::cursor_gpu::CursorDrawer;

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
    /// The CPU-readable copy of the picture, reused while the size stays.
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    /// The last desktop picture without the pointer, reused while the size stays.
    clean: Option<(ID3D11Texture2D, u32, u32)>,
    /// `clean` with the pointer drawn over it: what [`Duplicator::next_gpu`] returns.
    composed: Option<(ID3D11Texture2D, u32, u32)>,
    pointer: Pointer,
    /// `None` until the first picture, and `Err` for good if it could not be made (no pointer
    /// then).
    drawer: Option<Result<CursorDrawer, ()>>,
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
                        clean: None,
                        composed: None,
                        pointer: Pointer::default(),
                        drawer: None,
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

    /// Waits for a new picture (of the desktop or the pointer) and queues the copy of it, with the
    /// pointer drawn over it, into the texture for `target`.
    fn acquire(&mut self, timeout_ms: u32, target: Target) -> Result<Option<GpuShot>, Error> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        let (desktop, pointer) = unsafe {
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
            let copied = self.copy_clean(&info, resource);
            let pointer = self.update_pointer(&info);
            let released = self.duplication.ReleaseFrame();
            let desktop = copied?;
            released?;
            (desktop, pointer)
        };
        if !desktop && !pointer {
            return Ok(None);
        }
        let Some((clean, width, height)) = self.clean.clone() else {
            return Ok(None);
        };
        unsafe {
            let composed = self.texture(Slot::Composed, width, height)?;
            self.context.CopyResource(&composed, &clean);
            self.draw_pointer(&composed);
            let texture = match target {
                Target::Gpu => composed,
                Target::Staging => {
                    let staging = self.texture(Slot::Staging, width, height)?;
                    self.context.CopyResource(&staging, &composed);
                    staging
                }
            };
            Ok(Some(GpuShot {
                texture,
                width,
                height,
            }))
        }
    }

    /// Queues the copy of an acquired desktop picture into `clean`; `false` if only the pointer
    /// changed.
    unsafe fn copy_clean(
        &mut self,
        info: &DXGI_OUTDUPL_FRAME_INFO,
        resource: Option<IDXGIResource>,
    ) -> Result<bool, Error> {
        if info.LastPresentTime == 0 {
            return Ok(false);
        }
        let Some(resource) = resource else {
            return Ok(false);
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
            let clean = self.texture(Slot::Clean, desc.Width, desc.Height)?;
            self.context.CopyResource(&clean, &texture);
            Ok(true)
        }
    }

    /// Takes the pointer's new position and shape from an acquired frame (before it is
    /// released); `true` if what is drawn changed.
    unsafe fn update_pointer(&mut self, info: &DXGI_OUTDUPL_FRAME_INFO) -> bool {
        let mut changed = false;
        if info.LastMouseUpdateTime != 0 {
            let visible = info.PointerPosition.Visible.as_bool();
            let position = (
                info.PointerPosition.Position.x,
                info.PointerPosition.Position.y,
            );
            changed =
                visible != self.pointer.visible || (visible && position != self.pointer.position);
            self.pointer.visible = visible;
            self.pointer.position = position;
        }
        let size = info.PointerShapeBufferSize;
        if size > 0 {
            let mut buffer = vec![0u8; size as usize];
            let mut required = 0;
            let mut shape = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
            let read = unsafe {
                self.duplication.GetFramePointerShape(
                    size,
                    buffer.as_mut_ptr().cast(),
                    &mut required,
                    &mut shape,
                )
            };
            match read {
                Ok(()) => {
                    self.pointer.shape =
                        Shape::convert(shape.Type, shape.Width, shape.Height, shape.Pitch, &buffer);
                    self.pointer.shape_changed = true;
                    changed |= self.pointer.visible;
                }
                Err(err) => tracing::debug!("screen share: pointer shape: {err}"),
            }
        }
        changed
    }

    /// Draws the pointer over `target`; leaves it out if the drawer cannot work on this GPU.
    fn draw_pointer(&mut self, target: &ID3D11Texture2D) {
        let drawer = self.drawer.get_or_insert_with(|| {
            CursorDrawer::new(&self.device).map_err(|err| {
                tracing::warn!("screen share: the mouse pointer will not be shown: {err}");
            })
        });
        let Ok(drawer) = drawer else {
            return;
        };
        if let Err(err) = drawer.draw(&self.device, &self.context, target, &mut self.pointer) {
            tracing::warn!("screen share: the mouse pointer will not be shown: {err}");
            self.drawer = Some(Err(()));
        }
    }

    /// The texture for `slot`, made again when the size changes.
    unsafe fn texture(
        &mut self,
        slot: Slot,
        width: u32,
        height: u32,
    ) -> Result<ID3D11Texture2D, Error> {
        let place = match slot {
            Slot::Staging => &mut self.staging,
            Slot::Clean => &mut self.clean,
            Slot::Composed => &mut self.composed,
        };
        if let Some((texture, w, h)) = place
            && (*w, *h) == (width, height)
        {
            return Ok(texture.clone());
        }
        let (usage, bind, cpu) = match slot {
            Slot::Staging => (D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32),
            Slot::Clean | Slot::Composed => (
                D3D11_USAGE_DEFAULT,
                (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
                0,
            ),
        };
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
            Usage: usage,
            BindFlags: bind,
            CPUAccessFlags: cpu,
            MiscFlags: 0,
        };
        let mut texture = None;
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut texture))?;
        }
        let texture = texture.ok_or(Error::Other("no copy texture".into()))?;
        *place = Some((texture.clone(), width, height));
        Ok(texture)
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

/// The duplicator's textures.
#[derive(Clone, Copy)]
enum Slot {
    Staging,
    Clean,
    Composed,
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
