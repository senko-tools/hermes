//! Audio devices through cpal: listing, picking by name, and opening streams that only move
//! samples between the device and a ring buffer (PLAN.md 12.5 "Rules for audio code").

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, ErrorKind, FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

/// Input and output devices by name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioDevices {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub default_input: Option<String>,
    pub default_output: Option<String>,
}

pub fn device_name(d: &Device) -> String {
    d.description()
        .map(|desc| desc.name().to_string())
        .or_else(|_| d.id().map(|id| id.to_string()))
        .unwrap_or_else(|_| "<unknown>".to_string())
}

/// Every input and output device of the default host.
pub fn list() -> AudioDevices {
    let host = cpal::default_host();
    let names = |devices: Result<_, _>| -> Vec<String> {
        match devices {
            Ok(devices) => {
                let devices: Vec<Device> = Iterator::collect(devices);
                devices.iter().map(device_name).collect()
            }
            Err(_) => Vec::new(),
        }
    };
    AudioDevices {
        inputs: names(host.input_devices()),
        outputs: names(host.output_devices()),
        default_input: host.default_input_device().map(|d| device_name(&d)),
        default_output: host.default_output_device().map(|d| device_name(&d)),
    }
}

/// `name` (`None` or `"default"` = the default device) for input or output. `Err` if there is
/// no such device.
pub fn find(name: Option<&str>, input: bool) -> Result<Device, String> {
    let host = cpal::default_host();
    let kind = if input { "input" } else { "output" };
    let wanted = name.filter(|n| !n.is_empty() && *n != "default");
    let Some(wanted) = wanted else {
        let device = if input {
            host.default_input_device()
        } else {
            host.default_output_device()
        };
        return device.ok_or_else(|| format!("no {kind} device"));
    };
    let devices = if input {
        host.input_devices()
    } else {
        host.output_devices()
    };
    let mut devices = devices.map_err(|e| format!("cannot list {kind} devices: {e}"))?;
    devices
        .find(|d| device_name(d) == wanted)
        .ok_or_else(|| format!("{kind} device \"{wanted}\" not found"))
}

/// An open device stream. Dropping it stops the stream.
pub struct OpenStream {
    _stream: Stream,
    pub name: String,
    pub rate: u32,
    pub channels: usize,
    /// Set by the error callback when the stream must be rebuilt (device lost).
    pub failed: Arc<AtomicBool>,
    /// Input overflows / output underruns.
    pub glitches: Arc<AtomicU64>,
}

pub struct Input {
    pub stream: OpenStream,
    /// Interleaved samples at the device rate.
    pub ring: HeapCons<f32>,
}

pub struct Output {
    pub stream: OpenStream,
    /// Mono samples at the device rate; the callback copies each to all channels.
    pub ring: HeapProd<f32>,
}

/// Errors that need a new stream; the others (xruns, followed route changes) do not.
fn needs_rebuild(kind: ErrorKind) -> bool {
    !matches!(
        kind,
        ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied
    )
}

fn error_callback(failed: Arc<AtomicBool>) -> impl FnMut(cpal::Error) + Send + 'static {
    move |err| {
        if needs_rebuild(err.kind()) {
            failed.store(true, Ordering::Relaxed);
        }
    }
}

macro_rules! dispatch {
    ($fmt:expr, $f:ident($($arg:expr),*)) => {
        match $fmt {
            SampleFormat::I8 => $f::<i8>($($arg),*),
            SampleFormat::I16 => $f::<i16>($($arg),*),
            SampleFormat::I32 => $f::<i32>($($arg),*),
            SampleFormat::U8 => $f::<u8>($($arg),*),
            SampleFormat::U16 => $f::<u16>($($arg),*),
            SampleFormat::U32 => $f::<u32>($($arg),*),
            SampleFormat::F32 => $f::<f32>($($arg),*),
            SampleFormat::F64 => $f::<f64>($($arg),*),
            other => Err(format!("unsupported sample format {other}")),
        }
    };
}

pub fn open_input(device: &Device) -> Result<Input, String> {
    let supported = device
        .default_input_config()
        .map_err(|e| format!("input device has no usable format: {e}"))?;
    let config: StreamConfig = supported.config();
    let rate = supported.sample_rate();
    let channels = usize::from(supported.channels());
    // Half a second of headroom between the callback and the worker.
    let (prod, ring) = HeapRb::<f32>::new(rate as usize * channels / 2).split();
    let failed = Arc::new(AtomicBool::new(false));
    let glitches = Arc::new(AtomicU64::new(0));
    let stream = dispatch!(
        supported.sample_format(),
        input_stream(device, config, prod, failed.clone(), glitches.clone())
    )?;
    stream
        .play()
        .map_err(|e| format!("cannot start the input: {e}"))?;
    Ok(Input {
        stream: OpenStream {
            _stream: stream,
            name: device_name(device),
            rate,
            channels,
            failed,
            glitches,
        },
        ring,
    })
}

fn input_stream<T>(
    device: &Device,
    config: StreamConfig,
    mut prod: HeapProd<f32>,
    failed: Arc<AtomicBool>,
    glitches: Arc<AtomicU64>,
) -> Result<Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    // Only moves samples: no allocation, locking or logging.
    let callback = move |data: &[T], _: &cpal::InputCallbackInfo| {
        let mut lost = false;
        for s in data {
            lost |= prod.try_push(s.to_sample::<f32>()).is_err();
        }
        if lost {
            glitches.fetch_add(1, Ordering::Relaxed);
        }
    };
    device
        .build_input_stream(config, callback, error_callback(failed), None)
        .map_err(|e| format!("cannot open the input: {e}"))
}

pub fn open_output(device: &Device) -> Result<Output, String> {
    let supported = device
        .default_output_config()
        .map_err(|e| format!("output device has no usable format: {e}"))?;
    let config: StreamConfig = supported.config();
    let rate = supported.sample_rate();
    let channels = usize::from(supported.channels());
    let (mut ring, cons) = HeapRb::<f32>::new(rate as usize / 2).split();
    // 10 ms of silence so the first callback does not underrun.
    for _ in 0..rate / 100 {
        let _ = ring.try_push(0.0);
    }
    let failed = Arc::new(AtomicBool::new(false));
    let glitches = Arc::new(AtomicU64::new(0));
    let stream = dispatch!(
        supported.sample_format(),
        output_stream(device, config, cons, failed.clone(), glitches.clone())
    )?;
    stream
        .play()
        .map_err(|e| format!("cannot start the output: {e}"))?;
    Ok(Output {
        stream: OpenStream {
            _stream: stream,
            name: device_name(device),
            rate,
            channels,
            failed,
            glitches,
        },
        ring,
    })
}

fn output_stream<T>(
    device: &Device,
    config: StreamConfig,
    mut cons: HeapCons<f32>,
    failed: Arc<AtomicBool>,
    glitches: Arc<AtomicU64>,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels).max(1);
    // Only moves samples: no allocation, locking or logging.
    let callback = move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
        let mut short = false;
        for frame in data.chunks_mut(channels) {
            let v = match cons.try_pop() {
                Some(v) => T::from_sample(v),
                None => {
                    short = true;
                    T::EQUILIBRIUM
                }
            };
            frame.fill(v);
        }
        if short {
            glitches.fetch_add(1, Ordering::Relaxed);
        }
    };
    device
        .build_output_stream(config, callback, error_callback(failed), None)
        .map_err(|e| format!("cannot open the output: {e}"))
}
