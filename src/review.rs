//! The Review the descriptions window (milestone 8): each place of the last
//! described sound, the text spoken there, and buttons to play that stretch,
//! in the described sound or in the film's own. Text changed here is spoken
//! instead the next time the sound is made, which sends nothing. Opened with
//! Ctrl+R, or Review the descriptions in the Describe menu.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::job::{Review, Reviewed};
use crate::play::{self, Playing};
use crate::ui::{focus, get_text, set_text};
use crate::wide;

const IDD_REVIEW: u16 = 104;
const IDC_R_LIST: i32 = 1050;
const IDC_R_TEXT: i32 = 1051;
const IDC_R_PLAY: i32 = 1052;
const IDC_R_FILM: i32 = 1053;
const IDC_R_STOP: i32 = 1054;
const IDC_R_RESTORE: i32 = 1055;
const IDC_R_HEAR: i32 = 1056;
const IDC_R_LISTEN: i32 = 1057;
/// Says whether a heard text fits, once it has been heard.
const FIT_TIMER: usize = 1;

const LBN_SELCHANGE: u32 = 1;
const EN_CHANGE: u32 = 0x0300;

/// Seconds played before a place and after it, to hear it in its setting.
const BEFORE: f64 = 2.0;
/// Listen on from here starts this long before a place (round 4 test, 3.3,
/// and round 5, 1.1).
const LISTEN_BEFORE: f64 = 0.5;
const AFTER: f64 = 1.0;

struct State {
    review: Review,
    /// The user's own text for places, as it now stands in this window.
    edits: Vec<(usize, String)>,
    playing: Option<Playing>,
    /// Set while the window changes its own controls, so that is ignored.
    filling: bool,
    /// The voice and speed descriptions are spoken in, for hearing a text.
    voice: String,
    rate: i32,
    /// What to say once a heard text has played: whether it fits.
    after: Option<String>,
}

/// Opens the window on a video's last described sound, with the user's own
/// text so far. Their text as it stands when the window is closed.
pub fn show(
    owner: HWND,
    review: Review,
    edits: Vec<(usize, String)>,
    voice: &str,
    rate: i32,
) -> Vec<(usize, String)> {
    let mut state = State {
        review,
        edits,
        playing: None,
        filling: false,
        voice: voice.to_string(),
        rate,
        after: None,
    };
    unsafe {
        if let Ok(instance) = GetModuleHandleW(None) {
            DialogBoxParamW(
                Some(instance.into()),
                PCWSTR(IDD_REVIEW as usize as *const u16),
                Some(owner),
                Some(proc),
                LPARAM(&mut state as *mut State as isize),
            );
        }
    }
    state.playing = None;
    state.edits
}

/// The text that would be spoken at a place now, and the text that is in
/// the sound.
fn texts<'a>(p: &'a Reviewed, edits: &'a [(usize, String)]) -> (&'a str, &'a str) {
    let in_sound = p.yours.as_deref().unwrap_or(&p.described);
    let now = edits
        .iter()
        .find(|(n, _)| *n == p.place)
        .map_or(p.described.as_str(), |(_, t)| t.as_str());
    (now, in_sound)
}

/// A place's line in the list: the report's own, then whether the user's
/// text is in it, or waiting for the sound to be made again.
pub fn item(p: &Reviewed, edits: &[(usize, String)]) -> String {
    let (now, in_sound) = texts(p, edits);
    if now != in_sound {
        let what = if now.is_empty() {
            "nothing".to_string()
        } else {
            format!("\"{now}\"")
        };
        format!(
            "{} Changed by you, to say {what}: not in the sound until it is made again.",
            p.line
        )
    } else if p.yours.is_some() {
        format!("{} Your own text.", p.line)
    } else {
        p.line.clone()
    }
}

unsafe fn state<'a>(hwnd: HWND) -> &'a mut State {
    &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State)
}

fn selected(hwnd: HWND, s: &State) -> Option<usize> {
    let i = unsafe { SendDlgItemMessageW(hwnd, IDC_R_LIST, LB_GETCURSEL, WPARAM(0), LPARAM(0)) };
    (i.0 >= 0 && (i.0 as usize) < s.review.places.len()).then_some(i.0 as usize)
}

fn add(hwnd: HWND, at: Option<usize>, line: &str) {
    let w = wide(line);
    unsafe {
        SendDlgItemMessageW(
            hwnd,
            IDC_R_LIST,
            if at.is_some() {
                LB_INSERTSTRING
            } else {
                LB_ADDSTRING
            },
            WPARAM(at.unwrap_or(0)),
            LPARAM(w.as_ptr() as isize),
        );
    }
}

fn fill(hwnd: HWND, s: &State) {
    unsafe {
        SendDlgItemMessageW(hwnd, IDC_R_LIST, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
    }
    for p in &s.review.places {
        add(hwnd, None, &item(p, &s.edits));
    }
    unsafe {
        SendDlgItemMessageW(hwnd, IDC_R_LIST, LB_SETCURSEL, WPARAM(0), LPARAM(0));
    }
}

/// Rewrites one place's line, keeping it chosen.
fn refresh(hwnd: HWND, s: &State, i: usize) {
    unsafe {
        SendDlgItemMessageW(hwnd, IDC_R_LIST, LB_DELETESTRING, WPARAM(i), LPARAM(0));
    }
    add(hwnd, Some(i), &item(&s.review.places[i], &s.edits));
    unsafe {
        SendDlgItemMessageW(hwnd, IDC_R_LIST, LB_SETCURSEL, WPARAM(i), LPARAM(0));
    }
}

/// Puts the chosen place's text in the text box.
fn show_text(hwnd: HWND, s: &mut State) {
    let text = selected(hwnd, s)
        .map(|i| texts(&s.review.places[i], &s.edits).0.to_string())
        .unwrap_or_default();
    s.filling = true;
    set_text(hwnd, IDC_R_TEXT, &text);
    s.filling = false;
}

/// Keeps the text box's text for the chosen place, or forgets it when it is
/// the description again.
fn keep_text(hwnd: HWND, s: &mut State) {
    let Some(i) = selected(hwnd, s) else {
        return;
    };
    let p = &s.review.places[i];
    let text = get_text(hwnd, IDC_R_TEXT)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let n = p.place;
    s.edits.retain(|(m, _)| *m != n);
    if text != p.described {
        s.edits.push((n, text));
        s.edits.sort_by_key(|(m, _)| *m);
    }
    refresh(hwnd, s, i);
}

/// From a little before a place to a little after it, in the described
/// sound: its pause, or its description if that runs on.
pub fn described_stretch(p: &Reviewed) -> (f64, f64) {
    let pause_end = p.start + (p.film.1 - p.film.0);
    let end = p
        .said
        .map_or(pause_end, |(a, secs)| pause_end.max(a + secs));
    ((p.start - BEFORE).max(0.0), end + AFTER)
}

fn say(hwnd: HWND, text: &str) {
    crate::announce::say(hwnd, text);
}

fn play_place(hwnd: HWND, s: &mut State, film: bool) {
    let Some(i) = selected(hwnd, s) else {
        return;
    };
    let p = &s.review.places[i];
    let (path, (from, to)) = if film {
        (
            &s.review.film,
            ((p.film.0 - BEFORE).max(0.0), p.film.1 + AFTER),
        )
    } else {
        (&s.review.output, described_stretch(p))
    };
    if !path.exists() {
        say(
            hwnd,
            if film {
                "The film's own sound is no longer on this computer: a downloaded video is \
                 deleted once the described sound is made, unless Keep video downloads for a day \
                 is on."
            } else {
                "The described sound is no longer where it was saved, so it cannot be played."
            },
        );
        return;
    }
    s.playing = None;
    s.after = None;
    match play::play(path, from, to) {
        Ok(p) => s.playing = Some(p),
        Err(why) => say(hwnd, &format!("It could not be played: {why}")),
    }
}

/// Whether `secs` of speech fits a pause of `pause` seconds, in words.
pub fn fit(secs: f64, pause: f64) -> String {
    let over = secs - pause;
    if over <= 0.0 {
        format!("It takes {secs:.1} seconds, and its pause is {pause:.1}: it fits.")
    } else {
        format!(
            "It takes {secs:.1} seconds, and its pause is {pause:.1}: {over:.1} too long. It \
             will be spoken up to 15% faster, then your choice for a description that does not \
             fit applies."
        )
    }
}

/// Speaks the text box's text in the voice and speed descriptions use, then
/// says whether it fits its pause.
fn hear(hwnd: HWND, s: &mut State) {
    let Some(i) = selected(hwnd, s) else {
        return;
    };
    let text = get_text(hwnd, IDC_R_TEXT)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        say(
            hwnd,
            "The text box is empty, so nothing would be said here.",
        );
        return;
    }
    s.playing = None;
    let (voice, rate) = (s.voice.clone(), s.rate);
    // Off the window's thread, as Media Foundation decodes Edge's speech.
    let spoken = std::thread::spawn(move || {
        let _session = crate::media::Session::start()?;
        let work = crate::ytdlp::local_dir().join("work");
        let _ = std::fs::create_dir_all(&work);
        let mut v = crate::speak::Voice::new(&voice, rate, work);
        let percent = v.percent(1.0);
        v.say(&text, percent)
    })
    .join()
    .unwrap_or_else(|_| Err("The voice failed.".into()));
    match spoken {
        Ok(samples) => {
            let rate = crate::speak::RATE;
            let secs = samples.len() as f64 / rate as f64;
            let p = &s.review.places[i];
            s.after = Some(fit(secs, p.film.1 - p.film.0));
            s.playing = Some(play::play_samples(&samples, rate));
            unsafe {
                SetTimer(Some(hwnd), FIT_TIMER, (secs * 1000.0) as u32 + 300, None);
            }
        }
        Err(why) => say(hwnd, &format!("It could not be spoken: {why}")),
    }
}

/// The Listen window, from a little before the chosen place to the end
/// (log entry 33, Q3). What it plays is the described sound as made, without
/// changes not yet spoken.
fn listen_on(hwnd: HWND, s: &mut State) {
    let Some(i) = selected(hwnd, s) else {
        return;
    };
    let from = (s.review.places[i].start - LISTEN_BEFORE).max(0.0);
    s.after = None;
    s.playing = None;
    play::stop();
    match crate::listen::show(hwnd, &s.review.output, from, true) {
        Ok(Some(at)) => {
            if let Some(i) = just_spoken(&s.review.places, at) {
                unsafe {
                    SendDlgItemMessageW(hwnd, IDC_R_LIST, LB_SETCURSEL, WPARAM(i), LPARAM(0));
                }
                show_text(hwnd, s);
            }
            focus(hwnd, IDC_R_LIST);
        }
        Ok(None) => {}
        Err(why) => say(hwnd, &format!("It could not be played: {why}")),
    }
}

/// The place whose description was the last to start by `at` seconds into
/// the described sound.
fn just_spoken(places: &[Reviewed], at: f64) -> Option<usize> {
    places
        .iter()
        .rposition(|p| p.said.is_some_and(|(a, _)| a <= at))
}

unsafe extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match message {
        WM_INITDIALOG => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
            let s = state(hwnd);
            fill(hwnd, s);
            show_text(hwnd, s);
            focus(hwnd, IDC_R_LIST);
            0
        }
        WM_TIMER => {
            let _ = KillTimer(Some(hwnd), FIT_TIMER);
            if let Some(text) = state(hwnd).after.take() {
                say(hwnd, &text);
            }
            0
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            let s = state(hwnd);
            match (id, code) {
                (IDC_R_LIST, LBN_SELCHANGE) => show_text(hwnd, s),
                (IDC_R_TEXT, EN_CHANGE) if !s.filling => keep_text(hwnd, s),
                (IDC_R_PLAY, _) => play_place(hwnd, s, false),
                (IDC_R_FILM, _) => play_place(hwnd, s, true),
                (IDC_R_HEAR, _) => hear(hwnd, s),
                (IDC_R_LISTEN, _) => listen_on(hwnd, s),
                (IDC_R_STOP, _) => {
                    s.after = None;
                    s.playing = None;
                    play::stop();
                    say(hwnd, "Stopped.");
                }
                (IDC_R_RESTORE, _) => {
                    let Some(i) = selected(hwnd, s) else {
                        return 0;
                    };
                    let n = s.review.places[i].place;
                    if s.edits.iter().any(|(m, _)| *m == n) {
                        s.edits.retain(|(m, _)| *m != n);
                        refresh(hwnd, s, i);
                        show_text(hwnd, s);
                        let in_sound = s.review.places[i].yours.is_none();
                        say(
                            hwnd,
                            if in_sound {
                                "Restored: the text as described, which is what the sound has."
                            } else {
                                "Restored: the text as described. It is not in the sound until \
                                 it is made again."
                            },
                        );
                    } else {
                        say(hwnd, "This place already has the text as described.");
                    }
                }
                _ if id == IDCANCEL.0 => {
                    s.playing = None;
                    let _ = EndDialog(hwnd, id as isize);
                    return 1;
                }
                _ => {}
            }
            0
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(yours: Option<&str>) -> Reviewed {
        Reviewed {
            place: 3,
            film: (10.0, 14.0),
            start: 12.0,
            said: Some((12.1, 5.0)),
            line: "Place 3, at 0:12: A dog runs.".into(),
            described: "A dog runs.".into(),
            yours: yours.map(str::to_string),
        }
    }

    #[test]
    fn enter_in_listen_comes_back_to_the_description_just_spoken() {
        let place = |said: Option<(f64, f64)>| Reviewed {
            place: 1,
            film: (0.0, 1.0),
            start: 0.0,
            said,
            line: String::new(),
            described: String::new(),
            yours: None,
        };
        let places = [
            place(Some((2.0, 1.0))),
            place(None),
            place(Some((9.0, 2.0))),
        ];
        assert_eq!(just_spoken(&places, 1.0), None);
        assert_eq!(just_spoken(&places, 5.0), Some(0));
        assert_eq!(just_spoken(&places, 9.5), Some(2));
    }

    #[test]
    fn a_line_says_when_the_text_is_not_yet_in_the_sound() {
        let p = place(None);
        assert_eq!(item(&p, &[]), "Place 3, at 0:12: A dog runs.");
        let edits = vec![(3, "Sam's dog runs.".to_string())];
        assert!(item(&p, &edits).ends_with(
            "Changed by you, to say \"Sam's dog runs.\": not in the sound until it is made again."
        ));
        let edits = vec![(3, String::new())];
        assert!(item(&p, &edits).contains("to say nothing"));
        // The user's text, already in the sound.
        let p = place(Some("Sam's dog runs."));
        let edits = vec![(3, "Sam's dog runs.".to_string())];
        assert!(item(&p, &edits).ends_with("Your own text."));
        // Restored, but the sound still has the user's text.
        assert!(item(&p, &[]).contains("to say \"A dog runs.\""));
    }

    #[test]
    fn a_heard_text_says_whether_it_fits() {
        assert_eq!(
            fit(2.44, 3.1),
            "It takes 2.4 seconds, and its pause is 3.1: it fits."
        );
        assert!(fit(4.0, 3.1).contains("0.9 too long"));
    }

    #[test]
    fn a_place_is_played_with_a_little_either_side() {
        let near = |(a, b): (f64, f64), (x, y): (f64, f64)| {
            assert!((a - x).abs() < 1e-9 && (b - y).abs() < 1e-9, "{a} {b}")
        };
        // Said past the end of its pause: played to the end of the speech.
        near(described_stretch(&place(None)), (10.0, 18.1));
        let mut p = place(None);
        p.said = None;
        near(described_stretch(&p), (10.0, 17.0));
    }
}
