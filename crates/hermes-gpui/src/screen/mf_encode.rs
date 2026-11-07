//! Hardware H.264 encoding with Media Foundation: the encoder of the GPU the screen is connected
//! to (NVENC, AMF or Quick Sync, behind Windows' own interface), fed NV12 pictures that never
//! leave the GPU ([`super::gpu_convert`]). The output is the same Annex B H.264 the software
//! encoder makes, so watchers do not care which one made it.
//!
//! Hardware encoders are asynchronous: they say when they want a picture
//! (`METransformNeedInput`) and when one is encoded (`METransformHaveOutput`). A thread of its
//! own waits for these events (Media Foundation has no wait with a timeout) and passes them on,
//! so every wait here is bounded: a driver that stops answering ends the share instead of
//! hanging it.

// Media Foundation and Direct3D are COM APIs: every call is `unsafe`.
#![allow(unsafe_code)]

use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncCommonBufferSize, CODECAPI_AVEncCommonMeanBitRate,
    CODECAPI_AVEncCommonRateControlMode, CODECAPI_AVEncMPVDefaultBPictureCount,
    CODECAPI_AVEncMPVGOPSize, CODECAPI_AVEncVideoForceKeyFrame, CODECAPI_AVLowLatencyMode,
    ICodecAPI, IMF2DBuffer, IMFActivate, IMFAttributes, IMFDXGIDeviceManager,
    IMFMediaEventGenerator, IMFMediaType, IMFSample, IMFTransform,
    MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS, METransformHaveOutput, METransformNeedInput,
    MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE, MF_MT_AVG_BITRATE,
    MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_MT_VIDEO_NOMINAL_RANGE,
    MF_MT_YUV_MATRIX, MF_TRANSFORM_ASYNC, MF_TRANSFORM_ASYNC_UNLOCK, MF_VERSION,
    MFCreateAttributes, MFCreateDXGIDeviceManager, MFCreateDXGISurfaceBuffer, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video, MFNominalRange_16_235, MFSTARTUP_LITE,
    MFSampleExtension_CleanPoint, MFShutdown, MFShutdownObject, MFStartup,
    MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_ADAPTER_LUID, MFT_ENUM_FLAG_HARDWARE,
    MFT_ENUM_FLAG_SORTANDFILTER, MFT_FRIENDLY_NAME_Attribute, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_END_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
    MFT_MESSAGE_SET_D3D_MANAGER, MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES,
    MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFT_REGISTER_TYPE_INFO, MFTEnum2, MFVideoFormat_H264,
    MFVideoFormat_NV12, MFVideoInterlace_Progressive, MFVideoTransferMatrix_BT601,
    eAVEncCommonRateControlMode_CBR, eAVEncH264VProfile_High,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{Interface, PWSTR};

use super::gpu_convert::Nv12;

/// How long the encoder may take to want the next picture before the share is given up.
const INPUT_TIMEOUT: Duration = Duration::from_millis(500);
/// How long to wait for an encoded picture after handing one over (more than any hardware
/// encoder needs for one 4K frame).
const OUTPUT_TIMEOUT: Duration = Duration::from_millis(100);
/// Once the encoder wants the next picture without having returned the last one, how much
/// longer to wait for it.
const PIPELINED_TIMEOUT: Duration = Duration::from_millis(20);

/// Media Foundation time units per second.
const HNS: i64 = 10_000_000;

fn failed(what: &str) -> impl Fn(windows::core::Error) -> String + '_ {
    move |err| format!("{what}: {err}")
}

/// One encoded access unit and whether it is a keyframe.
pub type Encoded = (Vec<u8>, bool);

/// A hardware H.264 encoder for pictures of one size.
pub struct MfEncoder {
    transform: IMFTransform,
    /// The encoder's events (asynchronous encoders, which hardware ones are), from
    /// [`event_thread`].
    events: Option<std::sync::mpsc::Receiver<u32>>,
    codec: Option<ICodecAPI>,
    _manager: IMFDXGIDeviceManager,
    /// Pictures the encoder asked for and has not got yet.
    credits: u32,
    /// The encoder hands out its own output samples; otherwise one of `output_size` is made.
    provides_samples: bool,
    output_size: u32,
    fps: u32,
    frames: i64,
    /// The encoder's name ("NVIDIA H.264 Encoder MFT"), for the logs.
    pub name: String,
}

impl MfEncoder {
    /// The hardware encoder of the GPU `luid` (the one `device` is on), for `width`×`height`
    /// NV12 pictures at `fps`, at `bitrate` bits per second.
    pub fn new(
        device: &ID3D11Device,
        luid: LUID,
        (width, height): (u32, u32),
        fps: u32,
        bitrate: u32,
    ) -> Result<MfEncoder, String> {
        unsafe {
            // Already initialized (in another mode) is fine.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(failed("Media Foundation"))?;
        }
        let mut errors = Vec::new();
        for activate in enumerate(luid).inspect_err(|_| unsafe {
            let _ = MFShutdown();
        })? {
            match configure(&activate, device, (width, height), fps, bitrate) {
                Ok(encoder) => return Ok(encoder),
                Err(err) => errors.push(err),
            }
        }
        unsafe {
            let _ = MFShutdown();
        }
        Err(if errors.is_empty() {
            "no hardware H.264 encoder on this GPU".to_owned()
        } else {
            errors.join("; ")
        })
    }

    /// Changes the bitrate of the next pictures.
    pub fn set_bitrate(&self, bitrate: u32) {
        if let Some(codec) = &self.codec {
            unsafe {
                let _ = codec.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &VARIANT::from(bitrate));
            }
        }
    }

    /// Encodes one picture (a keyframe if `keyframe`). Returns what came out: usually that
    /// picture, sometimes nothing yet (the encoder works one picture behind).
    pub fn encode(&mut self, picture: &Nv12, keyframe: bool) -> Result<Vec<Encoded>, String> {
        let sample = self.sample(&picture.texture)?;
        if keyframe && let Some(codec) = &self.codec {
            unsafe {
                let _ = codec.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &VARIANT::from(1u32));
            }
        }
        let mut out = Vec::new();
        if self.events.is_some() {
            self.encode_async(&sample, &mut out)?;
        } else {
            self.encode_sync(&sample, &mut out)?;
        }
        Ok(out)
    }

    /// A sample holding the GPU picture, stamped with its time.
    fn sample(&mut self, texture: &ID3D11Texture2D) -> Result<IMFSample, String> {
        unsafe {
            let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, texture, 0, false)
                .map_err(failed("picture buffer"))?;
            if let Ok(two_d) = buffer.cast::<IMF2DBuffer>()
                && let Ok(len) = two_d.GetContiguousLength()
            {
                let _ = buffer.SetCurrentLength(len);
            }
            let sample = MFCreateSample().map_err(failed("sample"))?;
            sample.AddBuffer(&buffer).map_err(failed("sample"))?;
            let duration = HNS / i64::from(self.fps.max(1));
            let _ = sample.SetSampleTime(self.frames * duration);
            let _ = sample.SetSampleDuration(duration);
            self.frames += 1;
            Ok(sample)
        }
    }

    fn encode_async(&mut self, sample: &IMFSample, out: &mut Vec<Encoded>) -> Result<(), String> {
        // Wait until the encoder wants a picture, collecting what it finishes meanwhile.
        let deadline = Instant::now() + INPUT_TIMEOUT;
        while self.credits == 0 {
            match self.next_event(deadline)? {
                Some(event) if event == METransformNeedInput.0 as u32 => self.credits += 1,
                Some(event) if event == METransformHaveOutput.0 as u32 => self.collect(out)?,
                Some(_) => {}
                None => return Err("the hardware encoder stopped asking for pictures".into()),
            }
        }
        unsafe {
            self.transform
                .ProcessInput(0, sample, 0)
                .map_err(failed("encoding"))?;
        }
        self.credits -= 1;
        // Wait for this picture to come out; an encoder that wants the next one first works a
        // picture behind, and its output comes with a later call.
        let mut deadline = Instant::now() + OUTPUT_TIMEOUT;
        let before = out.len();
        while out.len() == before {
            match self.next_event(deadline)? {
                Some(event) if event == METransformHaveOutput.0 as u32 => self.collect(out)?,
                Some(event) if event == METransformNeedInput.0 as u32 => {
                    self.credits += 1;
                    deadline = deadline.min(Instant::now() + PIPELINED_TIMEOUT);
                }
                Some(_) => {}
                None => break,
            }
        }
        Ok(())
    }

    fn encode_sync(&mut self, sample: &IMFSample, out: &mut Vec<Encoded>) -> Result<(), String> {
        unsafe {
            self.transform
                .ProcessInput(0, sample, 0)
                .map_err(failed("encoding"))?;
        }
        loop {
            let before = out.len();
            match self.collect(out) {
                Ok(()) if out.len() > before => {}
                result => return result,
            }
        }
    }

    /// The encoder's next event within `deadline` (`None` when it did not come in time).
    fn next_event(&self, deadline: Instant) -> Result<Option<u32>, String> {
        let Some(events) = &self.events else {
            return Ok(None);
        };
        let wait = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(wait) {
            Ok(event) => Ok(Some(event)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err("the hardware encoder stopped".into())
            }
        }
    }

    /// Takes one encoded picture out of the encoder (none if it has none).
    fn collect(&mut self, out: &mut Vec<Encoded>) -> Result<(), String> {
        unsafe {
            let own = if self.provides_samples {
                None
            } else {
                let sample = MFCreateSample().map_err(failed("output sample"))?;
                let buffer = MFCreateMemoryBuffer(self.output_size.max(1024 * 1024))
                    .map_err(failed("output"))?;
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
                Ok(()) => {}
                Err(err) if err.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                Err(err) if err.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    // The encoder changed its output format: take the one it offers now.
                    let offered = self
                        .transform
                        .GetOutputAvailableType(0, 0)
                        .map_err(failed("encoder output"))?;
                    self.transform
                        .SetOutputType(0, &offered, 0)
                        .map_err(failed("encoder output"))?;
                    return Ok(());
                }
                Err(err) => return Err(format!("encoder output: {err}")),
            }
            let Some(sample) = sample else {
                return Ok(());
            };
            let data = read_sample(&sample)?;
            if data.is_empty() {
                return Ok(());
            }
            let keyframe = sample
                .GetUINT32(&MFSampleExtension_CleanPoint)
                .is_ok_and(|v| v != 0)
                || has_idr(&data);
            out.push((data, keyframe));
            Ok(())
        }
    }
}

impl Drop for MfEncoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            if self.events.is_some() {
                // Asynchronous encoders run threads of their own until shut down.
                let _ = MFShutdownObject(&self.transform);
            }
            let _ = MFShutdown();
        }
    }
}

/// The hardware H.264 encoders of the GPU `luid`, best first.
fn enumerate(luid: LUID) -> Result<Vec<IMFActivate>, String> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    unsafe {
        let mut attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attributes, 1).map_err(failed("encoder search"))?;
        let attributes = attributes.ok_or("encoder search: no attributes")?;
        let mut luid_bytes = [0u8; 8];
        luid_bytes[..4].copy_from_slice(&luid.LowPart.to_le_bytes());
        luid_bytes[4..].copy_from_slice(&luid.HighPart.to_le_bytes());
        attributes
            .SetBlob(&MFT_ENUM_ADAPTER_LUID, &luid_bytes)
            .map_err(failed("encoder search"))?;
        let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        MFTEnum2(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            Some(&output),
            &attributes,
            &mut list,
            &mut count,
        )
        .map_err(failed("encoder search"))?;
        let mut found = Vec::new();
        if !list.is_null() {
            for i in 0..count as usize {
                if let Some(activate) = (*list.add(i)).take() {
                    found.push(activate);
                }
            }
            CoTaskMemFree(Some(list as *const _));
        }
        Ok(found)
    }
}

/// Activates and sets up one encoder.
fn configure(
    activate: &IMFActivate,
    device: &ID3D11Device,
    (width, height): (u32, u32),
    fps: u32,
    bitrate: u32,
) -> Result<MfEncoder, String> {
    unsafe {
        let name = friendly_name(activate);
        let transform: IMFTransform = activate
            .ActivateObject()
            .map_err(failed("starting the encoder"))?;
        let attributes = transform
            .GetAttributes()
            .map_err(failed("encoder attributes"))?;
        let asynchronous = attributes
            .GetUINT32(&MF_TRANSFORM_ASYNC)
            .is_ok_and(|v| v != 0);
        if asynchronous {
            attributes
                .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                .map_err(failed("unlocking the encoder"))?;
        }

        let mut token = 0u32;
        let mut manager: Option<IMFDXGIDeviceManager> = None;
        MFCreateDXGIDeviceManager(&mut token, &mut manager).map_err(failed("device manager"))?;
        let manager = manager.ok_or("no device manager")?;
        manager
            .ResetDevice(device, token)
            .map_err(failed("device manager"))?;
        transform
            .ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)
            .map_err(failed("giving the encoder the GPU"))?;

        // Low latency, constant bitrate, no B-frames (each would delay the picture), keyframes
        // only when asked for. Encoders that do not know a setting keep their own.
        let codec = transform.cast::<ICodecAPI>().ok();
        if let Some(codec) = &codec {
            let set = |api, value: VARIANT| {
                let _ = codec.SetValue(api, &value);
            };
            set(&CODECAPI_AVLowLatencyMode, VARIANT::from(true));
            set(
                &CODECAPI_AVEncCommonRateControlMode,
                VARIANT::from(eAVEncCommonRateControlMode_CBR.0 as u32),
            );
            set(&CODECAPI_AVEncCommonMeanBitRate, VARIANT::from(bitrate));
            set(
                &CODECAPI_AVEncCommonBufferSize,
                VARIANT::from(buffer_size(bitrate)),
            );
            set(&CODECAPI_AVEncMPVDefaultBPictureCount, VARIANT::from(0u32));
            set(
                &CODECAPI_AVEncMPVGOPSize,
                VARIANT::from(fps.saturating_mul(60)),
            );
        }

        let output = media_type(MFVideoFormat_H264, (width, height), fps)?;
        let _ = output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate);
        let _ = output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32);
        transform
            .SetOutputType(0, &output, 0)
            .map_err(failed("encoder output format"))?;
        let input = media_type(MFVideoFormat_NV12, (width, height), fps)?;
        transform
            .SetInputType(0, &input, 0)
            .map_err(failed("encoder input format"))?;

        let info = transform
            .GetOutputStreamInfo(0)
            .map_err(failed("encoder output"))?;
        let provides =
            (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32;
        let events = if asynchronous {
            let generator = transform
                .cast::<IMFMediaEventGenerator>()
                .map_err(failed("encoder events"))?;
            Some(event_thread(generator)?)
        } else {
            None
        };
        transform
            .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
            .map_err(failed("starting the encoder"))?;
        transform
            .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
            .map_err(failed("starting the encoder"))?;
        Ok(MfEncoder {
            transform,
            events,
            codec,
            _manager: manager,
            credits: 0,
            provides_samples: info.dwFlags & provides != 0,
            output_size: info.cbSize,
            fps,
            frames: 0,
            name,
        })
    }
}

/// An encoder's event generator, handed to the thread that waits on it.
struct Events(IMFMediaEventGenerator);

// SAFETY: Media Foundation objects live in the multithreaded apartment (the encoder thread
// joined it in `MfEncoder::new`) and are free-threaded; the generator is only used by the
// event thread once handed over.
#[allow(unsafe_code)]
unsafe impl Send for Events {}

/// Starts the thread that waits for the encoder's events and passes their types on. It ends
/// when the encoder is shut down ([`MfEncoder`]'s `Drop`) or the receiver is dropped.
fn event_thread(
    generator: IMFMediaEventGenerator,
) -> Result<std::sync::mpsc::Receiver<u32>, String> {
    let (events, received) = std::sync::mpsc::channel();
    let generator = Events(generator);
    std::thread::Builder::new()
        .name("hermes-encoder-events".into())
        .spawn(move || {
            let generator = generator;
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            loop {
                let event = unsafe {
                    generator
                        .0
                        .GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0))
                };
                let Ok(event) = event else {
                    return;
                };
                let Ok(kind) = (unsafe { event.GetType() }) else {
                    return;
                };
                if events.send(kind).is_err() {
                    return;
                }
            }
        })
        .map_err(|e| format!("encoder event thread: {e}"))?;
    Ok(received)
}

/// A video media type of `subtype`: progressive, square pixels, limited-range BT.601.
fn media_type(
    subtype: windows::core::GUID,
    (width, height): (u32, u32),
    fps: u32,
) -> Result<IMFMediaType, String> {
    unsafe {
        let t = MFCreateMediaType().map_err(failed("media type"))?;
        let pair = |a: u32, b: u32| (u64::from(a) << 32) | u64::from(b);
        let set = || -> windows::core::Result<()> {
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            t.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
            t.SetUINT64(&MF_MT_FRAME_SIZE, pair(width, height))?;
            t.SetUINT64(&MF_MT_FRAME_RATE, pair(fps, 1))?;
            t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pair(1, 1))?;
            t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT601.0 as u32)?;
            t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
            Ok(())
        };
        set().map_err(failed("media type"))?;
        Ok(t)
    }
}

/// The rate control buffer: half a second, but small enough that a keyframe stays well under
/// the protocol's frame limit.
fn buffer_size(bitrate: u32) -> u32 {
    (bitrate / 2).min(12_000_000)
}

/// The encoder's display name, or "hardware encoder".
fn friendly_name(activate: &IMFActivate) -> String {
    unsafe {
        let mut name = PWSTR::null();
        let mut len = 0u32;
        if activate
            .GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut name, &mut len)
            .is_err()
            || name.is_null()
        {
            return "hardware encoder".to_owned();
        }
        let text = name.to_string().unwrap_or_default();
        CoTaskMemFree(Some(name.0 as *const _));
        text
    }
}

/// The bytes of an output sample.
fn read_sample(sample: &IMFSample) -> Result<Vec<u8>, String> {
    unsafe {
        let buffer = sample
            .ConvertToContiguousBuffer()
            .map_err(failed("encoder output"))?;
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut len = 0u32;
        buffer
            .Lock(&mut data, None, Some(&mut len))
            .map_err(failed("encoder output"))?;
        let bytes = if data.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(data, len as usize).to_vec()
        };
        let _ = buffer.Unlock();
        Ok(bytes)
    }
}

/// Whether an Annex B access unit holds an IDR picture (NAL unit type 5).
pub fn has_idr(data: &[u8]) -> bool {
    data.windows(4)
        .any(|w| w[..3] == [0, 0, 1] && w[3] & 0x1f == 5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idr_pictures_are_found() {
        // SPS, PPS, IDR slice.
        assert!(has_idr(&[
            0, 0, 0, 1, 0x67, 1, 0, 0, 1, 0x68, 2, 0, 0, 1, 0x65, 3
        ]));
        // A non-IDR slice only.
        assert!(!has_idr(&[0, 0, 0, 1, 0x41, 9, 9]));
        assert!(!has_idr(&[]));
        assert_eq!(buffer_size(40_000_000), 12_000_000);
        assert_eq!(buffer_size(8_000_000), 4_000_000);
    }
}
