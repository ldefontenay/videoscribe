//! Still pictures from a video, as small JPEGs, for the models that look at
//! pictures (PLAN.md §4.3, spike 0(e)): Media Foundation decodes and seeks,
//! WIC scales, rotates and encodes. Both are part of Windows.
//!
//! The video is opened read-only (CLAUDE.md constraint 5).

use windows::core::{Interface, GUID, HSTRING, PWSTR};
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Variant::{VARIANT, VT_R4};

const ALL: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const EOS: u32 = MF_SOURCE_READERF_ENDOFSTREAM.0 as u32;
const HNS: f64 = 10_000_000.0;

/// The long edge of every picture sent, as in round 2.
pub const LONG_EDGE: u32 = 512;

/// One video, open for taking pictures. Make one per job, on a thread where
/// `media::Session` is running.
pub struct Frames {
    reader: IMFSourceReader,
    wic: IWICImagingFactory,
    width: u32,
    height: u32,
    rotation: u32,
    /// Seconds to add to a time on the video's clock to get Media
    /// Foundation's: YouTube's video once had its edit list removed
    /// (ytdlp::drop_edit_list).
    shift: f64,
    name: String,
}

impl Frames {
    pub fn open(path: &str, shift: f64) -> Result<Frames, String> {
        let name = std::path::Path::new(path)
            .file_name()
            .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned());
        let fail = |e: windows::core::Error| {
            format!(
                "Windows could not take pictures from {name}: {}",
                e.message().trim()
            )
        };
        unsafe {
            let mut attr = None;
            MFCreateAttributes(&mut attr, 1).map_err(fail)?;
            let attr = attr.ok_or_else(|| format!("Windows could not open {name}."))?;
            // The advanced video processor converts any decoder's output to RGB.
            attr.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
                .map_err(fail)?;
            let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path), &attr).map_err(fail)?;
            reader.SetStreamSelection(ALL, false).map_err(fail)?;
            if reader.SetStreamSelection(VIDEO, true).is_err() {
                return Err(format!(
                    "{name} has no picture, so there is nothing to describe."
                ));
            }
            let native = reader.GetNativeMediaType(VIDEO, 0).map_err(fail)?;
            let native_size = native.GetUINT64(&MF_MT_FRAME_SIZE).map_err(fail)?;
            let mut rotation = native.GetUINT32(&MF_MT_VIDEO_ROTATION).unwrap_or(0);
            let want = MFCreateMediaType().map_err(fail)?;
            want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(fail)?;
            want.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                .map_err(fail)?;
            reader.SetCurrentMediaType(VIDEO, None, &want).map_err(|e| {
                if e.code().0 as u32 == 0xC00D5212 {
                    // MF_E_TOPO_CODEC_NOT_FOUND: HEVC without the Store extension, say.
                    format!(
                        "Windows has no decoder for the picture in {name}. If it is an HEVC (H.265) \
                         video, the HEVC Video Extensions from the Microsoft Store add one."
                    )
                } else {
                    fail(e)
                }
            })?;
            let got = reader.GetCurrentMediaType(VIDEO).map_err(fail)?;
            let size = got.GetUINT64(&MF_MT_FRAME_SIZE).map_err(fail)?;
            // The video processor applies the stream's rotation itself (seen on
            // Windows 11 26200): a 90-degree stream comes out with width and
            // height swapped. Rotate only if it did not.
            if rotation % 180 == 90 && size != native_size {
                rotation = 0;
            }
            let wic: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(fail)?;
            Ok(Frames {
                reader,
                wic,
                width: (size >> 32) as u32,
                height: size as u32,
                rotation,
                shift,
                name,
            })
        }
    }

    /// The picture on screen at `t` seconds, as a JPEG with its long edge at
    /// most LONG_EDGE.
    pub fn at(&self, t: f64) -> Result<Vec<u8>, String> {
        let bgra = self.grab(t)?;
        unsafe {
            self.upright(&bgra, LONG_EDGE)
                .and_then(|src| encode(&self.wic, &src, 0.85))
        }
        .map_err(|e| self.failed(t, e))
    }

    /// The picture on screen at `t` seconds, upright, with its long edge at
    /// most `long_edge`, as rows of blue, green and red: for finding faces,
    /// on this computer only.
    pub fn pixels(&self, t: f64, long_edge: u32) -> Result<Pixels, String> {
        let bgra = self.grab(t)?;
        let copy = || unsafe {
            let src = self.upright(&bgra, long_edge)?;
            let (mut w, mut h) = (0, 0);
            src.GetSize(&mut w, &mut h)?;
            let mut bgr = vec![0u8; (w * h * 3) as usize];
            src.CopyPixels(std::ptr::null(), w * 3, &mut bgr)?;
            windows::core::Result::Ok(Pixels {
                width: w,
                height: h,
                bgr,
            })
        };
        copy().map_err(|e| self.failed(t, e))
    }

    fn failed(&self, t: f64, e: windows::core::Error) -> String {
        format!(
            "Windows could not take a picture from {} at {}: {}",
            self.name,
            crate::speech::clock(t),
            e.message().trim()
        )
    }

    /// The frame on screen at `t`, as the video processor gives it.
    fn grab(&self, t: f64) -> Result<Vec<u8>, String> {
        let fail = |e: windows::core::Error| {
            format!(
                "Windows could not take a picture from {} at {}: {}",
                self.name,
                crate::speech::clock(t),
                e.message().trim()
            )
        };
        unsafe {
            let target = ((t + self.shift).max(0.0) * HNS) as i64;
            self.reader
                .SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(target))
                .map_err(fail)?;
            // Seeking lands on the key frame before `t`; read on to the frame
            // on screen at `t`.
            let mut last: Option<IMFSample> = None;
            loop {
                let mut flags = 0u32;
                let mut ts = 0i64;
                let mut sample = None;
                self.reader
                    .ReadSample(
                        VIDEO,
                        0,
                        None,
                        Some(&mut flags),
                        Some(&mut ts),
                        Some(&mut sample),
                    )
                    .map_err(fail)?;
                if let Some(s) = sample {
                    let past = ts > target;
                    if past && last.is_some() {
                        break;
                    }
                    last = Some(s);
                    if past {
                        break;
                    }
                }
                if flags & EOS != 0 {
                    break;
                }
            }
            let Some(sample) = last else {
                return Err(format!(
                    "{} has no picture at {}.",
                    self.name,
                    crate::speech::clock(t)
                ));
            };
            let buf = sample.ConvertToContiguousBuffer().map_err(fail)?;
            let mut p = std::ptr::null_mut();
            let mut len = 0u32;
            buf.Lock(&mut p, None, Some(&mut len)).map_err(fail)?;
            let bgra = std::slice::from_raw_parts(p, len as usize).to_vec();
            let _ = buf.Unlock();
            Ok(bgra)
        }
    }

    /// A frame scaled to `long_edge` at most, turned upright, as 24-bit BGR.
    unsafe fn upright(
        &self,
        bgra: &[u8],
        long_edge: u32,
    ) -> windows::core::Result<IWICBitmapSource> {
        let (w, h) = (self.width, self.height);
        let f = &self.wic;
        // The video processor gives top-down rows, stride = width * 4.
        let bitmap = f.CreateBitmapFromMemory(w, h, &GUID_WICPixelFormat32bppBGR, w * 4, bgra)?;
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
            match self.rotation {
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
        conv.cast()
    }
}

/// A picture as rows of blue, green and red bytes, top first.
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub bgr: Vec<u8>,
}

unsafe fn encode(
    f: &IWICImagingFactory,
    src: &IWICBitmapSource,
    quality: f32,
) -> windows::core::Result<Vec<u8>> {
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
    // The encoder wrote from the start of the stream; its position is the length.
    let mut pos = 0u64;
    stream.Seek(0, STREAM_SEEK_CUR, Some(&mut pos))?;
    let hg = GetHGlobalFromStream(&stream)?;
    let n = GlobalSize(hg);
    let p = GlobalLock(hg) as *const u8;
    if p.is_null() {
        return Err(windows::core::Error::empty());
    }
    let out = std::slice::from_raw_parts(p, (pos as usize).min(n)).to_vec();
    let _ = GlobalUnlock(hg);
    Ok(out)
}
