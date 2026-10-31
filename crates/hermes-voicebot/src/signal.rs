//! Test signals and their analysis: tone bursts are easy to find in a recording, so the time
//! from sending a burst to hearing it is the latency, and missing audio inside bursts is loss.

use hermes_client_core::audio::{FRAME, SAMPLE_RATE, dsp};

/// `count` bursts of a 440 Hz tone (`on_ms` long, amplitude 0.3), each followed by `off_ms` of
/// silence, at 48 kHz.
pub fn bursts(count: usize, on_ms: usize, off_ms: usize) -> Vec<f32> {
    let per_ms = SAMPLE_RATE as usize / 1000;
    let (on, off) = (on_ms * per_ms, off_ms * per_ms);
    let mut out = Vec::with_capacity(count * (on + off));
    for _ in 0..count {
        for i in 0..on {
            let fade = (i.min(on - i) as f32 / 240.0).min(1.0);
            out.push(0.3 * fade * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin());
        }
        out.resize(out.len() + off, 0.0);
    }
    out
}

/// A frame counts as audible above this RMS.
pub const AUDIBLE_RMS: f32 = 0.02;

/// Start sample of every run of audible 20 ms frames, and how many frames were audible.
pub fn onsets(samples: &[f32]) -> (Vec<usize>, usize) {
    let mut starts = Vec::new();
    let mut audible = 0;
    let mut was = false;
    for (i, frame) in samples.chunks(FRAME).enumerate() {
        let on = dsp::rms(frame) > AUDIBLE_RMS;
        if on {
            audible += 1;
            if !was {
                starts.push(i * FRAME);
            }
        }
        was = on;
    }
    (starts, audible)
}

/// Loss and latency of a burst transmission.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub sent_bursts: usize,
    pub heard_bursts: usize,
    /// Audible frames heard / expected, as lost percent.
    pub loss_pct: f32,
    /// Per burst, ms from sending to hearing.
    pub latencies_ms: Vec<f32>,
}

impl Report {
    pub fn median_latency_ms(&self) -> Option<f32> {
        let mut l = self.latencies_ms.clone();
        l.sort_by(f32::total_cmp);
        l.get(l.len() / 2).copied()
    }
}

/// Compares what was sent (`sent`, starting at `sent_at_ms`) with what was heard (`heard`,
/// starting at `heard_at_ms`); both 48 kHz mono, times on the same clock.
pub fn compare(sent: &[f32], sent_at_ms: f64, heard: &[f32], heard_at_ms: f64) -> Report {
    let (sent_onsets, sent_frames) = onsets(sent);
    let (heard_onsets, heard_frames) = onsets(heard);
    let ms = |sample: usize| sample as f64 * 1000.0 / f64::from(SAMPLE_RATE);
    let mut latencies_ms = Vec::new();
    let mut next = 0;
    for &s in &sent_onsets {
        let sent_time = sent_at_ms + ms(s);
        // The first heard onset after the burst was sent (and within a second).
        while next < heard_onsets.len() && heard_at_ms + ms(heard_onsets[next]) < sent_time {
            next += 1;
        }
        if let Some(&h) = heard_onsets.get(next) {
            let latency = heard_at_ms + ms(h) - sent_time;
            if latency < 1000.0 {
                latencies_ms.push(latency as f32);
                next += 1;
            }
        }
    }
    let loss_pct = if sent_frames == 0 {
        0.0
    } else {
        (1.0 - heard_frames as f32 / sent_frames as f32).max(0.0) * 100.0
    };
    Report {
        sent_bursts: sent_onsets.len(),
        heard_bursts: latencies_ms.len(),
        loss_pct,
        latencies_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_bursts_and_measures_a_delay() {
        let sent = bursts(3, 200, 300);
        assert_eq!(sent.len(), 3 * 500 * 48);
        let (starts, frames) = onsets(&sent);
        assert_eq!(starts, vec![0, 500 * 48, 1000 * 48]);
        assert!((9..=10).contains(&(frames / 3)), "{frames}");

        // Heard 80 ms later, recording started 20 ms after sending.
        let mut heard = vec![0.0; 60 * 48];
        heard.extend_from_slice(&sent);
        let report = compare(&sent, 0.0, &heard, 20.0);
        assert_eq!((report.sent_bursts, report.heard_bursts), (3, 3));
        for l in &report.latencies_ms {
            assert!((l - 80.0).abs() <= 20.0, "{l}");
        }
        assert!(report.loss_pct < 5.0);

        let report = compare(&sent, 0.0, &vec![0.0; heard.len()], 0.0);
        assert_eq!(report.heard_bursts, 0);
        assert_eq!(report.loss_pct, 100.0);
        assert_eq!(report.median_latency_ms(), None);
    }
}
