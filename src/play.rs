//! Playing a stretch of a sound file, for reviewing the result (milestone 8):
//! Media Foundation decodes the stretch, and Windows' own PlaySound plays it
//! from memory. Nothing is written to disk.

use std::path::{Path, PathBuf};

use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FLAGS, SND_MEMORY, SND_NODEFAULT};

use crate::media;

/// Rate and channels a stretch is decoded at: the described sound's own.
const RATE: u32 = 48_000;
const CHANNELS: u32 = 2;

/// A stretch being played. Playing stops when it is dropped, as PlaySound
/// reads from its memory.
pub struct Playing {
    _wav: Vec<u8>,
}

impl Drop for Playing {
    fn drop(&mut self) {
        stop();
    }
}

/// Stops whatever is playing.
pub fn stop() {
    unsafe {
        let _ = PlaySoundW(PCWSTR::null(), None, SND_FLAGS(0));
    }
}

/// Plays `from` to `to` seconds of `path`. Decoded on a thread of its own, as
/// Media Foundation must not start on the window's thread; that takes a
/// fraction of a second.
pub fn play(path: &Path, from: f64, to: f64) -> Result<Playing, String> {
    stop();
    crate::player::unmute_default();
    let path: PathBuf = path.to_path_buf();
    let from = from.max(0.0);
    let samples = std::thread::spawn(move || {
        let _session = media::Session::start()?;
        media::stretch(&path.to_string_lossy(), from, to, RATE, CHANNELS)
    })
    .join()
    .map_err(|_| "The sound could not be read.".to_string())??;
    let wav = wav(&samples, RATE, CHANNELS);
    unsafe {
        let _ = PlaySoundW(
            PCWSTR(wav.as_ptr() as *const u16),
            None,
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        );
    }
    Ok(Playing { _wav: wav })
}

/// Mono samples at `rate`, such as a description just spoken, played from
/// memory.
pub fn play_samples(samples: &[i16], rate: u32) -> Playing {
    stop();
    crate::player::unmute_default();
    let wav = wav(samples, rate, 1);
    unsafe {
        let _ = PlaySoundW(
            PCWSTR(wav.as_ptr() as *const u16),
            None,
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        );
    }
    Playing { _wav: wav }
}

/// 16-bit PCM samples as a WAV file in memory.
fn wav(samples: &[i16], rate: u32, channels: u32) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&(channels as u16).to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * channels * 2).to_le_bytes());
    w.extend_from_slice(&((channels * 2) as u16).to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wav_header_counts_its_data() {
        let w = wav(&[1, -1, 2, -2], 48_000, 2);
        assert_eq!(w.len(), 44 + 8);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()), 36 + 8);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 8);
        assert_eq!(u16::from_le_bytes(w[32..34].try_into().unwrap()), 4);
    }
}

#[cfg(test)]
mod on_a_file {
    /// Needs a sound file: `VS_TEST_SOUND=<path> cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn a_stretch_is_decoded_at_its_length() {
        let path = std::env::var("VS_TEST_SOUND").unwrap();
        let _session = crate::media::Session::start().unwrap();
        let s = crate::media::stretch(&path, 30.0, 33.0, super::RATE, super::CHANNELS).unwrap();
        let secs = s.len() as f64 / (super::RATE * super::CHANNELS) as f64;
        assert!((secs - 3.0).abs() < 0.05, "{secs}");
        assert!(s.iter().any(|&x| x.unsigned_abs() > 500), "silent");
    }
}
