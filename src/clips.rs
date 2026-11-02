//! Short video clips, picture and sound, for the models that watch video
//! (Gemini, PLAN.md §4.2). Each clip covers one pause and nothing else, so no
//! speech the detector found is ever sent (CLAUDE.md constraint 7). Media
//! Foundation decodes, scales and encodes: H.264 at 640 pixels and 2 frames
//! a second, with mono AAC, as in round 2 (spike 0(e), `mf::clip`).
//!
//! The picture and the sound may be two files (YouTube, which Video Scribe
//! never merges) or one (a video of your own). Both are opened read-only.

use std::path::{Path, PathBuf};

use windows::core::{GUID, HSTRING};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;

const ALL: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const EOS: u32 = MF_SOURCE_READERF_ENDOFSTREAM.0 as u32;
const HNS: f64 = 10_000_000.0;

const LONG_EDGE: u32 = 640;
const FPS: f64 = 2.0;
/// A clip never runs longer than this; a longer pause gets several.
pub const MAX_CLIP: f64 = 30.0;

pub struct Source<'a> {
    pub video: &'a Path,
    /// Seconds to add to a video time for Media Foundation (ytdlp::drop_edit_list).
    pub shift: f64,
    pub audio: &'a Path,
}

type R<T> = windows::core::Result<T>;

unsafe fn reader(path: &Path, processing: bool) -> R<IMFSourceReader> {
    let mut attr = None;
    MFCreateAttributes(&mut attr, 1)?;
    let attr = attr.ok_or_else(windows::core::Error::empty)?;
    attr.SetUINT32(
        &MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING,
        processing as u32,
    )?;
    MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attr)
}

unsafe fn pcm_type() -> R<IMFMediaType> {
    let t = MFCreateMediaType()?;
    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
    t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
    t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48_000)?;
    t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 1)?;
    t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
    t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 2)?;
    t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 96_000)?;
    t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
    Ok(t)
}

/// The samples of one stream from `t0` to `t1` (Media Foundation's clock),
/// each with its time from `t0`. `every` keeps at most one sample per that
/// many 100-ns units (the frame rate); 0 keeps them all.
unsafe fn samples(
    r: &IMFSourceReader,
    stream: u32,
    t0: i64,
    t1: i64,
    every: i64,
) -> R<Vec<(i64, IMFSample)>> {
    r.SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(t0))?;
    let mut out = Vec::new();
    let mut next = t0;
    loop {
        let mut flags = 0u32;
        let mut ts = 0i64;
        let mut sample = None;
        r.ReadSample(
            stream,
            0,
            None,
            Some(&mut flags),
            Some(&mut ts),
            Some(&mut sample),
        )?;
        if let Some(s) = sample {
            if ts >= t1 {
                break;
            }
            if ts >= t0 && ts >= next {
                out.push((ts - t0, s));
                if every > 0 {
                    next += every * ((ts - next) / every + 1);
                }
            }
        }
        if flags & EOS != 0 {
            break;
        }
    }
    Ok(out)
}

/// `start..end` seconds of the video as a small MP4, in memory. `work` is a
/// folder for the file Media Foundation writes, deleted once read.
pub fn clip(src: &Source, start: f64, end: f64, work: &Path) -> Result<Vec<u8>, String> {
    let fail = |e: windows::core::Error| {
        format!(
            "Windows could not cut a clip at {}: {}",
            crate::speech::clock(start),
            e.message().trim()
        )
    };
    std::fs::create_dir_all(work).map_err(|e| format!("{}: {e}", work.display()))?;
    let out: PathBuf = work.join(format!("clip-{}.mp4", std::process::id()));
    let made = unsafe { write(src, start, end, &out) };
    let bytes = made
        .map_err(fail)
        .and_then(|_| std::fs::read(&out).map_err(|e| e.to_string()));
    let _ = std::fs::remove_file(&out);
    bytes
}

unsafe fn write(src: &Source, start: f64, end: f64, out: &Path) -> R<()> {
    // The picture, scaled and converted for the encoder by the video processor.
    let v = reader(src.video, true)?;
    v.SetStreamSelection(ALL, false)?;
    v.SetStreamSelection(VIDEO, true)?;
    let native = v.GetNativeMediaType(VIDEO, 0)?;
    let size = native.GetUINT64(&MF_MT_FRAME_SIZE)?;
    let (w0, h0) = ((size >> 32) as u32, size as u32);
    let scale = (LONG_EDGE as f64 / w0.max(h0) as f64).min(1.0);
    // H.264 wants even sizes; multiples of 16 avoid padding surprises.
    let even = |x: f64| ((x as u32) / 16 * 16).max(16);
    let (w, h) = (even(w0 as f64 * scale), even(h0 as f64 * scale));
    let vin = MFCreateMediaType()?;
    vin.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    vin.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
    vin.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
    v.SetCurrentMediaType(VIDEO, None, &vin)?;
    let vin = v.GetCurrentMediaType(VIDEO)?;
    vin.SetUINT64(&MF_MT_FRAME_RATE, ((FPS as u64) << 32) | 1)?;

    // The sound, from its own file or the same one.
    let a = reader(src.audio, false)?;
    a.SetStreamSelection(ALL, false)?;
    let has_audio = a.SetStreamSelection(AUDIO, true).is_ok()
        && a.SetCurrentMediaType(AUDIO, None, &pcm_type()?).is_ok();

    let step = (HNS / FPS) as i64;
    let (t0, t1) = ((start * HNS) as i64, (end * HNS) as i64);
    let shift = (src.shift * HNS) as i64;
    let frames = samples(&v, VIDEO, t0 + shift, t1 + shift, step)?;
    let sound = if has_audio {
        samples(&a, AUDIO, t0, t1, 0)?
    } else {
        Vec::new()
    };

    let mut attr = None;
    MFCreateAttributes(&mut attr, 1)?;
    let attr = attr.ok_or_else(windows::core::Error::empty)?;
    attr.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
    let sw = MFCreateSinkWriterFromURL(&HSTRING::from(out.as_os_str()), None, &attr)?;
    let vout = MFCreateMediaType()?;
    vout.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    vout.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
    vout.SetUINT32(&MF_MT_AVG_BITRATE, 400_000)?;
    vout.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    vout.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
    vout.SetUINT64(&MF_MT_FRAME_RATE, ((FPS as u64) << 32) | 1)?;
    vout.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
    let vs = sw.AddStream(&vout)?;
    sw.SetInputMediaType(vs, &vin, None)?;
    let as_ = if has_audio {
        let aout = MFCreateMediaType()?;
        aout.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        aout.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        aout.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48_000)?;
        aout.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 1)?;
        aout.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        aout.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 12_000)?;
        let s = sw.AddStream(&aout)?;
        sw.SetInputMediaType(s, &pcm_type()?, None)?;
        Some(s)
    } else {
        None
    };
    sw.BeginWriting()?;
    // In time order, the two streams interleaved, as the MP4 writer prefers.
    let (mut vi, mut ai) = (0, 0);
    while vi < frames.len() || ai < sound.len() {
        let take_video = match (frames.get(vi), sound.get(ai)) {
            (Some(f), Some(s)) => f.0 <= s.0,
            (Some(_), None) => true,
            _ => false,
        };
        if take_video {
            let (ts, s) = &frames[vi];
            s.SetSampleTime(*ts)?;
            s.SetSampleDuration(step)?;
            sw.WriteSample(vs, s)?;
            vi += 1;
        } else {
            let (ts, s) = &sound[ai];
            s.SetSampleTime(*ts)?;
            if let Some(a) = as_ {
                sw.WriteSample(a, s)?;
            }
            ai += 1;
        }
    }
    if frames.is_empty() {
        return Err(windows::core::Error::empty());
    }
    sw.Finalize()
}

/// The clips for some places: one per pause, places that touch sharing one,
/// none longer than MAX_CLIP. Each is (first place, last place, start, end),
/// places numbered from `first`.
pub fn plan(gaps: &[crate::speech::Gap], first: usize) -> Vec<(usize, usize, f64, f64)> {
    let mut out: Vec<(usize, usize, f64, f64)> = Vec::new();
    for (i, g) in gaps.iter().enumerate() {
        let n = first + i;
        match out.last_mut() {
            Some(c) if g.start - c.3 < 0.01 && g.end - c.2 <= MAX_CLIP => {
                c.1 = n;
                c.3 = g.end;
            }
            _ => out.push((n, n, g.start, g.end)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::Gap;

    fn gap(start: f64, end: f64) -> Gap {
        Gap {
            start,
            end,
            words: 1,
            part: None,
        }
    }

    #[test]
    fn touching_places_share_a_clip_up_to_its_limit() {
        let gaps = [
            gap(1.0, 2.0),
            gap(10.0, 15.0),
            gap(15.0, 20.0),
            gap(20.0, 25.0),
            gap(25.0, 30.0),
            gap(30.0, 35.0),
            gap(35.0, 40.0),
            gap(40.0, 45.0),
        ];
        let p = plan(&gaps, 9);
        assert_eq!(p[0], (9, 9, 1.0, 2.0));
        assert_eq!(p[1], (10, 15, 10.0, 40.0));
        assert_eq!(p[2], (16, 16, 40.0, 45.0));
    }
}
