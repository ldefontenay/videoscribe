//! Just enough WAV reading for the spikes: 16-bit PCM, any channel count,
//! mixed to mono. Media Foundation replaces this in spike 0(e).

pub struct Audio {
    pub rate: u32,
    pub samples: Vec<f32>,
}

pub fn read(path: &str) -> Result<Audio, String> {
    let b = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err(format!("{path}: not a WAV file"));
    }
    let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let (mut channels, mut rate, mut bits) = (0u16, 0u32, 0u16);
    let mut i = 12;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let len = u32_at(i + 4) as usize;
        let body = i + 8;
        let end = (body + len).min(b.len());
        if id == b"fmt " {
            let format = u16_at(body);
            channels = u16_at(body + 2);
            rate = u32_at(body + 4);
            bits = u16_at(body + 14);
            if format != 1 && format != 0xFFFE {
                return Err(format!("{path}: WAV format {format} is not PCM"));
            }
        } else if id == b"data" {
            if bits != 16 || channels == 0 {
                return Err(format!("{path}: need 16-bit PCM, got {bits}-bit"));
            }
            let n = channels as usize;
            let samples = b[body..end]
                .chunks_exact(2 * n)
                .map(|frame| {
                    let sum: f32 = frame
                        .chunks_exact(2)
                        .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0)
                        .sum();
                    sum / n as f32
                })
                .collect();
            return Ok(Audio { rate, samples });
        }
        i = body + len + (len & 1);
    }
    Err(format!("{path}: no data chunk"))
}
