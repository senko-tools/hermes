//! The DSP worker: everything between the input and output ring buffers.
//!
//! input ring (device rate, interleaved) -> mono -> resample to 48 kHz -> nnnoiseless (480)
//! -> Opus encode (960 = 20 ms) -> Opus decode -> resample to output rate -> output ring (mono)

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use nnnoiseless::DenoiseState;
use opus::{Application, Bitrate, Channels, Decoder, Encoder};
use ringbuf::traits::{Consumer, Observer, Producer};
use ringbuf::{HeapCons, HeapProd};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

pub const OPUS_RATE: u32 = 48_000;
const OPUS_FRAME: usize = 960;
const DENOISE_FRAME: usize = DenoiseState::FRAME_SIZE;
const OPUS_BITRATE: i32 = 64_000;
const MAX_PACKET: usize = 4000;
/// nnnoiseless output lags its input by one frame (overlap-add), on top of frame buffering.
const DENOISE_DELAY_S: f64 = DENOISE_FRAME as f64 / OPUS_RATE as f64;

/// Counters written by the cpal callbacks (atomics only, no locks).
#[derive(Default)]
pub struct CallbackStats {
    pub in_callbacks: AtomicU64,
    pub in_frames: AtomicU64,
    pub in_device_latency_us: AtomicU64,
    pub in_overflows: AtomicU64,
    pub out_callbacks: AtomicU64,
    pub out_frames: AtomicU64,
    pub out_device_latency_us: AtomicU64,
    pub out_underruns: AtomicU64,
}

pub struct DeviceInfo {
    pub name: String,
    pub rate: u32,
    pub channels: usize,
    pub format: String,
    pub buffer_range: String,
}

pub struct Config {
    pub input: DeviceInfo,
    pub output: DeviceInfo,
    pub denoise: bool,
    /// Extra input gain in dB, applied right after the downmix.
    pub gain_db: f32,
}

/// Peak and RMS of a signal over one window.
#[derive(Default)]
struct Level {
    peak: f32,
    sum_sq: f64,
    samples: u64,
    clipped: u64,
}

impl Level {
    fn add(&mut self, s: f32) {
        let a = s.abs();
        self.peak = self.peak.max(a);
        self.sum_sq += f64::from(s * s);
        self.samples += 1;
        if a > 1.0 {
            self.clipped += 1;
        }
    }

    /// `rms/peak` in dBFS.
    fn describe(&self) -> String {
        let db = |v: f64| {
            if v > 0.0 {
                format!("{:.1}", 20.0 * v.log10())
            } else {
                "-inf".to_string()
            }
        };
        let rms = if self.samples > 0 {
            (self.sum_sq / self.samples as f64).sqrt()
        } else {
            0.0
        };
        format!("{}/{} dBFS", db(rms), db(f64::from(self.peak)))
    }
}

/// Resampler for one mono stream with a fixed input chunk, or a passthrough when rates match.
struct MonoResampler {
    inner: Option<Async<f32>>,
    out_buf: Vec<f32>,
}

impl MonoResampler {
    fn new(from: u32, to: u32) -> Self {
        if from == to {
            return Self {
                inner: None,
                out_buf: Vec::new(),
            };
        }
        let mut params = SincInterpolationParameters::new(64, WindowFunction::BlackmanHarris2);
        params.interpolation = SincInterpolationType::Linear;
        // 10 ms input chunks keep the extra buffering small.
        let chunk = (from / 100) as usize;
        let inner = Async::<f32>::new_sinc(
            f64::from(to) / f64::from(from),
            1.1,
            &params,
            chunk,
            1,
            FixedAsync::Input,
        )
        .expect("failed to create resampler");
        let out_buf = vec![0.0; inner.output_frames_max()];
        Self {
            inner: Some(inner),
            out_buf,
        }
    }

    /// Algorithmic delay in seconds at the output rate.
    fn delay_s(&self, to: u32) -> f64 {
        self.inner
            .as_ref()
            .map_or(0.0, |r| r.output_delay() as f64 / f64::from(to))
    }

    /// Consume as many whole chunks from `input` as possible and append the result to `output`.
    fn process(&mut self, input: &mut Vec<f32>, output: &mut Vec<f32>) {
        let Some(r) = self.inner.as_mut() else {
            output.append(input);
            return;
        };
        let mut used = 0;
        loop {
            let need = r.input_frames_next();
            if input.len() - used < need {
                break;
            }
            let src =
                InterleavedSlice::new(&input[used..used + need], 1, need).expect("input adapter");
            let frames = self.out_buf.len();
            let mut dst =
                InterleavedSlice::new_mut(&mut self.out_buf, 1, frames).expect("output adapter");
            let (n_in, n_out) = r
                .process_into_buffer(&src, &mut dst, None)
                .expect("resampling failed");
            used += n_in;
            output.extend_from_slice(&self.out_buf[..n_out]);
        }
        input.drain(..used);
    }

    fn pending(&self, input: &[f32]) -> usize {
        input.len()
    }
}

/// Per-second accumulators kept by the worker.
#[derive(Default)]
struct Window {
    loops: u64,
    in_ring_s: f64,
    pending_s: f64,
    out_ring_s: f64,
    out_ring_min: usize,
    packets: u64,
    dtx_packets: u64,
    bytes: u64,
    vad_sum: f64,
    vad_frames: u64,
    dropped: u64,
    mic: Level,
    out: Level,
}

/// All DSP state between the two ring buffers.
pub struct Worker {
    in_rate: u32,
    in_ch: usize,
    out_rate: u32,
    denoise: bool,
    gain: f32,
    enc: Encoder,
    dec: Decoder,
    denoiser: Box<DenoiseState<'static>>,
    up: MonoResampler,
    down: MonoResampler,
    lookahead_s: f64,
    raw: Vec<f32>,
    mono_in: Vec<f32>,
    at48: Vec<f32>,
    frame: Vec<f32>,
    denoise_in: [f32; DENOISE_FRAME],
    denoise_out: [f32; DENOISE_FRAME],
    packet: [u8; MAX_PACKET],
    decoded: Vec<f32>,
    dec48: Vec<f32>,
    at_out: Vec<f32>,
    drop_budget: usize,
    first_denoise_frame: bool,
    win: Window,
}

impl Worker {
    pub fn new(in_rate: u32, in_ch: usize, out_rate: u32, denoise: bool, gain_db: f32) -> Self {
        let mut enc =
            Encoder::new(OPUS_RATE, Channels::Mono, Application::Voip).expect("opus encoder");
        enc.set_bitrate(Bitrate::Bits(OPUS_BITRATE))
            .expect("set bitrate");
        enc.set_inband_fec(true).expect("set fec");
        enc.set_packet_loss_perc(10).expect("set packet loss");
        enc.set_dtx(true).expect("set dtx");
        let lookahead_s =
            f64::from(enc.get_lookahead().expect("get lookahead")) / f64::from(OPUS_RATE);
        Self {
            in_rate,
            in_ch,
            out_rate,
            denoise,
            gain: 10f32.powf(gain_db / 20.0),
            enc,
            dec: Decoder::new(OPUS_RATE, Channels::Mono).expect("opus decoder"),
            denoiser: DenoiseState::new(),
            up: MonoResampler::new(in_rate, OPUS_RATE),
            down: MonoResampler::new(OPUS_RATE, out_rate),
            lookahead_s,
            raw: vec![0.0; in_rate as usize * in_ch],
            mono_in: Vec::with_capacity(in_rate as usize),
            at48: Vec::with_capacity(OPUS_RATE as usize),
            frame: Vec::with_capacity(OPUS_FRAME * 2),
            denoise_in: [0.0; DENOISE_FRAME],
            denoise_out: [0.0; DENOISE_FRAME],
            packet: [0; MAX_PACKET],
            decoded: vec![0.0; OPUS_FRAME],
            dec48: Vec::with_capacity(OPUS_RATE as usize),
            at_out: Vec::with_capacity(out_rate as usize),
            drop_budget: 0,
            first_denoise_frame: true,
            win: Window::new(),
        }
    }

    /// Fixed algorithmic delay of the chain (resampler filters, Opus lookahead, denoiser).
    fn algo_s(&self) -> f64 {
        self.up.delay_s(OPUS_RATE)
            + self.down.delay_s(self.out_rate)
            + self.lookahead_s
            + if self.denoise { DENOISE_DELAY_S } else { 0.0 }
    }

    /// Move everything currently available in `input` as far through the chain as it goes.
    pub fn step(&mut self, input: &mut HeapCons<f32>, output: &mut HeapProd<f32>) {
        let in_ch = self.in_ch;
        let win = &mut self.win;

        // 1. Pull everything the input callback produced and downmix to mono.
        let n = input.pop_slice(&mut self.raw);
        let n = n - n % in_ch;
        for f in self.raw[..n].chunks_exact(in_ch) {
            let s = f.iter().sum::<f32>() / in_ch as f32 * self.gain;
            win.mic.add(s);
            self.mono_in.push(s);
        }

        // 2. Resample to 48 kHz.
        self.up.process(&mut self.mono_in, &mut self.at48);

        // 3. Denoise in 480-sample frames, collecting into 20 ms Opus frames.
        let mut used = 0;
        while self.at48.len() - used >= DENOISE_FRAME {
            let chunk = &self.at48[used..used + DENOISE_FRAME];
            used += DENOISE_FRAME;
            if self.denoise {
                for (d, s) in self.denoise_in.iter_mut().zip(chunk) {
                    *d = s * 32768.0;
                }
                let vad = self
                    .denoiser
                    .process_frame(&mut self.denoise_out, &self.denoise_in);
                if self.first_denoise_frame {
                    // The first output frame contains fade-in artifacts.
                    self.first_denoise_frame = false;
                    self.denoise_out.fill(0.0);
                }
                win.vad_sum += f64::from(vad);
                win.vad_frames += 1;
                self.frame
                    .extend(self.denoise_out.iter().map(|s| s / 32768.0));
            } else {
                self.frame.extend_from_slice(chunk);
            }
        }
        self.at48.drain(..used);

        // 4. Opus encode + decode every full 20 ms frame.
        while self.frame.len() >= OPUS_FRAME {
            let len = self
                .enc
                .encode_float(&self.frame[..OPUS_FRAME], &mut self.packet)
                .expect("opus encode");
            self.frame.drain(..OPUS_FRAME);
            win.packets += 1;
            win.bytes += len as u64;
            // With DTX on, a 1–2 byte packet means "silence"; a real client would not send it.
            if len <= 2 {
                win.dtx_packets += 1;
            }
            let samples = self
                .dec
                .decode_float(&self.packet[..len], &mut self.decoded, false)
                .expect("opus decode");
            for &s in &self.decoded[..samples] {
                win.out.add(s);
            }
            self.dec48.extend_from_slice(&self.decoded[..samples]);
        }

        // 5. Resample to the output rate and hand to the output callback.
        self.down.process(&mut self.dec48, &mut self.at_out);
        if !self.at_out.is_empty() {
            let skip = self.drop_budget.min(self.at_out.len());
            self.drop_budget -= skip;
            let pushed = output.push_slice(&self.at_out[skip..]);
            win.dropped += (self.at_out.len() - pushed) as u64;
            self.at_out.clear();
        }

        // Bookkeeping for the latency estimate (Little's law: average buffered / rate).
        let out_occ = output.occupied_len();
        win.loops += 1;
        win.in_ring_s += input.occupied_len() as f64 / (f64::from(self.in_rate) * in_ch as f64);
        win.pending_s += self.up.pending(&self.mono_in) as f64 / f64::from(self.in_rate)
            + (self.at48.len() + self.frame.len()) as f64 / f64::from(OPUS_RATE)
            + self.down.pending(&self.dec48) as f64 / f64::from(OPUS_RATE);
        win.out_ring_s += out_occ as f64 / f64::from(self.out_rate);
        win.out_ring_min = win.out_ring_min.min(out_occ);
    }

    /// Close the current one-second window: correct for clock drift and return its stats.
    fn end_window(&mut self) -> Window {
        // If the output queue never drained below 15 ms during the window, the input clock
        // is faster than the output clock; drop the surplus down to 5 ms.
        let rate = self.out_rate as usize;
        let min = self.win.out_ring_min;
        if min != usize::MAX && min > rate * 15 / 1000 {
            self.drop_budget += min - rate * 5 / 1000;
        }
        std::mem::replace(&mut self.win, Window::new())
    }
}

impl Window {
    fn new() -> Self {
        Self {
            out_ring_min: usize::MAX,
            ..Self::default()
        }
    }
}

pub fn run(
    cfg: Config,
    mut input: HeapCons<f32>,
    mut output: HeapProd<f32>,
    stats: Arc<CallbackStats>,
) -> ! {
    let mut worker = Worker::new(
        cfg.input.rate,
        cfg.input.channels,
        cfg.output.rate,
        cfg.denoise,
        cfg.gain_db,
    );
    print_header(&cfg, &worker);
    let mut last_print = Instant::now();
    let mut prev = Snapshot::take(&stats);
    loop {
        worker.step(&mut input, &mut output);
        if last_print.elapsed() >= Duration::from_secs(1) {
            let now = Snapshot::take(&stats);
            let algo_s = worker.algo_s();
            let win = worker.end_window();
            print_stats(&cfg, &win, &now.since(&prev), algo_s);
            prev = now;
            last_print = Instant::now();
        }
        thread::sleep(Duration::from_millis(1));
    }
}

#[derive(Clone, Copy)]
struct Snapshot {
    in_callbacks: u64,
    in_frames: u64,
    in_lat_us: u64,
    in_overflows: u64,
    out_callbacks: u64,
    out_frames: u64,
    out_lat_us: u64,
    out_underruns: u64,
}

impl Snapshot {
    fn take(s: &CallbackStats) -> Self {
        Self {
            in_callbacks: s.in_callbacks.load(Ordering::Relaxed),
            in_frames: s.in_frames.load(Ordering::Relaxed),
            in_lat_us: s.in_device_latency_us.load(Ordering::Relaxed),
            in_overflows: s.in_overflows.load(Ordering::Relaxed),
            out_callbacks: s.out_callbacks.load(Ordering::Relaxed),
            out_frames: s.out_frames.load(Ordering::Relaxed),
            out_lat_us: s.out_device_latency_us.load(Ordering::Relaxed),
            out_underruns: s.out_underruns.load(Ordering::Relaxed),
        }
    }

    fn since(&self, old: &Self) -> Self {
        Self {
            in_callbacks: self.in_callbacks - old.in_callbacks,
            in_frames: self.in_frames - old.in_frames,
            in_lat_us: self.in_lat_us - old.in_lat_us,
            in_overflows: self.in_overflows - old.in_overflows,
            out_callbacks: self.out_callbacks - old.out_callbacks,
            out_frames: self.out_frames - old.out_frames,
            out_lat_us: self.out_lat_us - old.out_lat_us,
            out_underruns: self.out_underruns - old.out_underruns,
        }
    }
}

fn print_header(cfg: &Config, w: &Worker) {
    for (dir, d) in [("input ", &cfg.input), ("output", &cfg.output)] {
        println!(
            "{dir}: \"{}\" {} Hz, {} ch, {}, buffer size {}",
            d.name, d.rate, d.channels, d.format, d.buffer_range
        );
    }
    println!(
        "opus: 48 kHz mono, 20 ms, {} kbps, VOIP, FEC on (10% loss), DTX on, lookahead {:.1} ms",
        OPUS_BITRATE / 1000,
        w.lookahead_s * 1000.0
    );
    println!(
        "resample in: {}, out: {}, denoise: {}, gain: {:+.1} dB",
        describe_resampler(&w.up, cfg.input.rate, OPUS_RATE),
        describe_resampler(&w.down, OPUS_RATE, cfg.output.rate),
        if cfg.denoise { "on" } else { "off" },
        cfg.gain_db
    );
    println!("Press Ctrl+C to stop.\n");
}

fn describe_resampler(r: &MonoResampler, from: u32, to: u32) -> String {
    match r.inner {
        None => "none".to_string(),
        Some(_) => format!("{from} -> {to} Hz (delay {:.1} ms)", r.delay_s(to) * 1000.0),
    }
}

fn ms(s: f64) -> f64 {
    s * 1000.0
}

fn per_cb(total_us: u64, callbacks: u64) -> f64 {
    if callbacks == 0 {
        0.0
    } else {
        total_us as f64 / callbacks as f64 / 1_000_000.0
    }
}

/// Average bitrate over the encoded audio (each packet is 20 ms).
fn kbps(bytes: u64, packets: u64) -> f64 {
    if packets == 0 {
        0.0
    } else {
        bytes as f64 * 8.0 / (packets as f64 * 0.020) / 1000.0
    }
}

fn print_stats(cfg: &Config, w: &Window, cb: &Snapshot, algo_s: f64) {
    let loops = w.loops.max(1) as f64;
    let in_dev = per_cb(cb.in_lat_us, cb.in_callbacks);
    let out_dev = per_cb(cb.out_lat_us, cb.out_callbacks);
    let in_ring = w.in_ring_s / loops;
    let pending = w.pending_s / loops;
    let out_ring = w.out_ring_s / loops;
    let total = in_dev + in_ring + pending + algo_s + out_ring + out_dev;
    let in_cb_frames = cb.in_frames.checked_div(cb.in_callbacks).unwrap_or(0);
    let out_cb_frames = cb.out_frames.checked_div(cb.out_callbacks).unwrap_or(0);
    let vad = if w.vad_frames > 0 {
        format!(" vad {:.2}", w.vad_sum / w.vad_frames as f64)
    } else {
        String::new()
    };
    println!(
        "latency ~{:.1} ms = in-dev {:.1} + in-ring {:.1} + buffering {:.1} + algo {:.1} + out-ring {:.1} + out-dev {:.1} | \
         cb in {}x{}f @{} Hz, out {}x{}f @{} Hz | opus {} pkt ({} dtx) {:.1} kbps{} | mic {} (clipped {}) out {} | underruns {} overflows {} dropped {}",
        ms(total),
        ms(in_dev),
        ms(in_ring),
        ms(pending),
        ms(algo_s),
        ms(out_ring),
        ms(out_dev),
        cb.in_callbacks,
        in_cb_frames,
        cfg.input.rate,
        cb.out_callbacks,
        out_cb_frames,
        cfg.output.rate,
        w.packets,
        w.dtx_packets,
        kbps(w.bytes, w.packets),
        vad,
        w.mic.describe(),
        w.mic.clipped,
        w.out.describe(),
        cb.out_underruns,
        cb.in_overflows,
        w.dropped,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::HeapRb;
    use ringbuf::traits::Split;

    /// Push one second of a stereo 440 Hz tone through the whole chain in 10 ms chunks and
    /// return the mono output.
    fn loopback(in_rate: u32, out_rate: u32, denoise: bool) -> Vec<f32> {
        let (mut in_prod, mut in_cons) = HeapRb::<f32>::new(in_rate as usize).split();
        let (mut out_prod, mut out_cons) = HeapRb::<f32>::new(out_rate as usize * 2).split();
        let mut worker = Worker::new(in_rate, 2, out_rate, denoise, 0.0);
        let chunk = in_rate as usize / 100;
        let mut out = Vec::new();
        for c in 0..100 {
            for i in 0..chunk {
                let t = (c * chunk + i) as f32 / in_rate as f32;
                let s = 0.3 * (2.0 * std::f32::consts::PI * 440.0 * t).sin();
                in_prod.push_slice(&[s, s]);
            }
            worker.step(&mut in_cons, &mut out_prod);
            while let Some(s) = out_cons.try_pop() {
                out.push(s);
            }
        }
        out
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    }

    #[test]
    fn tone_survives_resampling_and_opus() {
        for (in_rate, out_rate) in [(44_100, 48_000), (48_000, 48_000), (48_000, 96_000)] {
            let out = loopback(in_rate, out_rate, false);
            let expected = out_rate as usize;
            // At most one Opus frame plus resampler chunks may still be in flight.
            assert!(
                out.len() > expected * 9 / 10 && out.len() <= expected,
                "{in_rate}->{out_rate}: got {} samples",
                out.len()
            );
            // Skip the start-up transient; 0.3 amplitude sine has RMS ~0.21.
            let level = rms(&out[out.len() / 2..]);
            assert!(
                (0.15..0.28).contains(&level),
                "{in_rate}->{out_rate}: rms {level}"
            );
        }
    }

    #[test]
    fn gain_scales_the_signal() {
        let mut level = Level::default();
        level.add(0.5);
        level.add(-0.5);
        assert_eq!(level.describe(), "-6.0/-6.0 dBFS");
        let gain = Worker::new(48_000, 1, 48_000, false, 6.0).gain;
        assert!((gain - 1.995).abs() < 0.01);
    }

    #[test]
    fn denoise_path_runs() {
        let out = loopback(48_000, 48_000, true);
        assert!(out.len() > 43_000);
        assert!(out.iter().all(|s| s.is_finite()));
    }
}
