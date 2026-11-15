//! 16 kHz mono 16-bit WAV, the format recognition reads and training tools
//! expect.

use std::io::{Read, Write};
use std::path::Path;

use anyhow::{anyhow, Result};

pub const RATE: u32 = 16_000;

pub fn write(path: &Path, samples: &[f32]) -> Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    let tmp = path.with_extension("wav.tmp");
    std::fs::File::create(&tmp)?.write_all(&out)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// Reads a file written by [`write`] (16 kHz mono 16-bit PCM)
pub fn read(path: &Path) -> Result<Vec<f32>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut bytes)?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(anyhow!("not a WAV file: {}", path.display()));
    }
    let mut at = 12;
    let mut format_ok = false;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = at + 8;
        if id == b"fmt " && body + 16 <= bytes.len() {
            let channels = u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]);
            let rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().unwrap());
            let bits = u16::from_le_bytes([bytes[body + 14], bytes[body + 15]]);
            format_ok = channels == 1 && rate == RATE && bits == 16;
        }
        if id == b"data" {
            if !format_ok {
                return Err(anyhow!("expected 16 kHz mono 16-bit: {}", path.display()));
            }
            let end = (body + len).min(bytes.len());
            return Ok(bytes[body..end].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect());
        }
        at = body + len + (len & 1);
    }
    Err(anyhow!("no audio in {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("protokol-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.wav");
        let samples: Vec<f32> = (0..1600).map(|i| ((i as f32) / 50.0).sin() * 0.5).collect();
        write(&path, &samples).unwrap();
        let back = read(&path).unwrap();
        assert_eq!(back.len(), samples.len());
        assert!(back.iter().zip(&samples).all(|(a, b)| (a - b).abs() < 1e-3));
        std::fs::remove_dir_all(dir).ok();
    }
}
