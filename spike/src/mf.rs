//! Spike 0(e): Media Foundation instead of FFmpeg (PLAN.md §4.3, §8).
//!
//!   decode   any file Windows can play -> 16-bit PCM at a chosen rate and channel count
//!   frame    a still at a timestamp -> JPEG, long edge capped, via WIC
//!   mp3      PCM -> MP3 through the in-box MP3 encoder (Windows 8 and later)
//!   clip     a time range -> a small H.264/AAC MP4, for services that take video (Gemini)
//!   codecs   which decoders and encoders this machine has, HEVC included
//!
//! Every function opens the source read-only; nothing here can write to the input (CLAUDE.md rule 5).

use std::time::Instant;
use windows::core::{Interface, GUID, HSTRING, PWSTR};
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Variant::{VARIANT, VT_R4};

const ALL: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const EOS: u32 = MF_SOURCE_READERF_ENDOFSTREAM.0 as u32;
/// Media Foundation counts time in 100-nanosecond units.
const HNS: f64 = 10_000_000.0;

type R<T> = windows::core::Result<T>;

pub fn startup() -> R<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL)
    }
}

unsafe fn attributes(pairs: &[(GUID, u32)]) -> R<IMFAttributes> {
    let mut a = None;
    MFCreateAttributes(&mut a, pairs.len() as u32)?;
    let a = a.unwrap();
    for (k, v) in pairs {
        a.SetUINT32(k, *v)?;
    }
    Ok(a)
}

unsafe fn reader(path: &str, video_processing: bool) -> R<IMFSourceReader> {
    let attr = attributes(&[(
        MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING,
        video_processing as u32,
    )])?;
    MFCreateSourceReaderFromURL(&HSTRING::from(path), &attr)
}

unsafe fn pcm_type(rate: u32, channels: u32) -> R<IMFMediaType> {
    let t = MFCreateMediaType()?;
    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
    t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
    t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)?;
    t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)?;
    t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
    t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 2 * channels)?;
    t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 2 * channels * rate)?;
    t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
    Ok(t)
}

unsafe fn sample_bytes(sample: &IMFSample, out: &mut Vec<u8>) -> R<()> {
    let buf = sample.ConvertToContiguousBuffer()?;
    let mut p = std::ptr::null_mut();
    let mut len = 0u32;
    buf.Lock(&mut p, None, Some(&mut len))?;
    out.extend_from_slice(std::slice::from_raw_parts(p, len as usize));
    buf.Unlock()
}

/// What the source reader actually delivered, which may differ from what was asked.
pub struct Pcm {
    pub rate: u32,
    pub channels: u32,
    pub samples: Vec<i16>,
}

unsafe fn decode_reader(r: &IMFSourceReader, rate: u32, channels: u32) -> R<Pcm> {
    r.SetStreamSelection(ALL, false)?;
    r.SetStreamSelection(AUDIO, true)?;
    r.SetCurrentMediaType(AUDIO, None, &pcm_type(rate, channels)?)?;
    let got = r.GetCurrentMediaType(AUDIO)?;
    let rate = got.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)?;
    let channels = got.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)?;
    let mut bytes = Vec::new();
    loop {
        let mut flags = 0u32;
        let mut sample = None;
        r.ReadSample(AUDIO, 0, None, Some(&mut flags), None, Some(&mut sample))?;
        if let Some(s) = sample {
            sample_bytes(&s, &mut bytes)?;
        }
        if flags & EOS != 0 {
            break;
        }
    }
    let samples = bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    Ok(Pcm {
        rate,
        channels,
        samples,
    })
}

/// Decodes the first audio stream of `path`, converted to `rate` Hz and `channels` by Media Foundation.
pub fn decode(path: &str, rate: u32, channels: u32) -> R<Pcm> {
    unsafe { decode_reader(&reader(path, false)?, rate, channels) }
}

/// The same from bytes in memory, such as an MP3 from Edge TTS.
pub fn decode_bytes(data: &[u8], rate: u32, channels: u32) -> R<Pcm> {
    unsafe {
        let stream = CreateStreamOnHGlobal(HGLOBAL::default(), true)?;
        stream
            .Write(data.as_ptr() as _, data.len() as u32, None)
            .ok()?;
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        let bs = MFCreateMFByteStreamOnStream(&stream)?;
        let r = MFCreateSourceReaderFromByteStream(&bs, None)?;
        decode_reader(&r, rate, channels)
    }
}

pub struct Info {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub rotation: u32,
    pub video: Option<GUID>,
    pub audio: Option<GUID>,
}

pub fn info(path: &str) -> R<Info> {
    unsafe {
        let r = reader(path, false)?;
        let d =
            r.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)?;
        let duration = u64::try_from(&d).unwrap_or(0) as f64 / HNS;
        let mut i = Info {
            duration,
            width: 0,
            height: 0,
            rotation: 0,
            video: None,
            audio: None,
        };
        if let Ok(t) = r.GetNativeMediaType(VIDEO, 0) {
            i.video = t.GetGUID(&MF_MT_SUBTYPE).ok();
            let size = t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
            i.width = (size >> 32) as u32;
            i.height = size as u32;
            i.rotation = t.GetUINT32(&MF_MT_VIDEO_ROTATION).unwrap_or(0);
        }
        if let Ok(t) = r.GetNativeMediaType(AUDIO, 0) {
            i.audio = t.GetGUID(&MF_MT_SUBTYPE).ok();
        }
        Ok(i)
    }
}

fn wic() -> R<IWICImagingFactory> {
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
}

unsafe fn stream_bytes(stream: &IStream) -> R<Vec<u8>> {
    let mut pos = 0u64;
    stream.Seek(0, STREAM_SEEK_CUR, Some(&mut pos))?;
    let hg = GetHGlobalFromStream(stream)?;
    let n = GlobalSize(hg);
    let p = GlobalLock(hg) as *const u8;
    if p.is_null() {
        return Err(windows::core::Error::empty());
    }
    let out = std::slice::from_raw_parts(p, (pos as usize).min(n)).to_vec();
    let _ = GlobalUnlock(hg);
    Ok(out)
}

unsafe fn jpeg(f: &IWICImagingFactory, src: &IWICBitmapSource, quality: f32) -> R<Vec<u8>> {
    let (mut w, mut h) = (0, 0);
    src.GetSize(&mut w, &mut h)?;
    let stream = CreateStreamOnHGlobal(HGLOBAL::default(), true)?;
    let enc = f.CreateEncoder(&GUID_ContainerFormatJpeg, std::ptr::null())?;
    enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
    let mut fe: Option<IWICBitmapFrameEncode> = None;
    let mut bag: Option<IPropertyBag2> = None;
    enc.CreateNewFrame(&mut fe, &mut bag)?;
    let (Some(fe), Some(bag)) = (fe, bag) else {
        return Err(windows::core::Error::empty());
    };
    let name = HSTRING::from("ImageQuality");
    let pb = PROPBAG2 {
        pstrName: PWSTR(name.as_ptr() as *mut u16),
        ..Default::default()
    };
    let mut v = VARIANT::default();
    (*v.Anonymous.Anonymous).vt = VT_R4;
    (*v.Anonymous.Anonymous).Anonymous.fltVal = quality;
    bag.Write(1, &pb, &v)?;
    fe.Initialize(&bag)?;
    fe.SetSize(w, h)?;
    let mut pf = GUID_WICPixelFormat24bppBGR;
    fe.SetPixelFormat(&mut pf)?;
    fe.WriteSource(src, std::ptr::null())?;
    fe.Commit()?;
    enc.Commit()?;
    stream_bytes(&stream)
}

/// The frame showing at `t` seconds, as a JPEG whose long edge is at most `long_edge`.
/// Returns the JPEG and the timestamp of the frame actually used.
pub fn frame(path: &str, t: f64, long_edge: u32) -> R<(Vec<u8>, f64)> {
    unsafe {
        let r = reader(path, std::env::var_os("SPIKE_MF_NOVP").is_none())?;
        r.SetStreamSelection(ALL, false)?;
        r.SetStreamSelection(VIDEO, true)?;
        let native = r.GetNativeMediaType(VIDEO, 0)?;
        let native_size = native.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let mut rotation = native.GetUINT32(&MF_MT_VIDEO_ROTATION).unwrap_or(0);
        let want = MFCreateMediaType()?;
        want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        want.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        r.SetCurrentMediaType(VIDEO, None, &want)?;
        let got = r.GetCurrentMediaType(VIDEO)?;
        let size = got.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let (w, h) = ((size >> 32) as u32, size as u32);
        // The advanced video processor applies the stream's rotation itself (seen on Windows 11 26200):
        // a 90-degree stream comes out with width and height swapped. Rotate only if it did not.
        if rotation % 180 == 90 && size != native_size {
            rotation = 0;
        }
        if std::env::var_os("SPIKE_MF_DEBUG").is_some() {
            eprintln!("decoded {w}x{h}, stream rotation {rotation}");
        }
        let target = (t * HNS) as i64;
        let pos = PROPVARIANT::from(target);
        if std::env::var_os("SPIKE_MF_NOSEEK").is_none() {
            r.SetCurrentPosition(&GUID::zeroed(), &pos)?;
        }
        // Seeking lands on the key frame before `t`; read forward to the frame on screen at `t`.
        let mut last: Option<(IMFSample, i64)> = None;
        loop {
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut sample = None;
            r.ReadSample(
                VIDEO,
                0,
                None,
                Some(&mut flags),
                Some(&mut ts),
                Some(&mut sample),
            )?;
            if std::env::var_os("SPIKE_MF_DEBUG").is_some() {
                eprintln!("read: flags {flags:#x} ts {ts} sample {}", sample.is_some());
            }
            if let Some(s) = sample {
                let past = ts > target;
                if past && last.is_some() {
                    break;
                }
                last = Some((s, ts));
                if past {
                    break;
                }
            }
            if flags & EOS != 0 {
                break;
            }
        }
        let Some((sample, ts)) = last else {
            return Err(windows::core::Error::empty());
        };
        let mut bgra = Vec::new();
        sample_bytes(&sample, &mut bgra)?;
        // RGB32 from the source reader is bottom-up only for negative strides; the video processor
        // gives top-down, stride = width * 4.
        let f = wic()?;
        let bitmap = f.CreateBitmapFromMemory(w, h, &GUID_WICPixelFormat32bppBGR, w * 4, &bgra)?;
        let scale = (long_edge as f64 / w.max(h) as f64).min(1.0);
        let (sw, sh) = (
            ((w as f64 * scale) as u32).max(1),
            ((h as f64 * scale) as u32).max(1),
        );
        let scaler = f.CreateBitmapScaler()?;
        scaler.Initialize(&bitmap, sw, sh, WICBitmapInterpolationModeFant)?;
        let rot = f.CreateBitmapFlipRotator()?;
        rot.Initialize(
            &scaler,
            match rotation {
                90 => WICBitmapTransformRotate90,
                180 => WICBitmapTransformRotate180,
                270 => WICBitmapTransformRotate270,
                _ => WICBitmapTransformRotate0,
            },
        )?;
        let conv = f.CreateFormatConverter()?;
        conv.Initialize(
            &rot,
            &GUID_WICPixelFormat24bppBGR,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )?;
        Ok((jpeg(&f, &conv.cast()?, 0.85)?, ts as f64 / HNS))
    }
}

/// Writes 16-bit PCM to an MP3 file at `kbps` (Media Foundation's encoder takes 128, 160, 192, 256, 320).
pub fn mp3(path: &str, pcm: &Pcm, kbps: u32) -> R<()> {
    unsafe {
        let w = MFCreateSinkWriterFromURL(&HSTRING::from(path), None, None)?;
        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_MP3)?;
        out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, pcm.rate)?;
        out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, pcm.channels)?;
        out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, kbps * 1000 / 8)?;
        let stream = w.AddStream(&out)?;
        w.SetInputMediaType(stream, &pcm_type(pcm.rate, pcm.channels)?, None)?;
        w.BeginWriting()?;
        write_pcm(&w, stream, pcm, 0)?;
        w.Finalize()
    }
}

/// Feeds PCM to a sink writer stream in 100 ms samples starting at `start` (100 ns units).
unsafe fn write_pcm(w: &IMFSinkWriter, stream: u32, pcm: &Pcm, start: i64) -> R<()> {
    let frame = pcm.channels as usize;
    let chunk = (pcm.rate as usize / 10) * frame;
    let mut done = 0usize;
    for c in pcm.samples.chunks(chunk) {
        let bytes = c.len() * 2;
        let buf = MFCreateMemoryBuffer(bytes as u32)?;
        let mut p = std::ptr::null_mut();
        buf.Lock(&mut p, None, None)?;
        std::ptr::copy_nonoverlapping(c.as_ptr() as *const u8, p, bytes);
        buf.Unlock()?;
        buf.SetCurrentLength(bytes as u32)?;
        let s = MFCreateSample()?;
        s.AddBuffer(&buf)?;
        let t0 = start + (done / frame) as i64 * 10_000_000 / pcm.rate as i64;
        let n = c.len() / frame;
        s.SetSampleTime(t0)?;
        s.SetSampleDuration(n as i64 * 10_000_000 / pcm.rate as i64)?;
        w.WriteSample(stream, &s)?;
        done += c.len();
    }
    Ok(())
}

/// Cuts `start..end` seconds of `path` into a small MP4: H.264 with the long edge at most `long_edge`
/// and at most `fps` frames a second, plus mono AAC, so a service that takes video gets the picture
/// and the soundtrack in as few bytes as possible.
pub fn clip(path: &str, start: f64, end: f64, out_path: &str, long_edge: u32, fps: u32) -> R<()> {
    unsafe {
        let r = reader(path, true)?;
        r.SetStreamSelection(ALL, false)?;
        r.SetStreamSelection(VIDEO, true)?;
        r.SetStreamSelection(AUDIO, true)?;
        let native = r.GetNativeMediaType(VIDEO, 0)?;
        let size = native.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let (w0, h0) = ((size >> 32) as u32, size as u32);
        let scale = (long_edge as f64 / w0.max(h0) as f64).min(1.0);
        // H.264 wants even dimensions; multiples of 16 avoid padding surprises.
        let even = |x: f64| ((x as u32) / 16 * 16).max(16);
        let (w, h) = (even(w0 as f64 * scale), even(h0 as f64 * scale));
        let (num, den) = {
            let fr = native
                .GetUINT64(&MF_MT_FRAME_RATE)
                .unwrap_or((25u64 << 32) | 1);
            ((fr >> 32) as u32, (fr as u32).max(1))
        };
        let src_fps = num as f64 / den as f64;
        let out_fps = (fps as f64).min(src_fps);

        let vin = MFCreateMediaType()?;
        vin.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        vin.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        vin.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
        r.SetCurrentMediaType(VIDEO, None, &vin)?;
        let vin = r.GetCurrentMediaType(VIDEO)?;
        vin.SetUINT64(&MF_MT_FRAME_RATE, ((out_fps.round() as u64) << 32) | 1)?;
        let has_audio = r
            .SetCurrentMediaType(AUDIO, None, &pcm_type(48_000, 1)?)
            .is_ok();

        let attr = attributes(&[(MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)])?;
        let sw = MFCreateSinkWriterFromURL(&HSTRING::from(out_path), None, &attr)?;
        let vout = MFCreateMediaType()?;
        vout.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        vout.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        vout.SetUINT32(&MF_MT_AVG_BITRATE, 400_000)?;
        vout.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        vout.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
        vout.SetUINT64(&MF_MT_FRAME_RATE, ((out_fps.round() as u64) << 32) | 1)?;
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
            sw.SetInputMediaType(s, &pcm_type(48_000, 1)?, None)?;
            Some(s)
        } else {
            None
        };
        sw.BeginWriting()?;

        let t0 = (start * HNS) as i64;
        let t1 = (end * HNS) as i64;
        r.SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(t0))?;
        let step = (HNS / out_fps) as i64;
        let mut next_video = t0;
        let (mut video_done, mut audio_done) = (false, !has_audio);
        while !(video_done && audio_done) {
            let mut idx = 0u32;
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut sample = None;
            r.ReadSample(
                ANY_STREAM,
                0,
                Some(&mut idx),
                Some(&mut flags),
                Some(&mut ts),
                Some(&mut sample),
            )?;
            let is_video =
                r.GetCurrentMediaType(idx)?.GetGUID(&MF_MT_MAJOR_TYPE)? == MFMediaType_Video;
            let ended = flags & EOS != 0 || ts >= t1;
            if let Some(s) = sample {
                if ts >= t0 && ts < t1 {
                    if is_video {
                        if ts >= next_video {
                            s.SetSampleTime(ts - t0)?;
                            s.SetSampleDuration(step)?;
                            sw.WriteSample(vs, &s)?;
                            next_video += step;
                        }
                    } else if let Some(a) = as_ {
                        s.SetSampleTime(ts - t0)?;
                        sw.WriteSample(a, &s)?;
                    }
                }
            }
            if ended {
                if is_video {
                    video_done = true;
                    r.SetStreamSelection(VIDEO, false)?;
                } else {
                    audio_done = true;
                    r.SetStreamSelection(AUDIO, false)?;
                }
            }
        }
        sw.Finalize()
    }
}

const ANY_STREAM: u32 = MF_SOURCE_READER_ANY_STREAM.0 as u32;

fn fourcc(g: &GUID) -> String {
    let b = g.data1.to_le_bytes();
    if b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b' ') {
        String::from_utf8_lossy(&b).into_owned()
    } else {
        format!("0x{:x}", g.data1)
    }
}

/// How many MFTs of `category` turn `input` into `output` (GUID::zeroed() = anything).
fn count_mfts(category: GUID, major: GUID, input: Option<GUID>, output: Option<GUID>) -> u32 {
    unsafe {
        let i = input.map(|g| MFT_REGISTER_TYPE_INFO {
            guidMajorType: major,
            guidSubtype: g,
        });
        let o = output.map(|g| MFT_REGISTER_TYPE_INFO {
            guidMajorType: major,
            guidSubtype: g,
        });
        let mut arr = std::ptr::null_mut();
        let mut n = 0u32;
        let flags = MFT_ENUM_FLAG_SYNCMFT
            | MFT_ENUM_FLAG_ASYNCMFT
            | MFT_ENUM_FLAG_HARDWARE
            | MFT_ENUM_FLAG_LOCALMFT
            | MFT_ENUM_FLAG_SORTANDFILTER;
        if MFTEnumEx(
            category,
            flags,
            i.as_ref().map(|x| x as *const _),
            o.as_ref().map(|x| x as *const _),
            &mut arr,
            &mut n,
        )
        .is_err()
        {
            return 0;
        }
        for k in 0..n as usize {
            drop((*arr.add(k)).take());
        }
        CoTaskMemFree(Some(arr as _));
        n
    }
}

pub fn codecs() {
    let v = MFMediaType_Video;
    let a = MFMediaType_Audio;
    let rows: [(&str, GUID, GUID, Option<GUID>, Option<GUID>); 9] = [
        (
            "H.264 decoder",
            MFT_CATEGORY_VIDEO_DECODER,
            v,
            Some(MFVideoFormat_H264),
            None,
        ),
        (
            "HEVC decoder",
            MFT_CATEGORY_VIDEO_DECODER,
            v,
            Some(MFVideoFormat_HEVC),
            None,
        ),
        (
            "VP9 decoder",
            MFT_CATEGORY_VIDEO_DECODER,
            v,
            Some(MFVideoFormat_VP90),
            None,
        ),
        (
            "AV1 decoder",
            MFT_CATEGORY_VIDEO_DECODER,
            v,
            Some(MFVideoFormat_AV1),
            None,
        ),
        (
            "H.264 encoder",
            MFT_CATEGORY_VIDEO_ENCODER,
            v,
            None,
            Some(MFVideoFormat_H264),
        ),
        (
            "AAC decoder",
            MFT_CATEGORY_AUDIO_DECODER,
            a,
            Some(MFAudioFormat_AAC),
            None,
        ),
        (
            "MP3 decoder",
            MFT_CATEGORY_AUDIO_DECODER,
            a,
            Some(MFAudioFormat_MP3),
            None,
        ),
        (
            "AAC encoder",
            MFT_CATEGORY_AUDIO_ENCODER,
            a,
            None,
            Some(MFAudioFormat_AAC),
        ),
        (
            "MP3 encoder",
            MFT_CATEGORY_AUDIO_ENCODER,
            a,
            None,
            Some(MFAudioFormat_MP3),
        ),
    ];
    for (name, cat, major, i, o) in rows {
        println!("{name}: {}", count_mfts(cat, major, i, o));
    }
}

fn write_wav(path: &str, pcm: &Pcm) -> std::io::Result<()> {
    let data = pcm.samples.len() as u32 * 2;
    let mut b = Vec::with_capacity(44 + data as usize);
    let ch = pcm.channels as u16;
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&ch.to_le_bytes());
    b.extend_from_slice(&pcm.rate.to_le_bytes());
    b.extend_from_slice(&(pcm.rate * 2 * ch as u32).to_le_bytes());
    b.extend_from_slice(&(2 * ch).to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for s in &pcm.samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, b)
}

/// `mf <command> ...`, see the module comment.
pub fn run(args: &[&str]) -> Result<(), String> {
    startup().map_err(|e| e.to_string())?;
    let num = |s: &str| s.parse::<f64>().map_err(|_| format!("not a number: {s}"));
    let e = |e: windows::core::Error| format!("{} ({:#010x})", e.message().trim(), e.code().0);
    let t = Instant::now();
    match args {
        ["codecs"] => codecs(),
        ["count", path] => {
            let (n, last) = count_native(path).map_err(e)?;
            println!("{n} compressed video samples, last at {last:.2} s, in {} ms", t.elapsed().as_millis());
        }
        ["info", path] => {
            let i = info(path).map_err(e)?;
            println!(
                "duration {:.2} s, video {} {}x{} rotation {}, audio {}",
                i.duration,
                i.video.as_ref().map(fourcc).unwrap_or("none".into()),
                i.width,
                i.height,
                i.rotation,
                i.audio.as_ref().map(fourcc).unwrap_or("none".into())
            );
        }
        ["decode", path, rate, channels, out] => {
            let pcm = decode(path, num(rate)? as u32, num(channels)? as u32).map_err(e)?;
            println!(
                "{} Hz, {} ch, {:.2} s, in {} ms",
                pcm.rate,
                pcm.channels,
                pcm.samples.len() as f64 / pcm.channels as f64 / pcm.rate as f64,
                t.elapsed().as_millis()
            );
            write_wav(out, &pcm).map_err(|e| e.to_string())?;
        }
        ["frame", path, at, edge, out] => {
            let (jpg, ts) = frame(path, num(at)?, num(edge)? as u32).map_err(e)?;
            println!("frame at {ts:.3} s, {} bytes, in {} ms", jpg.len(), t.elapsed().as_millis());
            std::fs::write(out, jpg).map_err(|e| e.to_string())?;
        }
        ["mp3", input, kbps, out] => {
            let pcm = decode(input, 48_000, 2).map_err(e)?;
            mp3(out, &pcm, num(kbps)? as u32).map_err(e)?;
            println!("wrote {} bytes in {} ms", std::fs::metadata(out).map(|m| m.len()).unwrap_or(0), t.elapsed().as_millis());
        }
        ["mp3-bytes", input, out] => {
            let data = std::fs::read(input).map_err(|e| e.to_string())?;
            let pcm = decode_bytes(&data, 24_000, 1).map_err(e)?;
            println!(
                "{} Hz, {} ch, {:.3} s from memory, in {} ms",
                pcm.rate,
                pcm.channels,
                pcm.samples.len() as f64 / pcm.channels as f64 / pcm.rate as f64,
                t.elapsed().as_millis()
            );
            write_wav(out, &pcm).map_err(|e| e.to_string())?;
        }
        ["clip", path, start, end, edge, fps, out] => {
            clip(path, num(start)?, num(end)?, out, num(edge)? as u32, num(fps)? as u32).map_err(e)?;
            println!("wrote {} bytes in {} ms", std::fs::metadata(out).map(|m| m.len()).unwrap_or(0), t.elapsed().as_millis());
        }
        _ => return Err("usage: spike mf codecs | info <file> | decode <file> <rate> <channels> <out.wav> | frame <file> <seconds> <long_edge> <out.jpg> | mp3 <file> <kbps> <out.mp3> | mp3-bytes <file.mp3> <out.wav> | clip <file> <start> <end> <long_edge> <fps> <out.mp4>".into()),
    }
    Ok(())
}

/// Diagnostic: how many compressed video samples the source reader delivers, without decoding.
pub fn count_native(path: &str) -> R<(u32, f64)> {
    unsafe {
        let r = reader(path, false)?;
        r.SetStreamSelection(ALL, false)?;
        r.SetStreamSelection(VIDEO, true)?;
        let (mut n, mut last) = (0u32, 0i64);
        loop {
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut sample = None;
            r.ReadSample(
                VIDEO,
                0,
                None,
                Some(&mut flags),
                Some(&mut ts),
                Some(&mut sample),
            )?;
            if sample.is_some() {
                n += 1;
                last = ts;
            }
            if flags & EOS != 0 {
                break;
            }
        }
        Ok((n, last as f64 / HNS))
    }
}
