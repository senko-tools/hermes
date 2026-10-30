//! Spike A (PLAN.md section 20, Phase 0): audio loopback.
//!
//! cpal default input -> mono f32 -> rubato to 48 kHz -> nnnoiseless -> Opus encode
//! (20 ms, 64 kbps, FEC, DTX) -> Opus decode -> resample -> cpal default output,
//! printing device info and an estimated added latency once per second.

mod pipeline;

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;

use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    Device, FromSample, Host, InputCallbackInfo, OutputCallbackInfo, SampleFormat, SizedSample,
    Stream, StreamConfig, SupportedBufferSize, SupportedStreamConfig,
};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

use pipeline::{CallbackStats, Config, DeviceInfo};

#[derive(Parser)]
#[command(about = "Hermes Spike A: microphone -> denoise -> Opus -> speakers loopback")]
struct Args {
    /// Skip nnnoiseless noise suppression.
    #[arg(long)]
    no_denoise: bool,
    /// Extra microphone gain in dB (e.g. 12 = 4x louder), for testing quiet microphones.
    #[arg(
        long,
        value_name = "DB",
        default_value_t = 0.0,
        allow_negative_numbers = true
    )]
    gain: f32,
    /// List audio devices and their default configs, then exit.
    #[arg(long)]
    list_devices: bool,
    /// Input device: exact name, or a case-insensitive part of it (default: system default).
    #[arg(long, value_name = "NAME")]
    input: Option<String>,
    /// Output device: exact name, or a case-insensitive part of it (default: system default).
    #[arg(long, value_name = "NAME")]
    output: Option<String>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), String> {
    let host = cpal::default_host();
    if args.list_devices {
        list_devices(&host);
        return Ok(());
    }

    let in_dev = pick_device(&host, args.input.as_deref(), true)?;
    let out_dev = pick_device(&host, args.output.as_deref(), false)?;
    let in_cfg = in_dev
        .default_input_config()
        .map_err(|e| format!("input device has no default config: {e}"))?;
    let out_cfg = out_dev
        .default_output_config()
        .map_err(|e| format!("output device has no default config: {e}"))?;

    let in_rate = in_cfg.sample_rate();
    let in_ch = usize::from(in_cfg.channels());
    let out_rate = out_cfg.sample_rate();

    // Half a second of headroom each way; the worker keeps the queues short.
    let (in_prod, in_cons) = HeapRb::<f32>::new(in_rate as usize * in_ch / 2).split();
    let (mut out_prod, out_cons) = HeapRb::<f32>::new(out_rate as usize / 2).split();
    // 10 ms of silence so the output callback does not underrun before the first frame.
    for _ in 0..out_rate / 100 {
        let _ = out_prod.try_push(0.0);
    }

    let stats = Arc::new(CallbackStats::default());
    let in_stream = build_input(&in_dev, &in_cfg, in_prod, stats.clone())?;
    let out_stream = build_output(&out_dev, &out_cfg, out_cons, stats.clone())?;

    let cfg = Config {
        input: device_info(&in_dev, &in_cfg),
        output: device_info(&out_dev, &out_cfg),
        denoise: !args.no_denoise,
        gain_db: args.gain,
    };

    in_stream
        .play()
        .map_err(|e| format!("failed to start input: {e}"))?;
    out_stream
        .play()
        .map_err(|e| format!("failed to start output: {e}"))?;

    // The streams must stay alive on this thread; the worker runs until Ctrl+C.
    let worker = thread::Builder::new()
        .name("dsp".into())
        .spawn(move || pipeline::run(cfg, in_cons, out_prod, stats))
        .map_err(|e| format!("failed to start worker: {e}"))?;
    let _ = worker.join();
    drop((in_stream, out_stream));
    Ok(())
}

fn device_name(d: &Device) -> String {
    d.description()
        .map(|desc| desc.name().to_string())
        .or_else(|_| d.id().map(|id| id.to_string()))
        .unwrap_or_else(|_| "<unknown>".to_string())
}

fn pick_device(host: &Host, wanted: Option<&str>, input: bool) -> Result<Device, String> {
    let kind = if input { "input" } else { "output" };
    let Some(wanted) = wanted else {
        let d = if input {
            host.default_input_device()
        } else {
            host.default_output_device()
        };
        return d.ok_or_else(|| format!("no default {kind} device"));
    };
    let devices: Vec<Device> = if input {
        host.input_devices().map(|d| d.collect())
    } else {
        host.output_devices().map(|d| d.collect())
    }
    .map_err(|e| format!("cannot list {kind} devices: {e}"))?;

    if let Some(d) = devices.iter().find(|d| device_name(d) == wanted) {
        return Ok(d.clone());
    }
    let needle = wanted.to_lowercase();
    let mut matches: Vec<&Device> = devices
        .iter()
        .filter(|d| device_name(d).to_lowercase().contains(&needle))
        .collect();
    match matches.len() {
        1 => Ok(matches.remove(0).clone()),
        0 => Err(format!(
            "no {kind} device matches \"{wanted}\" (see --list-devices)"
        )),
        _ => Err(format!(
            "\"{wanted}\" matches several {kind} devices: {}",
            matches
                .iter()
                .map(|d| format!("\"{}\"", device_name(d)))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn buffer_range(b: &SupportedBufferSize) -> String {
    match b {
        SupportedBufferSize::Range { min, max } => format!("{min}..={max} frames"),
        SupportedBufferSize::Unknown => "unknown".to_string(),
    }
}

fn device_info(d: &Device, c: &SupportedStreamConfig) -> DeviceInfo {
    DeviceInfo {
        name: device_name(d),
        rate: c.sample_rate(),
        channels: usize::from(c.channels()),
        format: c.sample_format().to_string(),
        buffer_range: buffer_range(c.buffer_size()),
    }
}

fn describe(c: &SupportedStreamConfig) -> String {
    format!(
        "{} Hz, {} ch, {}, buffer size {}",
        c.sample_rate(),
        c.channels(),
        c.sample_format(),
        buffer_range(c.buffer_size())
    )
}

fn list_devices(host: &Host) {
    println!("host: {}", host.id().name());
    let default_in = host.default_input_device().map(|d| device_name(&d));
    let default_out = host.default_output_device().map(|d| device_name(&d));
    for (kind, input) in [("Input", true), ("Output", false)] {
        println!("\n{kind} devices:");
        let devices = if input {
            host.input_devices().map(|d| d.collect::<Vec<_>>())
        } else {
            host.output_devices().map(|d| d.collect::<Vec<_>>())
        };
        let devices = match devices {
            Ok(d) => d,
            Err(e) => {
                println!("  error: {e}");
                continue;
            }
        };
        let default = if input { &default_in } else { &default_out };
        for d in devices {
            let name = device_name(&d);
            let mark = if default.as_deref() == Some(name.as_str()) {
                " (default)"
            } else {
                ""
            };
            let cfg = if input {
                d.default_input_config()
            } else {
                d.default_output_config()
            };
            match cfg {
                Ok(c) => println!("  \"{name}\"{mark}: {}", describe(&c)),
                Err(e) => println!("  \"{name}\"{mark}: no default config ({e})"),
            }
        }
    }
}

fn stream_err(kind: &'static str) -> impl FnMut(cpal::Error) + Send + 'static {
    move |e| eprintln!("{kind} stream error: {e}")
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

fn build_input(
    d: &Device,
    c: &SupportedStreamConfig,
    prod: HeapProd<f32>,
    stats: Arc<CallbackStats>,
) -> Result<Stream, String> {
    let config: StreamConfig = c.config();
    dispatch!(c.sample_format(), input_stream(d, config, prod, stats))
}

fn input_stream<T>(
    d: &Device,
    config: StreamConfig,
    mut prod: HeapProd<f32>,
    stats: Arc<CallbackStats>,
) -> Result<Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = u64::from(config.channels);
    let callback = move |data: &[T], info: &InputCallbackInfo| {
        let ts = info.timestamp();
        let lat = ts.callback.duration_since(ts.capture);
        stats.in_callbacks.fetch_add(1, Ordering::Relaxed);
        stats
            .in_frames
            .fetch_add(data.len() as u64 / channels, Ordering::Relaxed);
        stats
            .in_device_latency_us
            .fetch_add(lat.as_micros() as u64, Ordering::Relaxed);
        let mut lost = false;
        for s in data {
            lost |= prod.try_push(s.to_sample::<f32>()).is_err();
        }
        if lost {
            stats.in_overflows.fetch_add(1, Ordering::Relaxed);
        }
    };
    d.build_input_stream(config, callback, stream_err("input"), None)
        .map_err(|e| format!("failed to open input stream: {e}"))
}

fn build_output(
    d: &Device,
    c: &SupportedStreamConfig,
    cons: HeapCons<f32>,
    stats: Arc<CallbackStats>,
) -> Result<Stream, String> {
    let config: StreamConfig = c.config();
    dispatch!(c.sample_format(), output_stream(d, config, cons, stats))
}

fn output_stream<T>(
    d: &Device,
    config: StreamConfig,
    mut cons: HeapCons<f32>,
    stats: Arc<CallbackStats>,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let callback = move |data: &mut [T], info: &OutputCallbackInfo| {
        let ts = info.timestamp();
        let lat = ts.playback.duration_since(ts.callback);
        stats.out_callbacks.fetch_add(1, Ordering::Relaxed);
        stats
            .out_frames
            .fetch_add((data.len() / channels) as u64, Ordering::Relaxed);
        stats
            .out_device_latency_us
            .fetch_add(lat.as_micros() as u64, Ordering::Relaxed);
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
            stats.out_underruns.fetch_add(1, Ordering::Relaxed);
        }
    };
    d.build_output_stream(config, callback, stream_err("output"), None)
        .map_err(|e| format!("failed to open output stream: {e}"))
}
