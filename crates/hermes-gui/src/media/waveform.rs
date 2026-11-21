//! A sound's waveform: [`BARS`] levels from 0 to 1, measured from a quick mono decode. The
//! levels come in one by one while FFmpeg works, so a card can draw them growing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::ffmpeg;

/// How many bars a waveform has.
pub const BARS: usize = 48;

/// The levels measured so far.
#[derive(Clone, Debug, Default)]
pub struct Waveform {
    /// One per bar once measured.
    pub bars: Vec<f32>,
    pub done: bool,
}

impl Waveform {
    /// A waveform known in full (sent along with the file).
    pub fn known(levels: &[u8]) -> Waveform {
        Waveform {
            bars: resample(levels, BARS)
                .into_iter()
                .map(|l| f32::from(l) / 255.)
                .collect(),
            done: true,
        }
    }

    /// The levels as bytes, as sent along with a file.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.bars
            .iter()
            .map(|l| (l.clamp(0., 1.) * 255.).round() as u8)
            .collect()
    }
}

/// `levels` stretched or squeezed to `n` (each output takes the loudest input it covers).
fn resample(levels: &[u8], n: usize) -> Vec<u8> {
    if levels.is_empty() {
        return vec![0; n];
    }
    (0..n)
        .map(|i| {
            let from = i * levels.len() / n;
            let to = ((i + 1) * levels.len() / n).max(from + 1).min(levels.len());
            levels[from..to].iter().copied().max().unwrap_or(0)
        })
        .collect()
}

/// How loud a stretch of samples is, from 0 to 1: its RMS in decibels, from -45 dB to -3 dB.
pub fn level(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.;
    }
    let mean = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    let db = 10. * mean.max(1e-10).log10();
    ((db + 45.) / 42.).clamp(0., 1.)
}

/// Measures `url` (lasting `duration_ms`) into `out`, bar by bar, until done or `stop`.
pub fn measure(
    url: &str,
    duration_ms: u64,
    out: &Mutex<Waveform>,
    stop: &AtomicBool,
) -> Result<(), String> {
    let total = (duration_ms.max(1) * u64::from(ffmpeg::LEVEL_RATE) / 1000).max(BARS as u64);
    let per_bar = (total / BARS as u64).max(1) as usize;
    let mut child = ffmpeg::levels(url)?;
    let mut stdout = child.stdout.take().ok_or("no output")?;
    let mut bytes = vec![0u8; per_bar * 4];
    let mut samples = Vec::with_capacity(per_bar);
    let result = loop {
        if stop.load(Ordering::Relaxed) {
            break Ok(());
        }
        let read = read_full(&mut stdout, &mut bytes).map_err(|e| e.to_string());
        let read = match read {
            Ok(read) => read,
            Err(err) => break Err(err),
        };
        samples.clear();
        samples.extend(
            bytes[..read - read % 4]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b)),
        );
        let mut wave = out.lock().unwrap_or_else(|e| e.into_inner());
        if read > 0 && wave.bars.len() < BARS {
            wave.bars.push(level(&samples));
        }
        if read < bytes.len() {
            // The end: a short file (or a wrong length) fills the rest with silence.
            wave.bars.resize(BARS, 0.);
            wave.done = true;
            break Ok(());
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// Reads until `buf` is full or the stream ends; how much was read.
fn read_full(reader: &mut impl std::io::Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

/// Measures in the background; the waveform fills in as it goes.
pub fn start(url: String, duration_ms: u64, stop: Arc<AtomicBool>) -> Arc<Mutex<Waveform>> {
    let wave = Arc::new(Mutex::new(Waveform::default()));
    let out = wave.clone();
    let spawned = std::thread::Builder::new()
        .name("media-waveform".into())
        .spawn(move || {
            if let Err(err) = measure(&url, duration_ms, &out, &stop) {
                tracing::debug!("waveform: {err}");
                let mut wave = out.lock().unwrap_or_else(|e| e.into_inner());
                wave.bars.resize(BARS, 0.);
                wave.done = true;
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("waveform thread: {err}");
    }
    wave
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_flat_and_full_scale_is_full() {
        assert_eq!(level(&[0.; 100]), 0.);
        assert!((level(&[1.; 100]) - 1.).abs() < 0.01);
        let quiet = level(&[0.05; 100]);
        assert!(quiet > 0.2 && quiet < 0.6, "{quiet}");
    }

    #[test]
    fn known_levels_are_stretched_to_the_bars() {
        let wave = Waveform::known(&[0, 255]);
        assert_eq!(wave.bars.len(), BARS);
        assert_eq!(wave.bars[0], 0.);
        assert_eq!(wave.bars[BARS - 1], 1.);
        assert!(wave.done);
        assert_eq!(Waveform::known(&wave.to_bytes()).bars, wave.bars);
    }
}
