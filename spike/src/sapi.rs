//! Spike 0(g): SAPI 5 rendered to a memory stream, measured (PLAN.md §8).
//!
//! The voice speaks into an `SpStream` wrapped round an HGLOBAL `IStream`, in a fixed 24 kHz mono 16-bit
//! format, so nothing is played aloud. Duration is samples / rate; leading and trailing silence are
//! measured separately because scheduling needs the audible length, not the padded one.
//!
//! Voices come from two token categories: SAPI's own (`Speech\Voices`) and the OneCore voices that
//! Windows 10 and 11 install for Narrator (`Speech_OneCore\Voices`), which SAPI can also load.

use std::time::Instant;
use windows::core::{GUID, HSTRING, PCWSTR};
use windows::Win32::Media::Audio::{WAVEFORMATEX, WAVE_FORMAT_PCM};
use windows::Win32::Media::Speech::*;
use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
use windows::Win32::System::Com::*;

pub const RATE: u32 = 24_000;

/// {C31ADBAE-527F-4FF5-A230-F62BB61FF70C}: the stream carries the WAVEFORMATEX passed beside it.
const SPDFID_WAVE_FORMAT_EX: GUID = GUID::from_u128(0xc31adbae_527f_4ff5_a230_f62bb61ff70c);

const CATEGORIES: [&str; 2] = [
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Speech\\Voices",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Speech_OneCore\\Voices",
];

pub struct Voice {
    pub name: String,
    pub category: &'static str,
    token: ISpObjectToken,
}

pub fn com_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

pub fn voices() -> windows::core::Result<Vec<Voice>> {
    let mut out = Vec::new();
    for (i, cat) in CATEGORIES.iter().enumerate() {
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
                out.push(Voice {
                    name,
                    category: if i == 0 { "SAPI" } else { "OneCore" },
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

/// Speaks `text` (plain text, not SSML) with `voice` at `rate` (-10..10) into 24 kHz mono samples.
pub fn speak(voice: &Voice, text: &str, rate: i32) -> windows::core::Result<Vec<i16>> {
    speak_hz(voice, text, rate, RATE)
}

/// As `speak`, at `hz` samples a second.
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
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
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

pub fn write_wav(path: &str, samples: &[i16], rate: u32) -> std::io::Result<()> {
    let data = samples.len() as u32 * 2;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, b)
}

/// Sentences of typical description lengths, 1 to 12 words.
pub const SAMPLES: [&str; 6] = [
    "Grandma.",
    "Sophie climbs onto the swing.",
    "Mum, Dad and Grandma sit round the table.",
    "A banner reads Happy Birthday Sophie.",
    "The bride and groom walk out under a shower of confetti.",
    "In a dark kitchen, a girl in a red coat peers into the open fridge.",
];

pub fn words(s: &str) -> usize {
    s.split_whitespace().count()
}

/// `sapi [out_dir]`: list voices, speak each sample with each voice, print timings.
pub fn run(out_dir: Option<&str>) -> Result<(), String> {
    run_inner(out_dir).map_err(|e| e.to_string())
}

fn run_inner(out_dir: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    com_init();
    let voices = voices()?;
    println!("{} voices", voices.len());
    for v in &voices {
        println!("  [{}] {}", v.category, v.name);
    }
    println!();
    println!("voice | words | total s | lead s | trail s | speech s | words/s | render ms");
    for (vi, v) in voices.iter().enumerate() {
        let mut total_words = 0;
        let mut total_speech = 0.0;
        for (si, s) in SAMPLES.iter().enumerate() {
            let t = Instant::now();
            let samples = match speak(v, s, 0) {
                Ok(x) => x,
                Err(e) => {
                    println!("{} | failed: {e}", v.name);
                    break;
                }
            };
            let ms = t.elapsed().as_millis();
            let total = samples.len() as f64 / RATE as f64;
            let (lead, trail) = silence(&samples, RATE);
            let speech = total - lead - trail;
            total_words += words(s);
            total_speech += speech;
            println!(
                "{} | {} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {}",
                v.name,
                words(s),
                total,
                lead,
                trail,
                speech,
                words(s) as f64 / speech,
                ms
            );
            if let Some(dir) = out_dir {
                write_wav(&format!("{dir}\\sapi-{vi}-{si}.wav"), &samples, RATE)?;
            }
        }
        if total_speech > 0.0 {
            println!(
                "{} | overall {:.2} words/s",
                v.name,
                total_words as f64 / total_speech
            );
        }
    }
    Ok(())
}
