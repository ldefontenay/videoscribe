//! The described video (milestone 9): the film's own pictures with the
//! described sound, as an MP4 to share. Media Foundation's sink writer does
//! it, in place of FFmpeg (PLAN.md §4.3, §5.8): the pictures are copied as
//! they are, never decoded or encoded again, so nothing is lost and it is
//! quick; the sound is encoded as AAC, which every player takes.
//!
//! Only when the film is never paused for a description (PLAN.md §5.10):
//! then the described sound runs exactly as long as the film, on the same
//! clock, and the pictures need no change.
//!
//! The film is opened read-only, and the file is written under a temporary
//! name and renamed when whole (CLAUDE.md constraint 5).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::HSTRING;
use windows::Win32::Media::MediaFoundation::*;

const ALL: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const EOS: u32 = MF_SOURCE_READERF_ENDOFSTREAM.0 as u32;
/// Media Foundation counts time in 100-nanosecond units.
const HNS: f64 = 10_000_000.0;
/// The described sound's own rate and channels.
const RATE: u32 = 48_000;
const CHANNELS: u32 = 2;
/// AAC at 192 kbps: as good as the 192 kbps MP3 it is made from.
const BYTES_PER_SEC: u32 = 192_000 / 8;
/// How far the sound is written ahead of the pictures, so the two are
/// interleaved in the file rather than one after the other.
const AHEAD: f64 = 0.5;

/// Writes `out`: the pictures of `film` with the sound of `sound`. `shift`
/// is the seconds to add to a time on the film's clock to get Media
/// Foundation's (`frames::Frames`). False if `stop` was set; the temporary
/// file is then gone.
pub fn make(
    film: &Path,
    shift: f64,
    sound: &Path,
    out: &Path,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<bool, String> {
    let part = out.with_extension("mp4.part");
    let shown = out
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let fail = |e: windows::core::Error| {
        format!(
            "Windows could not make the described video, {shown}: {}",
            e.message().trim()
        )
    };
    let result = unsafe { write(film, shift, sound, &part, stop, progress) }.map_err(fail);
    match result {
        Ok(true) => std::fs::rename(&part, out).map(|_| true).map_err(|e| {
            let _ = std::fs::remove_file(&part);
            format!("{shown} could not be saved: {e}")
        }),
        other => {
            let _ = std::fs::remove_file(&part);
            other
        }
    }
}

unsafe fn reader(path: &Path) -> windows::core::Result<IMFSourceReader> {
    let r = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None)?;
    r.SetStreamSelection(ALL, false)?;
    Ok(r)
}

/// One sample from `stream`: its time in seconds, or None at the end.
unsafe fn next(
    r: &IMFSourceReader,
    stream: u32,
) -> windows::core::Result<Option<(f64, IMFSample)>> {
    loop {
        let mut flags = 0u32;
        let mut stamp = 0i64;
        let mut sample = None;
        r.ReadSample(
            stream,
            0,
            None,
            Some(&mut flags),
            Some(&mut stamp),
            Some(&mut sample),
        )?;
        if let Some(s) = sample {
            return Ok(Some((stamp as f64 / HNS, s)));
        }
        if flags & EOS != 0 {
            return Ok(None);
        }
        // A gap in the stream: read on.
    }
}

unsafe fn write(
    film: &Path,
    shift: f64,
    sound: &Path,
    part: &Path,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> windows::core::Result<bool> {
    // The pictures, as they are in the file.
    let pictures = reader(film)?;
    pictures.SetStreamSelection(VIDEO, true)?;
    let native = pictures.GetNativeMediaType(VIDEO, 0)?;
    pictures.SetCurrentMediaType(VIDEO, None, &native)?;
    let secs = media_secs(&pictures);

    // The described sound, decoded, for the AAC encoder.
    let sounds = reader(sound)?;
    sounds.SetStreamSelection(AUDIO, true)?;
    let pcm = MFCreateMediaType()?;
    pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
    pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
    pcm.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
    pcm.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS)?;
    pcm.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
    pcm.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 2 * CHANNELS)?;
    pcm.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 2 * CHANNELS * RATE)?;
    sounds.SetCurrentMediaType(AUDIO, None, &pcm)?;

    // An MP4: the name ends in .part, so the container is said outright.
    let mut attr = None;
    MFCreateAttributes(&mut attr, 1)?;
    let attr = attr.ok_or_else(windows::core::Error::empty)?;
    attr.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
    let writer = MFCreateSinkWriterFromURL(&HSTRING::from(part.as_os_str()), None, &attr)?;
    let v = writer.AddStream(&native)?;
    writer.SetInputMediaType(v, &native, None)?;
    let aac = MFCreateMediaType()?;
    aac.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
    aac.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
    aac.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
    aac.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS)?;
    aac.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
    aac.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, BYTES_PER_SEC)?;
    let a = writer.AddStream(&aac)?;
    writer.SetInputMediaType(a, &pcm, None)?;
    writer.BeginWriting()?;

    // On the film's clock a picture is at its Media Foundation time less
    // `shift`. Should that put the first picture before nought, everything
    // moves later by `lead`, the sound with it, so the two stay together.
    let mut first = next(&pictures, VIDEO)?;
    let lead = first.as_ref().map_or(0.0, |(t, _)| (shift - t).max(0.0));
    let mut sound_next = next(&sounds, AUDIO)?;
    while let Some((t, sample)) = first.take() {
        if stop.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let at = t - shift + lead;
        while let Some((st, s)) = sound_next.take() {
            if st + lead > at + AHEAD {
                sound_next = Some((st, s));
                break;
            }
            s.SetSampleTime(((st + lead) * HNS) as i64)?;
            writer.WriteSample(a, &s)?;
            sound_next = next(&sounds, AUDIO)?;
        }
        sample.SetSampleTime((at * HNS) as i64)?;
        writer.WriteSample(v, &sample)?;
        if secs > 0.0 {
            progress((t / secs).clamp(0.0, 1.0));
        }
        first = next(&pictures, VIDEO)?;
    }
    while let Some((st, s)) = sound_next.take() {
        if stop.load(Ordering::Relaxed) {
            return Ok(false);
        }
        s.SetSampleTime(((st + lead) * HNS) as i64)?;
        writer.WriteSample(a, &s)?;
        sound_next = next(&sounds, AUDIO)?;
    }
    writer.Finalize()?;
    Ok(true)
}

/// How long the source is, in seconds, or nought if it does not say.
unsafe fn media_secs(r: &IMFSourceReader) -> f64 {
    r.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
        .ok()
        .and_then(|p| u64::try_from(&p).ok())
        .map_or(0.0, |d| d as f64 / HNS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs VS_FILM, VS_SOUND and VS_OUT: `cargo test -- --ignored mux`.
    #[test]
    #[ignore]
    fn mux_makes_a_described_video() {
        let var = |n: &str| std::path::PathBuf::from(std::env::var_os(n).expect(n));
        let _s = crate::media::Session::start().unwrap();
        let stop = AtomicBool::new(false);
        let out = var("VS_OUT");
        let made = make(
            &var("VS_FILM"),
            0.0,
            &var("VS_SOUND"),
            &out,
            &stop,
            &mut |_| {},
        )
        .unwrap();
        assert!(made);
        let film = crate::media::duration(&var("VS_FILM").to_string_lossy()).unwrap();
        let got = crate::media::duration(&out.to_string_lossy()).unwrap();
        assert!(
            (film - got).abs() < 0.5,
            "film {film} s, described video {got} s"
        );
    }
}
