//! Hardware H.264 decoding with Media Foundation: Windows' own H.264 decoder (kept up to date by
//! Windows Update), given a Direct3D 11 device so it decodes on the GPU (DXVA). Without one (no
//! GPU with video support) it decodes on the CPU, with several threads. Either way it hands out
//! NV12 pictures in memory, which `super::to_bgra` turns into images.
//!
//! A GPU picture is read back through a staging texture of our own: Media Foundation's own way
//! (locking the buffer) stalls for about 8 ms per picture whatever its size, against about 1 ms
//! for the decoding itself.
//!
//! [`super::codec::Decoder`] falls back to OpenH264 when this cannot be set up.

// Media Foundation and Direct3D are COM APIs: every call is `unsafe`.
#![allow(unsafe_code)]

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
    D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
    ID3D11Texture2D,
};
use windows::Win32::Media::MediaFoundation::{
    CLSID_MSH264DecoderMFT, IMF2DBuffer2, IMFDXGIBuffer, IMFDXGIDeviceManager, IMFMediaBuffer,
    IMFMediaType, IMFSample, IMFTransform, MF_E_TRANSFORM_NEED_MORE_INPUT,
    MF_E_TRANSFORM_STREAM_CHANGE, MF_LOW_LATENCY, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE,
    MF_MT_MAJOR_TYPE, MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_SUBTYPE, MF_SA_D3D11_AWARE, MF_VERSION,
    MF2DBuffer_LockFlags_Read, MFCreateDXGIDeviceManager, MFCreateMediaType, MFCreateMemoryBuffer,
    MFCreateSample, MFMediaType_Video, MFSTARTUP_LITE, MFShutdown, MFStartup,
    MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_END_STREAMING,
    MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_MESSAGE_SET_D3D_MANAGER, MFT_OUTPUT_DATA_BUFFER,
    MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFVideoArea,
    MFVideoFormat_H264, MFVideoFormat_NV12,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::core::Interface;

use super::codec::Picture;
use hermes_client_core::proto::voice::MAX_VIDEO_DIMENSION;

fn failed(what: &str) -> impl Fn(windows::core::Error) -> String + '_ {
    move |err| format!("{what}: {err}")
}

/// Decodes one person's share with Windows' H.264 decoder.
pub struct MfDecoder {
    transform: IMFTransform,
    _manager: Option<IMFDXGIDeviceManager>,
    /// The device the decoder decodes on, and where its pictures are read back.
    gpu: Option<(ID3D11Device, ID3D11DeviceContext)>,
    /// The CPU-readable copy of a decoded picture, reused while the size stays.
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    /// Decoding on the GPU (else on the CPU).
    pub hardware: bool,
    provides_samples: bool,
    output_size: u32,
    /// The size of the picture to show (the decoder's pictures may have a few more rows).
    display: (u32, u32),
}

impl MfDecoder {
    pub fn new() -> Result<MfDecoder, String> {
        unsafe {
            // Already initialized (in another mode) is fine.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(failed("Media Foundation"))?;
        }
        let made = Self::make();
        if made.is_err() {
            unsafe {
                let _ = MFShutdown();
            }
        }
        made
    }

    fn make() -> Result<MfDecoder, String> {
        unsafe {
            let transform: IMFTransform =
                CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER)
                    .map_err(failed("H.264 decoder"))?;
            let attributes = transform
                .GetAttributes()
                .map_err(failed("decoder attributes"))?;
            // Every picture as soon as it is decoded (screen shares have no B-frames).
            let _ = attributes.SetUINT32(&MF_LOW_LATENCY, 1);

            // On the GPU where possible.
            let mut gpu = None;
            if attributes
                .GetUINT32(&MF_SA_D3D11_AWARE)
                .is_ok_and(|v| v != 0)
                && let Ok((device, manager)) = gpu_device()
                && transform
                    .ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)
                    .is_ok()
            {
                gpu = Some((device, manager));
            }

            let input = MFCreateMediaType().map_err(failed("media type"))?;
            input
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(failed("media type"))?;
            input
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                .map_err(failed("media type"))?;
            transform
                .SetInputType(0, &input, 0)
                .map_err(failed("decoder input format"))?;
            let display = choose_nv12(&transform)?;
            let info = transform
                .GetOutputStreamInfo(0)
                .map_err(failed("decoder output"))?;
            let provides = (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0
                | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(failed("starting the decoder"))?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(failed("starting the decoder"))?;
            let hardware = gpu.is_some();
            let (device, manager) = gpu.unzip();
            let gpu = device.and_then(|d| {
                let context = d.GetImmediateContext().ok()?;
                Some((d, context))
            });
            Ok(MfDecoder {
                transform,
                _manager: manager,
                gpu,
                staging: None,
                hardware,
                provides_samples: info.dwFlags & provides != 0,
                output_size: info.cbSize,
                display,
            })
        }
    }

    /// Decodes one access unit; `Ok(None)` while the decoder waits for more data.
    pub fn decode(&mut self, data: &[u8]) -> Result<Option<Picture>, String> {
        let sample = input_sample(data)?;
        unsafe {
            self.transform
                .ProcessInput(0, &sample, 0)
                .map_err(failed("decoding"))?;
        }
        // Usually one picture comes out; keep the newest if there are more.
        let mut newest = None;
        while let Some(picture) = self.output()? {
            newest = Some(picture);
        }
        Ok(newest)
    }

    /// One decoded picture, if the decoder has one.
    fn output(&mut self) -> Result<Option<Picture>, String> {
        // A changed format (the first picture, or a new size) is taken and asked again.
        for _ in 0..3 {
            unsafe {
                let own = if self.provides_samples {
                    None
                } else {
                    let sample = MFCreateSample().map_err(failed("output sample"))?;
                    let buffer = MFCreateMemoryBuffer(self.output_size.max(1))
                        .map_err(failed("output buffer"))?;
                    sample.AddBuffer(&buffer).map_err(failed("output sample"))?;
                    Some(sample)
                };
                let mut buffer = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(own),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                };
                let mut status = 0u32;
                let result =
                    self.transform
                        .ProcessOutput(0, std::slice::from_mut(&mut buffer), &mut status);
                let sample = ManuallyDrop::take(&mut buffer.pSample);
                drop(ManuallyDrop::take(&mut buffer.pEvents));
                match result {
                    Ok(()) => {
                        let Some(sample) = sample else {
                            return Ok(None);
                        };
                        if let Some(picture) = self.read_gpu(&sample)? {
                            return Ok(Some(picture));
                        }
                        return read_nv12(&sample, &self.transform, self.display).map(Some);
                    }
                    Err(err) if err.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                    Err(err) if err.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                        self.display = choose_nv12(&self.transform)?;
                        let info = self
                            .transform
                            .GetOutputStreamInfo(0)
                            .map_err(failed("decoder output"))?;
                        self.output_size = info.cbSize;
                    }
                    Err(err) => return Err(format!("decoder output: {err}")),
                }
            }
        }
        Ok(None)
    }
}

impl MfDecoder {
    /// Reads a picture the decoder left on the GPU back through our staging texture; `None` if
    /// it is not on the GPU (decoded on the CPU).
    fn read_gpu(&mut self, sample: &IMFSample) -> Result<Option<Picture>, String> {
        let Some((device, context)) = &self.gpu else {
            return Ok(None);
        };
        unsafe {
            let buffer = sample
                .GetBufferByIndex(0)
                .map_err(failed("decoded picture"))?;
            let Ok(dxgi) = buffer.cast::<IMFDXGIBuffer>() else {
                return Ok(None);
            };
            let mut raw = std::ptr::null_mut();
            dxgi.GetResource(&ID3D11Texture2D::IID, &mut raw)
                .map_err(failed("decoded picture"))?;
            let texture = ID3D11Texture2D::from_raw(raw);
            let index = dxgi
                .GetSubresourceIndex()
                .map_err(failed("decoded picture"))?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            let fits = self
                .staging
                .as_ref()
                .is_some_and(|(_, w, h)| (*w, *h) == (desc.Width, desc.Height));
            if !fits {
                let staging_desc = D3D11_TEXTURE2D_DESC {
                    MipLevels: 1,
                    ArraySize: 1,
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                    ..desc
                };
                let mut staging = None;
                device
                    .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                    .map_err(failed("readback texture"))?;
                let staging = staging.ok_or("no readback texture")?;
                self.staging = Some((staging, desc.Width, desc.Height));
            }
            let Some((staging, _, rows)) = &self.staging else {
                return Ok(None);
            };
            context.CopySubresourceRegion(staging, 0, 0, 0, 0, &texture, index, None);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(failed("reading the picture"))?;
            let pitch = mapped.RowPitch as usize;
            let rows = *rows as usize;
            let all = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * rows * 3 / 2);
            let picture = nv12_from(all, pitch, rows, self.display);
            context.Unmap(staging, 0);
            picture.map(Some)
        }
    }
}

impl Drop for MfDecoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            let _ = MFShutdown();
        }
    }
}

/// A Direct3D 11 device with video support on the default GPU, and a device manager for it.
fn gpu_device() -> Result<(ID3D11Device, IMFDXGIDeviceManager), String> {
    unsafe {
        let mut device = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
        .map_err(failed("Direct3D 11 device"))?;
        let device = device.ok_or("no Direct3D 11 device")?;
        // The decoder uses the device from its own threads.
        if let Ok(multithread) = device.cast::<ID3D11Multithread>() {
            let _ = multithread.SetMultithreadProtected(true);
        }
        let mut token = 0u32;
        let mut manager: Option<IMFDXGIDeviceManager> = None;
        MFCreateDXGIDeviceManager(&mut token, &mut manager).map_err(failed("device manager"))?;
        let manager = manager.ok_or("no device manager")?;
        manager
            .ResetDevice(&device, token)
            .map_err(failed("device manager"))?;
        Ok((device, manager))
    }
}

/// Picks NV12 among the decoder's output formats; returns the picture size to show.
fn choose_nv12(transform: &IMFTransform) -> Result<(u32, u32), String> {
    unsafe {
        let mut index = 0;
        while let Ok(offered) = transform.GetOutputAvailableType(0, index) {
            index += 1;
            if offered.GetGUID(&MF_MT_SUBTYPE).ok() != Some(MFVideoFormat_NV12) {
                continue;
            }
            transform
                .SetOutputType(0, &offered, 0)
                .map_err(failed("decoder output format"))?;
            return Ok(display_size(&offered));
        }
        Err("the decoder offers no NV12 output".into())
    }
}

/// The size to show: the display aperture if the stream has one (the coded picture is a
/// multiple of 16 rows, 1088 for 1080), else the frame size. `(0, 0)` before the first picture.
fn display_size(t: &IMFMediaType) -> (u32, u32) {
    unsafe {
        let mut area = MFVideoArea::default();
        let bytes = std::slice::from_raw_parts_mut(
            std::ptr::from_mut(&mut area).cast::<u8>(),
            size_of::<MFVideoArea>(),
        );
        if t.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, bytes, None)
            .is_ok()
            && area.Area.cx > 0
            && area.Area.cy > 0
        {
            return (area.Area.cx as u32, area.Area.cy as u32);
        }
        t.GetUINT64(&MF_MT_FRAME_SIZE)
            .map_or((0, 0), |size| ((size >> 32) as u32, size as u32))
    }
}

/// A sample holding one access unit.
fn input_sample(data: &[u8]) -> Result<IMFSample, String> {
    unsafe {
        let len = u32::try_from(data.len()).map_err(|_| "frame too large")?;
        let buffer = MFCreateMemoryBuffer(len.max(1)).map_err(failed("input buffer"))?;
        let mut target: *mut u8 = std::ptr::null_mut();
        buffer
            .Lock(&mut target, None, None)
            .map_err(failed("input buffer"))?;
        if !target.is_null() {
            std::ptr::copy_nonoverlapping(data.as_ptr(), target, data.len());
        }
        let _ = buffer.Unlock();
        buffer
            .SetCurrentLength(len)
            .map_err(failed("input buffer"))?;
        let sample = MFCreateSample().map_err(failed("input sample"))?;
        sample.AddBuffer(&buffer).map_err(failed("input sample"))?;
        Ok(sample)
    }
}

/// Copies a decoded NV12 picture of `display` size out of `sample`: the Y rows, then the
/// interleaved U/V rows, without padding.
fn read_nv12(
    sample: &IMFSample,
    transform: &IMFTransform,
    (width, height): (u32, u32),
) -> Result<Picture, String> {
    unsafe {
        let buffer = sample
            .GetBufferByIndex(0)
            .map_err(failed("decoded picture"))?;
        // Decoding on the CPU, it writes into the plain buffer we gave it, which has no 2D access.
        let Ok(two_d) = buffer.cast::<IMF2DBuffer2>() else {
            return read_plain_nv12(&buffer, transform, (width, height));
        };
        let mut scanline: *mut u8 = std::ptr::null_mut();
        let mut pitch = 0i32;
        let mut start: *mut u8 = std::ptr::null_mut();
        let mut len = 0u32;
        two_d
            .Lock2DSize(
                MF2DBuffer_LockFlags_Read,
                &mut scanline,
                &mut pitch,
                &mut start,
                &mut len,
            )
            .map_err(failed("decoded picture"))?;
        let result = (|| {
            let pitch = usize::try_from(pitch).map_err(|_| "a bottom-up picture")?;
            if scanline.is_null() {
                return Err("the decoder returned no picture".to_owned());
            }
            // The buffer holds `rows` Y rows and `rows / 2` U/V rows of `pitch` bytes.
            let available = len as usize - (scanline as usize - start as usize);
            let rows = available * 2 / 3 / pitch.max(1);
            let all = std::slice::from_raw_parts(scanline, available);
            nv12_from(all, pitch, rows, (width, height))
        })();
        let _ = two_d.Unlock2D();
        result
    }
}

/// [`read_nv12`] for a plain buffer: its layout comes from the decoder's output format.
fn read_plain_nv12(
    buffer: &IMFMediaBuffer,
    transform: &IMFTransform,
    display: (u32, u32),
) -> Result<Picture, String> {
    unsafe {
        let format = transform
            .GetOutputCurrentType(0)
            .map_err(failed("decoder output format"))?;
        let frame = format
            .GetUINT64(&MF_MT_FRAME_SIZE)
            .map_err(failed("decoder output format"))?;
        let (columns, rows) = ((frame >> 32) as usize, frame as u32 as usize);
        // A stride is stored as a signed number; negative is bottom-up, which NV12 never is.
        let pitch = format
            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
            .map_or(columns, |stride| (stride as i32).unsigned_abs() as usize);
        let mut start: *mut u8 = std::ptr::null_mut();
        let mut len = 0u32;
        buffer
            .Lock(&mut start, None, Some(&mut len))
            .map_err(failed("decoded picture"))?;
        let result = if start.is_null() {
            Err("the decoder returned no picture".to_owned())
        } else {
            let all = std::slice::from_raw_parts(start, len as usize);
            nv12_from(all, pitch, rows, display)
        };
        let _ = buffer.Unlock();
        result
    }
}

/// Packs the top-left `width`×`height` of an NV12 picture in `all` (`rows` Y rows, then
/// `rows / 2` U/V rows, each `pitch` bytes) without padding.
fn nv12_from(
    all: &[u8],
    pitch: usize,
    rows: usize,
    (width, height): (u32, u32),
) -> Result<Picture, String> {
    let max = u32::from(MAX_VIDEO_DIMENSION);
    if width < 2 || height < 2 || width > max || height > max || width % 2 + height % 2 != 0 {
        return Err(format!("unexpected picture size {width}x{height}"));
    }
    let (w, h) = (width as usize, height as usize);
    if pitch < w || rows < h || all.len() < pitch * rows * 3 / 2 {
        return Err("the decoder returned a short picture".to_owned());
    }
    let mut planes = Vec::with_capacity(w * h * 3 / 2);
    for row in 0..h {
        planes.extend_from_slice(&all[row * pitch..row * pitch + w]);
    }
    let uv = rows * pitch;
    for row in 0..h / 2 {
        let at = uv + row * pitch;
        planes.extend_from_slice(&all[at..at + w]);
    }
    Ok(Picture {
        width,
        height,
        nv12: true,
        planes,
    })
}
