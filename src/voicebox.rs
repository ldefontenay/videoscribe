//! The Language and voice window (Options, Ctrl+Shift+V; log entry 46): the
//! languages Edge speaks, found by typing their first letters, then that
//! language's voices, each playing a short sample at the chosen speed a
//! moment after it is reached. Planned at milestone 7; Edge's whole list
//! comes from `voices.rs`.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::announce;
use crate::play::{self, Playing};
use crate::voices::{self, Voice};

const IDD_VOICE: u16 = 109;
const IDC_V_LANGUAGE: i32 = 1100;
const IDC_V_VOICE: i32 = 1101;
const IDC_V_PLAY: i32 = 1102;

const LBN_SELCHANGE: u32 = 1;
/// The sample plays this long after a voice is reached, so arrowing past
/// voices does not start each one.
const SAMPLE_TIMER: usize = 1;
const SAMPLE_DELAY_MS: u32 = 600;
/// Edge's list arrived: fill the lists again.
const WM_LISTED: u32 = WM_APP + 1;
/// A sample arrived: `lparam` is a `Box<Sample>`.
const WM_SAMPLE: u32 = WM_APP + 2;

struct Sample {
    voice: String,
    spoken: Result<Vec<i16>, String>,
}

struct State {
    language: String,
    voice: String,
    rate: i32,
    list: Arc<Vec<Voice>>,
    playing: Option<Playing>,
    /// The voice whose sample is being fetched, so a late one is not played
    /// over the next.
    asked: Option<String>,
}

/// The language and voice chosen, or None when the user cancelled.
pub fn show(owner: HWND, language: &str, voice: &str, rate: i32) -> Option<(String, String)> {
    let mut state = State {
        language: language.to_string(),
        voice: voice.to_string(),
        rate,
        list: voices::all(),
        playing: None,
        asked: None,
    };
    let ok = unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        DialogBoxParamW(
            Some(instance.into()),
            PCWSTR(IDD_VOICE as usize as *const u16),
            Some(owner),
            Some(proc),
            LPARAM(&mut state as *mut State as isize),
        ) == IDOK.0 as isize
    };
    play::stop();
    ok.then_some((state.language, state.voice))
}

unsafe fn state<'a>(hwnd: HWND) -> Option<&'a mut State> {
    (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State).as_mut()
}

fn lb(hwnd: HWND, id: i32, msg: u32, w: usize, l: isize) -> isize {
    unsafe { SendDlgItemMessageW(hwnd, id, msg, WPARAM(w), LPARAM(l)).0 }
}

fn fill(hwnd: HWND, id: i32, items: &[String], at: usize) {
    lb(hwnd, id, LB_RESETCONTENT, 0, 0);
    for item in items {
        let w = crate::wide(item);
        lb(hwnd, id, LB_ADDSTRING, 0, w.as_ptr() as isize);
    }
    lb(hwnd, id, LB_SETCURSEL, at, 0);
}

fn chosen(hwnd: HWND, id: i32) -> Option<usize> {
    usize::try_from(lb(hwnd, id, LB_GETCURSEL, 0, 0)).ok()
}

fn fill_languages(hwnd: HWND, s: &State) {
    let languages = voices::languages(&s.list);
    let at = languages.iter().position(|l| *l == s.language).unwrap_or(0);
    fill(hwnd, IDC_V_LANGUAGE, &languages, at);
}

/// The chosen language's voices, with `s.voice` chosen when it is one of
/// them, else the language's first voice, which becomes `s.voice`.
fn fill_voices(hwnd: HWND, s: &mut State) {
    let languages = voices::languages(&s.list);
    let Some(language) = chosen(hwnd, IDC_V_LANGUAGE).and_then(|i| languages.get(i)) else {
        return;
    };
    s.language = language.clone();
    let of = voices::of_language(&s.list, language);
    let at = match of.iter().position(|v| v.id == s.voice) {
        Some(at) => at,
        None => {
            let first = crate::choices::first_voice(language);
            of.iter()
                .position(|v| Some(&v.id) == first.as_ref())
                .unwrap_or(0)
        }
    };
    if let Some(v) = of.get(at) {
        s.voice = v.id.clone();
    }
    let items: Vec<String> = of
        .iter()
        .map(|v| format!("{}, {}", v.name, v.kind))
        .collect();
    fill(hwnd, IDC_V_VOICE, &items, at);
}

fn chosen_voice(hwnd: HWND, s: &State) -> Option<Voice> {
    let of = voices::of_language(&s.list, &s.language);
    chosen(hwnd, IDC_V_VOICE).and_then(|i| of.get(i).map(|v| (*v).clone()))
}

/// Fetches the chosen voice's sample off the window's thread, as Media
/// Foundation decodes it; it is played when it arrives.
fn fetch_sample(hwnd: HWND, s: &mut State) {
    let Some(v) = chosen_voice(hwnd, s) else {
        return;
    };
    s.playing = None;
    s.asked = Some(v.id.clone());
    let rate = s.rate;
    let window = hwnd.0 as isize;
    std::thread::spawn(move || {
        let spoken = (|| {
            let _session = crate::media::Session::start()?;
            let work = crate::ytdlp::local_dir().join("sample");
            crate::speak::Voice::new(&v.id, rate, work).sample(&voices::sample(&v))
        })();
        let boxed = Box::new(Sample {
            voice: v.id,
            spoken,
        });
        unsafe {
            let raw = Box::into_raw(boxed);
            if PostMessageW(
                Some(HWND(window as *mut _)),
                WM_SAMPLE,
                WPARAM(0),
                LPARAM(raw as isize),
            )
            .is_err()
            {
                drop(Box::from_raw(raw));
            }
        }
    });
}

unsafe extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match message {
        WM_INITDIALOG => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
            for id in [IDC_V_LANGUAGE, IDC_V_VOICE] {
                if let Ok(list) = GetDlgItem(Some(hwnd), id) {
                    let _ = SetWindowSubclass(list, Some(type_ahead), 1, 0);
                }
            }
            if let Some(s) = state(hwnd) {
                fill_languages(hwnd, s);
                fill_voices(hwnd, s);
            }
            // Edge's whole list, fetched again when a week old.
            let window = hwnd.0 as isize;
            std::thread::spawn(move || {
                if voices::refresh().is_ok() {
                    let _ = PostMessageW(
                        Some(HWND(window as *mut _)),
                        WM_LISTED,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            });
            crate::ui::focus(hwnd, IDC_V_LANGUAGE);
            0
        }
        WM_LISTED => {
            if let Some(s) = state(hwnd) {
                let list = voices::all();
                if list.len() != s.list.len() {
                    s.list = list;
                    fill_languages(hwnd, s);
                    fill_voices(hwnd, s);
                }
            }
            1
        }
        WM_SAMPLE => {
            let sample = Box::from_raw(lparam.0 as *mut Sample);
            if let Some(s) = state(hwnd) {
                if s.asked.as_deref() == Some(sample.voice.as_str()) {
                    s.asked = None;
                    match sample.spoken {
                        Ok(samples) => {
                            s.playing = Some(play::play_samples(&samples, crate::speak::RATE))
                        }
                        Err(why) => {
                            announce::say(hwnd, &why);
                        }
                    }
                }
            }
            1
        }
        WM_TIMER if wparam.0 == SAMPLE_TIMER => {
            let _ = KillTimer(Some(hwnd), SAMPLE_TIMER);
            if let Some(s) = state(hwnd) {
                fetch_sample(hwnd, s);
            }
            1
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            let Some(s) = state(hwnd) else { return 0 };
            match (id, code) {
                (IDC_V_LANGUAGE, LBN_SELCHANGE) => {
                    let _ = KillTimer(Some(hwnd), SAMPLE_TIMER);
                    s.playing = None;
                    fill_voices(hwnd, s);
                }
                (IDC_V_VOICE, LBN_SELCHANGE) => {
                    if let Some(v) = chosen_voice(hwnd, s) {
                        s.voice = v.id;
                    }
                    s.playing = None;
                    s.asked = None;
                    SetTimer(Some(hwnd), SAMPLE_TIMER, SAMPLE_DELAY_MS, None);
                }
                (IDC_V_PLAY, _) => {
                    let _ = KillTimer(Some(hwnd), SAMPLE_TIMER);
                    fetch_sample(hwnd, s);
                }
                (id, _) if id == IDOK.0 || id == IDCANCEL.0 => {
                    let _ = KillTimer(Some(hwnd), SAMPLE_TIMER);
                    s.playing = None;
                    let _ = EndDialog(hwnd, id as isize);
                }
                _ => return 0,
            }
            1
        }
        _ => 0,
    }
}

/// When the last letter was typed into a list, and what has been typed.
static TYPED_AT: AtomicIsize = AtomicIsize::new(0);
static TYPED: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Type-ahead in a list: letters typed within a second of each other find
/// the first item that starts with them all ("af" finds Afrikaans, past
/// Albanian); the same letter again moves to the next item with it, as a
/// list does by itself.
unsafe extern "system" fn type_ahead(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if message == WM_CHAR {
        if let Some(c) = char::from_u32(wparam.0 as u32).filter(|c| !c.is_control()) {
            let now = START.elapsed().as_millis() as isize;
            let last = TYPED_AT.swap(now, Ordering::Relaxed);
            let mut typed = TYPED.lock().unwrap_or_else(|e| e.into_inner());
            if now - last > 1000 {
                typed.clear();
            }
            typed.push(c);
            let all_same = typed.chars().all(|t| t.eq_ignore_ascii_case(&c));
            let current = SendMessageW(hwnd, LB_GETCURSEL, None, None).0;
            let (from, text) = if all_same && typed.chars().count() > 1 {
                (current, c.to_string())
            } else if typed.chars().count() == 1 {
                (current, typed.clone())
            } else {
                (current - 1, typed.clone())
            };
            let w = crate::wide(&text);
            let found = SendMessageW(
                hwnd,
                LB_FINDSTRING,
                Some(WPARAM(from as usize)),
                Some(LPARAM(w.as_ptr() as isize)),
            )
            .0;
            if found >= 0 && found != current {
                SendMessageW(hwnd, LB_SETCURSEL, Some(WPARAM(found as usize)), None);
                let id = GetDlgCtrlID(hwnd) as usize;
                if let Ok(parent) = GetParent(hwnd) {
                    SendMessageW(
                        parent,
                        WM_COMMAND,
                        Some(WPARAM(id | ((LBN_SELCHANGE as usize) << 16))),
                        Some(LPARAM(hwnd.0 as isize)),
                    );
                }
            }
            return LRESULT(0);
        }
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

static START: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);
