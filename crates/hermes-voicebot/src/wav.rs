//! Minimal RIFF/WAVE reading (PCM 8/16/24/32-bit and 32-bit float, any channels and rate) and
//! writing (16-bit PCM). Enough for test signals and recordings; no dependency needed.

use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct Wav {
    pub rate: u32,
    pub channels: u16,
    /// Interleaved, full scale = 1.0.
    pub samples: Vec<f32>,
}

impl Wav {
    /// Average of the channels.
    pub fn mono(&self) -> Vec<f32> {
        let channels = usize::from(self.channels.max(1));
        self.samples
            .chunks_exact(channels)
            .map(|f| f.iter().sum::<f32>() / channels as f32)
            .collect()
    }

    pub fn duration_secs(&self) -> f32 {
        self.samples.len() as f32 / f32::from(self.channels.max(1)) / self.rate.max(1) as f32
    }
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

pub fn parse(bytes: &[u8]) -> Result<Wav, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let mut format = None;
    let mut data = None;
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32_at(bytes, pos + 4) as usize;
        let body = pos + 8;
        let end = body.saturating_add(len).min(bytes.len());
        match id {
            b"fmt " if end - body >= 16 => {
                let mut tag = u16_at(bytes, body);
                // WAVE_FORMAT_EXTENSIBLE: the real format is in the sub-format GUID.
                if tag == 0xFFFE && end - body >= 26 {
                    tag = u16_at(bytes, body + 24);
                }
                format = Some((
                    tag,
                    u16_at(bytes, body + 2),
                    u32_at(bytes, body + 4),
                    u16_at(bytes, body + 14),
                ));
            }
            b"data" => data = Some(&bytes[body..end]),
            _ => {}
        }
        // Chunks are padded to an even length.
        pos = body + len + (len & 1);
    }
    let (tag, channels, rate, bits) = format.ok_or("no fmt chunk")?;
    let data = data.ok_or("no data chunk")?;
    if channels == 0 || rate == 0 {
        return Err("invalid format".into());
    }
    let samples: Vec<f32> = match (tag, bits) {
        (1, 8) => data
            .iter()
            .map(|&b| (f32::from(b) - 128.0) / 128.0)
            .collect(),
        (1, 16) => data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f32::from(i16::from_le_bytes([c[0], c[1]])) / 32768.0)
            .collect(),
        (1, 24) => data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0)
            .collect(),
        (1, 32) => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0)
            .collect(),
        (3, 32) => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
        _ => return Err(format!("unsupported WAV format {tag} with {bits} bits")),
    };
    Ok(Wav {
        rate,
        channels,
        samples,
    })
}

pub fn read(path: &Path) -> Result<Wav, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// 16-bit PCM.
pub fn encode(wav: &Wav) -> Vec<u8> {
    let data_len = wav.samples.len() * 2;
    let block = wav.channels * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(
        &u32::try_from(36 + data_len)
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&wav.channels.to_le_bytes());
    out.extend_from_slice(&wav.rate.to_le_bytes());
    out.extend_from_slice(&(wav.rate * u32::from(block)).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&u32::try_from(data_len).unwrap_or(u32::MAX).to_le_bytes());
    for s in &wav.samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

pub fn write(path: &Path, wav: &Wav) -> Result<(), String> {
    std::fs::write(path, encode(wav)).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_16_bit() {
        let wav = Wav {
            rate: 44_100,
            channels: 2,
            samples: vec![0.0, 0.5, -0.5, 1.0, -1.0, 0.25],
        };
        let back = parse(&encode(&wav)).unwrap();
        assert_eq!((back.rate, back.channels), (44_100, 2));
        for (a, b) in wav.samples.iter().zip(&back.samples) {
            assert!((a - b).abs() < 1e-3, "{a} {b}");
        }
        assert_eq!(back.mono().len(), 3);
        assert!((back.duration_secs() - 3.0 / 44_100.0).abs() < 1e-6);
    }

    #[test]
    fn reads_float_and_skips_unknown_chunks() {
        let mut bytes = b"RIFF\0\0\0\0WAVE".to_vec();
        bytes.extend_from_slice(b"LIST\x03\0\0\0abc\0"); // odd length, padded
        bytes.extend_from_slice(b"fmt \x10\0\0\0");
        bytes.extend_from_slice(&3u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(b"data\x08\0\0\0");
        bytes.extend_from_slice(&0.75f32.to_le_bytes());
        bytes.extend_from_slice(&(-0.25f32).to_le_bytes());
        let wav = parse(&bytes).unwrap();
        assert_eq!(wav.samples, vec![0.75, -0.25]);
        assert!(parse(b"nope").is_err());
    }
}
