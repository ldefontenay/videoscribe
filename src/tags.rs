//! Tags in the described sound and video, so that a media player or File
//! Explorer can tell what each file is (test round 3, section 5): written
//! through the Windows property system, which writes MP3's ID3 tags and
//! MP4's own, as Photo Scribe writes its photos' descriptions.

use std::path::Path;

use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::StructuredStorage::*;
use windows::Win32::System::Variant::*;
use windows::Win32::UI::Shell::PropertiesSystem::*;

/// What goes in a described file's tags.
pub struct Tags {
    /// "Sintel trailer, described".
    pub title: String,
    /// What it was made from and with, for Explorer's Comments.
    pub comment: String,
    /// Every place and what is said there, as the file's lyrics, which some
    /// players show while playing.
    pub lyrics: String,
    pub year: u32,
}

/// Album, album artist and genre: the same for every described file, so a
/// player keeps them together.
const ALBUM: &str = "Video Scribe";
const GENRE: &str = "Audio description";

enum Value<'a> {
    Text(&'a str),
    List(&'a str),
    Number(u32),
}

/// Writes `tags` into the file at `path`. A property the file's handler
/// refuses is left out, not a failure: MP3 and MP4 take them all.
pub fn write(path: &Path, tags: &Tags) -> Result<(), String> {
    let explain = |e: windows::core::Error| e.message().trim().to_string();
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreFromParsingName(
            &HSTRING::from(path.as_os_str()),
            None,
            GPS_READWRITE,
        )
        .map_err(explain)?;
        let props = [
            ("System.Title", Value::Text(&tags.title)),
            ("System.Comment", Value::Text(&tags.comment)),
            ("System.Music.AlbumTitle", Value::Text(ALBUM)),
            ("System.Music.AlbumArtist", Value::Text(ALBUM)),
            ("System.Music.Genre", Value::List(GENRE)),
            ("System.Music.Lyrics", Value::Text(&tags.lyrics)),
            ("System.Media.Year", Value::Number(tags.year)),
        ];
        for (name, value) in props {
            let mut key = PROPERTYKEY::default();
            if PSGetPropertyKeyFromName(&HSTRING::from(name), &mut key).is_err() {
                continue;
            }
            let mut pv = match value {
                Value::Text(s) => {
                    let mut pv = PROPVARIANT::default();
                    let s: PWSTR =
                        windows::Win32::UI::Shell::SHStrDupW(&HSTRING::from(s)).map_err(explain)?;
                    let inner = &mut *pv.Anonymous.Anonymous;
                    inner.vt = VT_LPWSTR;
                    inner.Anonymous.pwszVal = s;
                    pv
                }
                Value::List(s) => {
                    let h = HSTRING::from(s);
                    InitPropVariantFromStringVector(Some(&[PCWSTR(h.as_ptr())])).map_err(explain)?
                }
                Value::Number(n) => {
                    let mut pv = PROPVARIANT::default();
                    let inner = &mut *pv.Anonymous.Anonymous;
                    inner.vt = VT_UI4;
                    inner.Anonymous.ulVal = n;
                    pv
                }
            };
            let _ = store.SetValue(&key, &pv);
            let _ = PropVariantClear(&mut pv);
        }
        store.Commit().map_err(explain)
    }
}

/// The year now, here.
pub fn this_year() -> u32 {
    unsafe { windows::Win32::System::SystemInformation::GetLocalTime() }.wYear as u32
}

/// A text property, read back through a fresh property store.
#[cfg(test)]
pub fn read(path: &Path, name: &str) -> Option<String> {
    unsafe {
        let store: IPropertyStore =
            SHGetPropertyStoreFromParsingName(&HSTRING::from(path.as_os_str()), None, GPS_DEFAULT)
                .ok()?;
        let mut key = PROPERTYKEY::default();
        PSGetPropertyKeyFromName(&HSTRING::from(name), &mut key).ok()?;
        let pv = store.GetValue(&key).ok()?;
        let s = PropVariantToStringAlloc(&pv).ok()?;
        let out = s.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(s.0 as _));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes tags into a copy of an MP3 or MP4, and reads them back. Run
    /// with VS_TAGS_MP3 set to such a file:
    /// `cargo test tags_read_back -- --ignored`.
    #[test]
    #[ignore]
    fn tags_read_back() {
        let from = std::env::var("VS_TAGS_MP3").expect("VS_TAGS_MP3");
        let ext = Path::new(&from)
            .extension()
            .map_or("mp3".into(), |e| e.to_string_lossy().into_owned());
        let copy = std::env::temp_dir().join(format!("video scribe tags test.{ext}"));
        std::fs::copy(&from, &copy).unwrap();
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            );
        }
        let tags = Tags {
            title: "Sintel trailer, described".into(),
            comment: "Audio described by Video Scribe.".into(),
            lyrics: "Place 1, at 0:00.2: Snow falls.\r\nPlace 2, at 0:05.0: A girl runs.".into(),
            year: 2026,
        };
        write(&copy, &tags).unwrap();
        for name in [
            "System.Title",
            "System.Comment",
            "System.Music.AlbumTitle",
            "System.Music.Genre",
            "System.Music.Lyrics",
            "System.Media.Year",
        ] {
            println!("{name}: {:?}", read(&copy, name));
        }
        assert_eq!(
            read(&copy, "System.Title").as_deref(),
            Some("Sintel trailer, described")
        );
        if ext == "mp3" {
            assert_eq!(read(&copy, "System.Music.Lyrics"), Some(tags.lyrics));
        }
        let _ = std::fs::remove_file(&copy);
    }
}
