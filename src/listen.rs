//! The Listen to the described sound window (asked for in the milestone 8
//! test): the whole described sound, with Play or pause, Back and Forward 10
//! seconds, and the arrow keys for 2 seconds. Opened with Ctrl+L, or Listen to
//! the described sound in the Describe menu, or from a place in Review.
//!
//! Every move is said out loud, as the window has nothing to see: where
//! playing is now, and whether it plays. Except a skip while playing, by
//! arrow or button: the sound says where it is, and a time over it is clutter
//! (round 2 and 3 tests); T says it on request. The Where playing is box holds
//! the same, for reading at any time.
//!
//! Opened from Review, Enter goes back to it, on the description just spoken
//! (round 3 test, 3.1), so listening and correcting go hand in hand.

use std::cell::Cell;
use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, VK_HOME, VK_LEFT, VK_RETURN, VK_RIGHT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::player::Player;
use crate::speech::clock;
use crate::ui::{focus, set_text};

const IDD_LISTEN: u16 = 105;
const IDC_L_WHERE: i32 = 1060;
const IDC_L_PLAY: i32 = 1061;
const IDC_L_BACK: i32 = 1062;
const IDC_L_FORWARD: i32 = 1063;

/// Seconds a button skips, and an arrow key (2, log entry 33, Q1).
const SKIP: f64 = 10.0;
const STEP: f64 = 2.0;
/// The key that says where playing is.
const VK_T: u16 = b'T' as u16;

/// How often the window checks on the player: for the end, and to keep the
/// Where playing is box current.
const TICK_MS: u32 = 250;
const TIMER: usize = 1;

struct State {
    player: Player,
    /// Opened from Review: Enter goes back to it.
    from_review: bool,
    /// Where playing was when Enter went back to Review.
    back_at: Option<f64>,
    /// What the Where playing is box last said, to change it only when needed.
    shown: String,
}

thread_local! {
    /// The open window, for the keyboard hook.
    static OPEN: Cell<Option<HWND>> = const { Cell::new(None) };
}

/// Opens the window on `path` and starts playing it `from` that many seconds
/// in. From Review, where playing was if Enter was pressed to go back to it.
pub fn show(owner: HWND, path: &Path, from: f64, from_review: bool) -> Result<Option<f64>, String> {
    let player = Player::open(path)?;
    if from > 0.0 {
        player.seek(from);
    }
    let mut state = State {
        player,
        from_review,
        back_at: None,
        shown: String::new(),
    };
    unsafe {
        let hook = SetWindowsHookExW(WH_MSGFILTER, Some(keys), None, GetCurrentThreadId()).ok();
        if let Ok(instance) = GetModuleHandleW(None) {
            DialogBoxParamW(
                Some(instance.into()),
                PCWSTR(IDD_LISTEN as usize as *const u16),
                Some(owner),
                Some(proc),
                LPARAM(&mut state as *mut State as isize),
            );
        }
        if let Some(h) = hook {
            let _ = UnhookWindowsHookEx(h);
        }
    }
    OPEN.with(|o| o.set(None));
    Ok(state.back_at)
}

/// "1:23 of 5:11", in whole seconds: tenths are too many to listen to.
fn where_text(at: f64, secs: f64) -> String {
    let whole = |t: f64| {
        let c = clock(t.floor());
        c.split_once('.').map_or(c.clone(), |(a, _)| a.to_string())
    };
    format!("{} of {}", whole(at), whole(secs))
}

unsafe fn state<'a>(hwnd: HWND) -> &'a mut State {
    &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State)
}

fn say(hwnd: HWND, text: &str) {
    crate::announce::say(hwnd, text);
}

/// Keeps the Where playing is box current, but not while it has focus, so
/// a screen reader reading it is not interrupted.
fn show_where(hwnd: HWND, s: &mut State, even_with_focus: bool) {
    let text = format!(
        "{}, {}",
        where_text(s.player.at(), s.player.secs()),
        if s.player.playing() {
            "playing"
        } else {
            "paused"
        }
    );
    if text == s.shown {
        return;
    }
    let has_focus = unsafe { GetDlgItem(Some(hwnd), IDC_L_WHERE).is_ok_and(|c| GetFocus() == c) };
    if has_focus && !even_with_focus {
        return;
    }
    set_text(hwnd, IDC_L_WHERE, &text);
    s.shown = text;
}

fn toggle(hwnd: HWND, s: &mut State) {
    if s.player.playing() {
        s.player.pause();
        say(
            hwnd,
            &format!("Paused at {}.", where_text(s.player.at(), s.player.secs())),
        );
    } else {
        s.player.play();
        say(
            hwnd,
            &format!(
                "Playing from {}.",
                where_text(s.player.at(), s.player.secs())
            ),
        );
    }
    show_where(hwnd, s, false);
}

/// Moves by `by` seconds. Paused, says the time it moved to, and nothing
/// more (round 4 test, 2.2); playing, nothing, as the sound says where it is
/// (round 2 and 3 tests).
fn skip(hwnd: HWND, s: &mut State, by: f64) {
    let to = s.player.seek(s.player.at() + by);
    if !s.player.playing() {
        let at = where_text(to, s.player.secs());
        say(hwnd, at.split(" of ").next().unwrap_or(&at));
    }
    show_where(hwnd, s, false);
}

/// The arrow keys, Home and T, anywhere in the window except the Where playing
/// is box, whose text they read. A dialog's own buttons do nothing with
/// them, so nothing is lost.
unsafe extern "system" fn keys(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == MSGF_DIALOGBOX as i32 {
        let msg = &*(lparam.0 as *const MSG);
        if let Some(dialog) = OPEN.with(|o| o.get()) {
            let ours = msg.hwnd == dialog || IsChild(dialog, msg.hwnd).as_bool();
            let in_box = GetDlgItem(Some(dialog), IDC_L_WHERE).is_ok_and(|c| c == msg.hwnd);
            let key = msg.wParam.0 as u16;
            if msg.message == WM_KEYDOWN && ours && (!in_box || key == VK_RETURN.0) {
                let s = state(dialog);
                if key == VK_LEFT.0 {
                    skip(dialog, s, -STEP);
                    return LRESULT(1);
                }
                if key == VK_RIGHT.0 {
                    skip(dialog, s, STEP);
                    return LRESULT(1);
                }
                if key == VK_RETURN.0 && s.from_review {
                    s.back_at = Some(s.player.at());
                    s.player.pause();
                    let _ = KillTimer(Some(dialog), TIMER);
                    let _ = EndDialog(dialog, IDOK.0 as isize);
                    return LRESULT(1);
                }
                if key == VK_T {
                    let at = where_text(s.player.at(), s.player.secs());
                    let how = if s.player.playing() {
                        "playing"
                    } else {
                        "paused"
                    };
                    say(dialog, &format!("{at}, {how}."));
                    return LRESULT(1);
                }
                if key == VK_HOME.0 {
                    // Said even while playing: it is a long way to go.
                    s.player.seek(0.0);
                    say(dialog, "At the start.");
                    show_where(dialog, s, false);
                    return LRESULT(1);
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

unsafe extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match message {
        WM_INITDIALOG => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
            OPEN.with(|o| o.set(Some(hwnd)));
            let s = state(hwnd);
            s.player.play();
            show_where(hwnd, s, true);
            SetTimer(Some(hwnd), TIMER, TICK_MS, None);
            focus(hwnd, IDC_L_PLAY);
            let at = s.player.at();
            let long = where_text(0.0, s.player.secs());
            let long = long.split(" of ").nth(1).unwrap_or_default();
            say(
                hwnd,
                &if s.from_review {
                    // Review has just said where (round 3 test, 3.1).
                    "Playing. Enter goes back to the description just spoken.".to_string()
                } else if at > 0.0 {
                    format!("Playing from {}.", where_text(at, s.player.secs()))
                } else {
                    format!("Playing from the start. {long} long.")
                },
            );
            0
        }
        WM_TIMER => {
            let s = state(hwnd);
            if let Some(why) = s.player.take_failure() {
                say(hwnd, &format!("Playing stopped: {why}"));
            } else if s.player.take_ended() {
                say(
                    hwnd,
                    "Reached the end. Play or pause starts again from the beginning.",
                );
            }
            show_where(hwnd, s, false);
            0
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            let s = state(hwnd);
            match id {
                IDC_L_PLAY => toggle(hwnd, s),
                // Only the time, as the arrows (round 4 test, 2.2).
                IDC_L_BACK => skip(hwnd, s, -SKIP),
                IDC_L_FORWARD => skip(hwnd, s, SKIP),
                // Current as it gets focus (EN_SETFOCUS).
                IDC_L_WHERE if code == 0x0100 => show_where(hwnd, s, true),
                _ if id == IDCANCEL.0 => {
                    let _ = KillTimer(Some(hwnd), TIMER);
                    s.player.pause();
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

    #[test]
    fn where_playing_is_said_in_whole_seconds() {
        assert_eq!(where_text(83.7, 311.6), "1:23 of 5:11");
        assert_eq!(where_text(0.0, 3725.0), "0:00 of 1:02:05");
    }
}
