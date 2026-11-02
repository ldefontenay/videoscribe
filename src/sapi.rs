//! SAPI 5, Windows' own voices, rendered to a memory stream: the fallback when
//! Edge cannot be reached (PLAN.md §4.4). Moved from spike 0(g).
//!
//! The voice speaks into an `SpStream` wrapped round an HGLOBAL `IStream`, in a fixed 24 kHz mono 16-bit
//! format, so nothing is played aloud. Duration is samples / rate; leading and trailing silence are
//! measured separately because scheduling needs the audible length, not the padded one.
//!
//! Voices come from two token categories: SAPI's own (`Speech\Voices`) and the OneCore voices that
//! Windows 10 and 11 install for Narrator (`Speech_OneCore\Voices`), which SAPI can also load.

use windows::core::{GUID, HSTRING, PCWSTR};
use windows::Win32::Media::Audio::{WAVEFORMATEX, WAVE_FORMAT_PCM};
use windows::Win32::Media::Speech::*;
use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
use windows::Win32::System::Com::*;

/// {C31ADBAE-527F-4FF5-A230-F62BB61FF70C}: the stream carries the WAVEFORMATEX passed beside it.
const SPDFID_WAVE_FORMAT_EX: GUID = GUID::from_u128(0xc31adbae_527f_4ff5_a230_f62bb61ff70c);

const CATEGORIES: [&str; 2] = [
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Speech\\Voices",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Speech_OneCore\\Voices",
];

pub struct Voice {
    pub name: String,
    /// The language code it speaks, such as "fr"; empty when the voice does
    /// not say.
    pub language: String,
    token: ISpObjectToken,
}

/// The language code of a voice token, from its Attributes key's Language
/// value: a hexadecimal LCID, or several separated by semicolons ("809;9").
unsafe fn language_of(token: &ISpObjectToken) -> String {
    let Ok(attrs) = token.OpenKey(&HSTRING::from("Attributes")) else {
        return String::new();
    };
    let Ok(pw) = attrs.GetStringValue(&HSTRING::from("Language")) else {
        return String::new();
    };
    let text = pw.to_string().unwrap_or_default();
    CoTaskMemFree(Some(pw.0 as _));
    lcid_language(&text)
}

/// "40C" as "fr", "809;9" as "en", "436" as "af": the primary language of
/// the first LCID, through Windows' own table.
pub fn lcid_language(text: &str) -> String {
    let Some(lcid) = text
        .split(';')
        .next()
        .and_then(|h| u32::from_str_radix(h.trim(), 16).ok())
    else {
        return String::new();
    };
    let mut buf = [0u16; 85];
    let n = unsafe { windows::Win32::Globalization::LCIDToLocaleName(lcid, Some(&mut buf), 0) };
    if n <= 1 {
        return String::new();
    }
    let name = String::from_utf16_lossy(&buf[..n as usize - 1]);
    name.split('-').next().unwrap_or("").to_lowercase()
}

pub fn voices() -> windows::core::Result<Vec<Voice>> {
    let mut out = Vec::new();
    for cat in CATEGORIES.iter() {
        unsafe {
            let category: ISpObjectTokenCategory =
                CoCreateInstance(&SpObjectTokenCategory, None, CLSCTX_ALL)?;
            if category.SetId(&HSTRING::from(*cat), false).is_err() {
                continue;
            }
            let tokens = category.EnumTokens(PCWSTR::null(), PCWSTR::null())?;
            let mut count = 0;
            tokens.GetCount(&mut count)?;
            for n in 0..count {
                let token = tokens.Item(n)?;
                let pw = token.GetStringValue(PCWSTR::null())?;
                let name = pw.to_string().unwrap_or_default();
                CoTaskMemFree(Some(pw.0 as _));
                let language = language_of(&token);
                out.push(Voice {
                    name,
                    language,
                    token,
                });
            }
        }
    }
    Ok(out)
}

fn format(hz: u32) -> WAVEFORMATEX {
    WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: 1,
        nSamplesPerSec: hz,
        nAvgBytesPerSec: hz * 2,
        nBlockAlign: 2,
        wBitsPerSample: 16,
        cbSize: 0,
    }
}

/// Speaks `text` (plain text, not SSML) with `voice` at `rate` (-10..10)
/// into mono samples at `hz` a second.
pub fn speak_hz(voice: &Voice, text: &str, rate: i32, hz: u32) -> windows::core::Result<Vec<i16>> {
    unsafe {
        let mem = CreateStreamOnHGlobal(Default::default(), true)?;
        let stream: ISpStream = CoCreateInstance(&SpStream, None, CLSCTX_ALL)?;
        let wfx = format(hz);
        stream.SetBaseStream(&mem, &SPDFID_WAVE_FORMAT_EX, &wfx)?;
        let sp: ISpVoice = CoCreateInstance(&SpVoice, None, CLSCTX_ALL)?;
        sp.SetVoice(&voice.token)?;
        sp.SetRate(rate)?;
        sp.SetOutput(&stream, false)?;
        // SPF_IS_NOT_XML: the text is spoken literally, so a description can never inject markup.
        sp.Speak(&HSTRING::from(text), SPF_IS_NOT_XML.0 as u32, None)?;
        sp.WaitUntilDone(u32::MAX)?;
        let _ = sp.SetOutput(None::<&windows::core::IUnknown>, false);
        let mut len = 0u64;
        mem.Seek(0, STREAM_SEEK_CUR, Some(&mut len))?;
        mem.Seek(0, STREAM_SEEK_SET, None)?;
        let mut bytes = vec![0u8; len as usize];
        let mut read = 0u32;
        mem.Read(bytes.as_mut_ptr() as _, len as u32, Some(&mut read))
            .ok()?;
        bytes.truncate(read as usize);
        Ok(bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b))
            .collect())
    }
}

/// (leading, trailing) silence in seconds, below -50 dBFS in 10 ms windows.
pub fn silence(samples: &[i16], rate: u32) -> (f64, f64) {
    let win = (rate / 100) as usize;
    let loud: Vec<bool> = samples
        .chunks(win)
        .map(|c| c.iter().map(|&s| (s as i32).abs()).max().unwrap_or(0) > 104)
        .collect();
    let first = loud.iter().position(|&l| l).unwrap_or(loud.len());
    let last = loud
        .iter()
        .rposition(|&l| l)
        .map(|i| i + 1)
        .unwrap_or(first);
    (first as f64 / 100.0, (loud.len() - last) as f64 / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_voices_language_comes_from_its_lcid() {
        assert_eq!(lcid_language("40C"), "fr");
        assert_eq!(lcid_language("809;9"), "en");
        assert_eq!(lcid_language("436"), "af");
        assert_eq!(lcid_language("nonsense"), "");
    }

    /// The Windows voices here and the languages they speak:
    /// `cargo test windows_voices -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn windows_voices() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        for v in voices().unwrap() {
            println!("{}: {}", v.name, v.language);
        }
    }
}
