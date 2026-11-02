//! Media Foundation, which is part of Windows, in place of FFmpeg (PLAN.md
//! §4.3). Milestone 3 needs only the sound, as 16 kHz mono for the VAD.
//!
//! Every function opens the source read-only; nothing here can write to the
//! user's video (CLAUDE.md constraint 5).

use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::HSTRING;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

const ALL: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const EOS: u32 = MF_SOURCE_READERF_ENDOFSTREAM.0 as u32;
/// Media Foundation counts time in 100-nanosecond units.
const HNS: f64 = 10_000_000.0;

/// COM and Media Foundation for the life of a worker thread.
pub struct Session;

impl Session {
    pub fn start() -> Result<Session, String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_FULL)
                .map_err(|e| format!("Windows' media support would not start: {}", e.message()))?;
        }
        Ok(Session)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}

/// Mono samples at a known rate.
pub struct Sound {
    pub rate: u32,
    pub samples: Vec<i16>,
}

impl Sound {
    pub fn secs(&self) -> f64 {
        self.samples.len() as f64 / self.rate as f64
    }
}

/// The file's name without its folders, which are long to listen to.
fn name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

/// Windows' error in words.
fn plain(e: windows::core::Error, path: &str) -> String {
    let path = name(path);
    // MF_E_UNSUPPORTED_BYTESTREAM_TYPE and friends: Windows cannot read it.
    let code = e.code().0 as u32;
    if matches!(code, 0xC00D36C4 | 0xC00D36C3 | 0xC00D36B4 | 0xC00D36E6) {
        format!(
            "Windows cannot read {path}: it is not a video type that Windows can play, or it is \
             damaged."
        )
    } else if code == 0x80070002 || code == 0x80070003 {
        format!("{path} could not be found.")
    } else {
        format!("Windows could not read {path}: {}", e.message().trim())
    }
}

/// The running time in seconds, as the file declares it.
pub fn duration(path: &str) -> Result<f64, String> {
    unsafe {
        let r =
            MFCreateSourceReaderFromURL(&HSTRING::from(path), None).map_err(|e| plain(e, path))?;
        let d = r
            .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
            .map_err(|e| plain(e, path))?;
        Ok(u64::try_from(&d).unwrap_or(0) as f64 / HNS)
    }
}

/// The first audio stream of `path` as 16-bit mono at `rate`, converted by
/// Media Foundation. `progress` hears the share done, 0 to 1. Returns None if
/// `stop` is set on the way.
pub fn decode_mono(
    path: &str,
    rate: u32,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<Option<Sound>, String> {
    let secs = duration(path)?;
    let mut samples: Vec<i16> = Vec::with_capacity((secs * rate as f64) as usize + rate as usize);
    let done = each_mono(path, rate, stop, progress, &mut |s| {
        samples.extend_from_slice(s);
        Ok(())
    })?;
    Ok(done.then_some(Sound { rate, samples }))
}

/// The same, handed over a piece at a time, so a long film never has to be
/// held whole. False if `stop` was set on the way.
pub fn each_mono(
    path: &str,
    rate: u32,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(f64),
    take: &mut dyn FnMut(&[i16]) -> Result<(), String>,
) -> Result<bool, String> {
    each(path, rate, 1, stop, progress, take)
}

/// As `each_mono`, with `channels` interleaved: 1 or 2. Media Foundation mixes
/// or copies channels as needed.
pub fn each(
    path: &str,
    rate: u32,
    channels: u32,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(f64),
    take: &mut dyn FnMut(&[i16]) -> Result<(), String>,
) -> Result<bool, String> {
    read(path, rate, channels, 0.0, stop, progress, &mut |_, s| {
        take(s)
    })
}

/// `from` to `to` seconds of `path`'s sound, `channels` interleaved, for
/// playing a stretch of it (milestone 8).
pub fn stretch(
    path: &str,
    from: f64,
    to: f64,
    rate: u32,
    channels: u32,
) -> Result<Vec<i16>, String> {
    let enough = AtomicBool::new(false);
    let mut out: Vec<i16> = Vec::new();
    let per = channels as usize;
    let first = (from * rate as f64) as i64;
    let last = (to * rate as f64) as i64;
    read(
        path,
        rate,
        channels,
        from,
        &enough,
        &mut |_| {},
        &mut |at, s| {
            // Seeking lands near `from`, not on it: keep only what lies inside.
            let start = (at * rate as f64).round() as i64;
            for (k, frame) in s.chunks_exact(per).enumerate() {
                let f = start + k as i64;
                if f >= last {
                    enough.store(true, Ordering::Relaxed);
                    break;
                }
                if f >= first {
                    out.extend_from_slice(frame);
                }
            }
            Ok(())
        },
    )?;
    Ok(out)
}

/// `path`'s sound from `from` seconds to the end, each piece handed over with
/// the time it starts at, for the player. Seeking lands near `from`, not on
/// it. False if `stop` was set on the way.
pub fn from(
    path: &str,
    from: f64,
    rate: u32,
    channels: u32,
    stop: &AtomicBool,
    take: &mut Take,
) -> Result<bool, String> {
    read(path, rate, channels, from, stop, &mut |_| {}, take)
}

/// Takes each decoded piece, with the time it starts at.
pub type Take<'a> = dyn FnMut(f64, &[i16]) -> Result<(), String> + 'a;

/// The decoding loop under `each` and `stretch`: from `from` seconds, each
/// piece handed over with the time it starts at.
fn read(
    path: &str,
    rate: u32,
    channels: u32,
    from: f64,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(f64),
    take: &mut Take,
) -> Result<bool, String> {
    let secs = duration(path)?;
    unsafe {
        let r =
            MFCreateSourceReaderFromURL(&HSTRING::from(path), None).map_err(|e| plain(e, path))?;
        r.SetStreamSelection(ALL, false)
            .map_err(|e| plain(e, path))?;
        if r.SetStreamSelection(AUDIO, true).is_err() {
            return Err(format!(
                "{} has no sound, so there is no speech to find and nothing to describe over.",
                name(path)
            ));
        }
        let t = MFCreateMediaType().map_err(|e| plain(e, path))?;
        let set = || -> windows::core::Result<()> {
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)?;
            t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)?;
            t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 2 * channels)?;
            t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 2 * channels * rate)?;
            t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
            r.SetCurrentMediaType(AUDIO, None, &t)
        };
        set().map_err(|e| plain(e, path))?;
        let got = r.GetCurrentMediaType(AUDIO).map_err(|e| plain(e, path))?;
        let got_rate = got.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(0);
        let got_channels = got.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(0);
        if got_rate != rate || got_channels != channels {
            return Err(format!(
                "Windows offered the sound of {} as {got_rate} Hz with {got_channels} channels, \
                 not the {rate} Hz with {channels} Video Scribe asked for.",
                name(path)
            ));
        }
        if from > 0.0 {
            let at = (from * HNS) as i64;
            r.SetCurrentPosition(
                &windows::core::GUID::zeroed(),
                &windows::Win32::System::Com::StructuredStorage::PROPVARIANT::from(at),
            )
            .map_err(|e| plain(e, path))?;
        }
        let mut piece: Vec<i16> = Vec::new();
        loop {
            if stop.load(Ordering::Relaxed) {
                return Ok(false);
            }
            let mut flags = 0u32;
            let mut stamp = 0i64;
            let mut sample = None;
            r.ReadSample(
                AUDIO,
                0,
                None,
                Some(&mut flags),
                Some(&mut stamp),
                Some(&mut sample),
            )
            .map_err(|e| plain(e, path))?;
            if let Some(s) = sample {
                let buf = s.ConvertToContiguousBuffer().map_err(|e| plain(e, path))?;
                let mut p = std::ptr::null_mut();
                let mut len = 0u32;
                buf.Lock(&mut p, None, Some(&mut len))
                    .map_err(|e| plain(e, path))?;
                let bytes = std::slice::from_raw_parts(p, len as usize);
                piece.clear();
                piece.extend(
                    bytes
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|b| i16::from_le_bytes(*b)),
                );
                let _ = buf.Unlock();
                take(stamp as f64 / HNS, &piece)?;
                if secs > 0.0 {
                    progress((stamp as f64 / HNS / secs).clamp(0.0, 1.0));
                }
            }
            if flags & EOS != 0 {
                break;
            }
        }
        Ok(true)
    }
}

/// An MP3 file being written by Windows' own encoder, from 16-bit samples.
/// Written under a temporary name and renamed by `finish`, so a stopped job
/// never leaves half a file under the real name.
pub struct Mp3 {
    writer: IMFSinkWriter,
    rate: u32,
    channels: u32,
    written: u64,
    part: std::path::PathBuf,
    path: std::path::PathBuf,
}

impl Mp3 {
    /// Mono at 128 kbps, for the pause check file.
    pub fn create(path: &std::path::Path, rate: u32) -> Result<Mp3, String> {
        Mp3::create_with(path, rate, 1, 128)
    }

    /// `rate` must be one the encoder takes: 32000, 44100 or 48000. Samples
    /// are written interleaved when `channels` is 2.
    pub fn create_with(
        path: &std::path::Path,
        rate: u32,
        channels: u32,
        kbps: u32,
    ) -> Result<Mp3, String> {
        let part = path.with_extension("mp3.part");
        let shown = name(&path.to_string_lossy());
        let fail = |e: windows::core::Error| {
            format!("Windows could not write {shown}: {}", e.message().trim())
        };
        unsafe {
            // The name does not end in .mp3, so the container is said outright.
            let attr = {
                let mut a = None;
                MFCreateAttributes(&mut a, 1).map_err(fail)?;
                a.ok_or_else(|| format!("Windows could not write {shown}."))?
            };
            attr.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MP3)
                .map_err(fail)?;
            let writer = MFCreateSinkWriterFromURL(&HSTRING::from(part.as_os_str()), None, &attr)
                .map_err(fail)?;
            let out = MFCreateMediaType().map_err(fail)?;
            let input = MFCreateMediaType().map_err(fail)?;
            let set = || -> windows::core::Result<u32> {
                out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_MP3)?;
                out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)?;
                out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)?;
                out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, kbps * 1000 / 8)?;
                let stream = writer.AddStream(&out)?;
                input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
                input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)?;
                input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)?;
                input.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 2 * channels)?;
                input.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 2 * channels * rate)?;
                writer.SetInputMediaType(stream, &input, None)?;
                writer.BeginWriting()?;
                Ok(stream)
            };
            if let Err(e) = set() {
                let _ = std::fs::remove_file(&part);
                return Err(fail(e));
            }
            Ok(Mp3 {
                writer,
                rate,
                channels,
                written: 0,
                part,
                path: path.to_path_buf(),
            })
        }
    }

    pub fn write(&mut self, samples: &[i16]) -> Result<(), String> {
        if samples.is_empty() {
            return Ok(());
        }
        let shown = name(&self.path.to_string_lossy());
        let fail = |e: windows::core::Error| {
            format!("Windows could not write {shown}: {}", e.message().trim())
        };
        unsafe {
            let bytes = samples.len() * 2;
            let buf = MFCreateMemoryBuffer(bytes as u32).map_err(fail)?;
            let mut p = std::ptr::null_mut();
            buf.Lock(&mut p, None, None).map_err(fail)?;
            std::ptr::copy_nonoverlapping(samples.as_ptr() as *const u8, p, bytes);
            buf.Unlock().map_err(fail)?;
            buf.SetCurrentLength(bytes as u32).map_err(fail)?;
            let s = MFCreateSample().map_err(fail)?;
            s.AddBuffer(&buf).map_err(fail)?;
            let frames = (samples.len() / self.channels as usize) as u64;
            let hns = |n: u64| (n as f64 * HNS / self.rate as f64) as i64;
            s.SetSampleTime(hns(self.written)).map_err(fail)?;
            s.SetSampleDuration(hns(frames)).map_err(fail)?;
            self.writer.WriteSample(0, &s).map_err(fail)?;
            self.written += frames;
        }
        Ok(())
    }

    pub fn finish(self) -> Result<(), String> {
        let shown = name(&self.path.to_string_lossy());
        unsafe { self.writer.Finalize() }.map_err(|e| {
            let _ = std::fs::remove_file(&self.part);
            format!("Windows could not finish {shown}: {}", e.message().trim())
        })?;
        drop(self.writer);
        std::fs::rename(&self.part, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&self.part);
            format!("{shown} could not be saved: {e}")
        })
    }

    /// Stopped: the temporary file goes.
    pub fn abandon(self) {
        drop(self.writer);
        let _ = std::fs::remove_file(&self.part);
    }
}
